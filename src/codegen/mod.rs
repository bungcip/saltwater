/// Return an error from a function
/// Assumes that 'Locatable' is in scope and that the function it is called in
/// returns a 'Result<Locatable<T>>'
macro_rules! semantic_err {
    ($message: expr, $location: expr $(,)?) => {
        return Err(CompileError::semantic(Locatable {
            data: $message,
            location: $location,
        }))
    };
}

mod expr;
mod static_init;
mod stmt;

use std::collections::HashMap;
use std::convert::TryFrom;
use std::path::Path;
use std::sync::Arc;

use crate::parser::arch::TARGET;
use crate::parser::{Opt, Program};
use codegen::ir::UserFuncName;
use cranelift::codegen::{
    self,
    ir::{
        InstBuilder,
        entities::StackSlot,
        function::Function,
        stackslot::{StackSlotData, StackSlotKind},
    },
    isa::TargetIsa,
    settings::{self, Configurable, Flags},
};
use cranelift::frontend::Switch;
use cranelift::prelude::{Block, FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{self, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::parser::data::{
    StorageClass,
    hir::{Declaration, Initializer, Stmt, Symbol},
    types::FunctionType,
    *,
};

pub(crate) fn get_isa() -> Arc<dyn TargetIsa + 'static> {
    let mut flags_builder = cranelift::codegen::settings::builder();

    // allow creating shared libraries
    flags_builder.enable("is_pic").expect("is_pic should be a valid option");

    // use debug assertions
    flags_builder
        .enable("enable_verifier")
        .expect("enable_verifier should be a valid option");
    // don't emit call to __cranelift_probestack
    flags_builder
        .set("enable_probestack", "false")
        .expect("enable_probestack should be a valid option");
    let flags = Flags::new(flags_builder);

    let result = cranelift::codegen::isa::lookup(TARGET)
        .unwrap_or_else(|_| panic!("platform not supported: {TARGET}"))
        .finish(flags);

    result.unwrap()
}

pub fn initialize_aot_module(name: String) -> ObjectModule {
    let builder = ObjectBuilder::new(get_isa(), name, cranelift_module::default_libcall_names());
    ObjectModule::new(builder.expect("unsupported binary format or target architecture"))
}

enum Id {
    Function(FuncId),
    Global(DataId),
    Local(StackSlot),
}

#[derive(PartialEq, PartialOrd)]
pub enum BlockState {
    Empty,
    Filled,
}

struct Compiler {
    module: ObjectModule,
    debug: bool,
    // if false, we last saw a switch
    last_saw_loop: bool,
    strings: HashMap<Vec<u8>, DataId>,
    declarations: HashMap<Symbol, Id>,
    loops: Vec<(Block, Block)>,
    // switch, default, end
    // if default is empty once we get to the end of a switch body,
    // we didn't see a default case
    switches: Vec<(Switch, Option<Block>, Block)>,
    labels: HashMap<InternedStr, Block>,
    error_handler: ErrorHandler,

    current_block_state: BlockState,
}

impl Compiler {
    fn new(module: ObjectModule, debug: bool) -> Compiler {
        Compiler {
            module,
            declarations: HashMap::new(),
            loops: Vec::new(),
            switches: Vec::new(),
            labels: HashMap::new(),
            // the initial value doesn't really matter
            last_saw_loop: true,
            strings: Default::default(),
            error_handler: Default::default(),
            debug,
            current_block_state: BlockState::Empty,
        }
    }
    // we have to consider the following cases:
    // 1. declaration before definition
    // 2. 2nd declaration before definition
    // 3. definition
    // 4. declaration after definition

    // 1. should declare `id` a import unless specified as `static`.
    // 3. should always declare `id` as export or local.
    // 2. and 4. should be a no-op.
    fn declare_func(&mut self, symbol: Symbol, is_definition: bool) -> CompileResult<FuncId> {
        if !is_definition {
            // case 2 and 4
            if let Some(Id::Function(func_id)) = self.declarations.get(&symbol) {
                return Ok(*func_id);
            }
        }
        let metadata = symbol.get();
        let func_type = match &metadata.ctype {
            Type::Function(func_type) => func_type,
            _ => unreachable!("bug in backend: only functions should be passed to `declare_func`"),
        };
        let signature = func_type.signature(self.module.isa());
        let linkage = match metadata.storage_class {
            StorageClass::Auto | StorageClass::Extern if is_definition => Linkage::Export,
            StorageClass::Auto | StorageClass::Extern => Linkage::Import,
            StorageClass::Static => Linkage::Local,
            StorageClass::Register | StorageClass::Typedef => unreachable!(),
        };
        let strings = crate::parser::intern::STRINGS
            .read()
            .expect("failed to lock String cache for reading");
        let tmp = strings.resolve(&metadata.id.0);

        let func_id = self
            .module
            .declare_function(tmp, linkage, &signature)
            .unwrap_or_else(|err| panic!("{}", err));
        self.declarations.insert(symbol, Id::Function(func_id));
        Ok(func_id)
    }
    /// declare an object on the stack
    fn declare_stack(
        &mut self,
        decl: Declaration,
        location: Location,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        let meta = decl.symbol.get();
        if let StorageClass::Typedef = meta.storage_class {
            return Ok(());
        }
        if let Type::Function(_) = &meta.ctype {
            self.declare_func(decl.symbol, false)?;
            return Ok(());
        }
        let u64_size = match meta.ctype.sizeof() {
            Ok(size) => size,
            Err(err) => {
                return Err(CompileError::semantic(Locatable {
                    data: err.into(),
                    location,
                }));
            }
        };
        let kind = StackSlotKind::ExplicitSlot;
        let size = match u32::try_from(u64_size) {
            Ok(size) => size,
            Err(_) => {
                return Err(CompileError::semantic(Locatable {
                    data: "cannot store items on the stack that are more than 4 GB, it will overflow the stack".into(),
                    location,
                }));
            }
        };
        let data = StackSlotData::new(kind, size, 0);
        let stack_slot = builder.create_sized_stack_slot(data);
        self.declarations.insert(decl.symbol, Id::Local(stack_slot));
        if let Some(init) = decl.init {
            self.store_stack(init, stack_slot, builder)?;
        }
        Ok(())
    }
    fn store_stack(
        &mut self,
        init: Initializer,
        stack_slot: StackSlot,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        match init {
            Initializer::Scalar(expr) => {
                let val = self.compile_expr(*expr, builder)?;
                builder.ins().stack_store(val.ir_val, stack_slot, 0);
            }
            Initializer::List(_) => unimplemented!("aggregate dynamic initialization"),
            Initializer::FunctionBody(_) => unreachable!("functions can't be stored on the stack"),
        }
        Ok(())
    }
    // TODO: this is grossly inefficient, ask Cranelift devs if
    // there's an easier way to make parameters modifiable.
    fn store_stack_params(
        &mut self,
        params: &[Symbol],
        func_start: Block,
        location: &Location,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        // Cranelift requires that all block params are declared up front
        let ir_vals: Vec<_> = params
            .iter()
            .map(|param| {
                let ir_type = param.get().ctype.as_ir_type();
                Ok(builder.append_block_param(func_start, ir_type))
            })
            .collect::<CompileResult<_>>()?;
        for (&param, ir_val) in params.iter().zip(ir_vals) {
            let u64_size = match param.get().ctype.sizeof() {
                Err(data) => semantic_err!(data.into(), *location),
                Ok(size) => size,
            };
            let u32_size = match u32::try_from(u64_size) {
                Err(_) => semantic_err!(
                    format!(
                        "size {} is too large for stack (can only handle 32-bit values)",
                        u64_size
                    ),
                    *location
                ),
                Ok(size) => size,
            };
            let stack_data = StackSlotData::new(StackSlotKind::ExplicitSlot, u32_size, 0);
            let slot = builder.create_sized_stack_slot(stack_data);
            builder.ins().stack_store(ir_val, slot, 0);
            self.declarations.insert(param, Id::Local(slot));
        }
        Ok(())
    }
    fn compile_func(
        &mut self,
        symbol: Symbol,
        func_type: &FunctionType,
        stmts: Vec<Stmt>,
        location: Location,
    ) -> CompileResult<()> {
        let func_id = self.declare_func(symbol, true)?;
        // TODO: make declare_func should take a `signature` after all?
        // This just calculates it twice, it's probably fine
        let signature = func_type.signature(self.module.isa());

        // external name is meant to be a lookup in a symbol table,
        // but we just give it garbage values
        let user_func_name = UserFuncName::user(0, 0);
        let mut func = Function::with_name_signature(user_func_name, signature);

        // this context is just boiler plate
        let mut ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut ctx);

        let func_start = builder.create_block();
        builder.switch_to_block(func_start);
        self.current_block_state = BlockState::Empty;

        let should_ret = func_type.should_return();
        if func_type.has_params() {
            self.store_stack_params(
                // TODO: get rid of this clone
                &func_type.params,
                func_start,
                &location,
                &mut builder,
            )?;
        }

        self.compile_all(stmts, &mut builder)?;

        if self.current_block_state != BlockState::Filled {
            let id = symbol.get().id;
            if id == InternedStr::get_or_intern("main") {
                let ir_int = func_type.return_type.as_ir_type();
                let zero = [builder.ins().iconst(ir_int, 0)];
                builder.ins().return_(&zero);
            } else if should_ret {
                semantic_err!(
                    format!(
                        "expected a return statement before end of function '{}' returning {}",
                        id, func_type.return_type
                    ),
                    location
                );
            } else {
                // void function, return nothing
                builder.ins().return_(&[]);
            }
        }

        builder.seal_all_blocks();
        builder.finalize();

        let flags = settings::Flags::new(settings::builder());

        if self.debug {
            println!("ir: {}", func);
        }

        if let Err(err) = codegen::verify_function(&func, &flags) {
            panic!("verification error: {}\nnote: while compiling {}", err, func);
        }

        let mut ctx = codegen::Context::for_function(func);
        // let mut trap_sink = codegen::binemit::NullTrapSink {};
        if let Err(err) = self.module.define_function(func_id, &mut ctx) {
            panic!("definition error: {}\nnote: while compiling {}", err, ctx.func);
        }

        Ok(())
    }
}

pub type Product = cranelift_object::ObjectProduct;

/// Compile and return the declarations and warnings.
pub fn compile(module: ObjectModule, buf: &str, opt: Opt) -> Program<ObjectModule> {
    use crate::parser::check_semantics;
    use crate::vec_deque;

    let debug_asm = opt.debug_asm;
    let mut program = check_semantics(buf, opt);
    let hir = match program.result {
        Ok(hir) => hir,
        Err(err) => {
            return Program {
                result: Err(err),
                warnings: program.warnings,
                files: program.files,
            };
        }
    };
    // really we'd like to have all errors but that requires a refactor
    let mut err = None;
    let mut compiler = Compiler::new(module, debug_asm);
    for decl in hir {
        let meta = decl.data.symbol.get();
        if let StorageClass::Typedef = meta.storage_class {
            continue;
        }
        let current = match &meta.ctype {
            Type::Function(func_type) => match decl.data.init {
                Some(Initializer::FunctionBody(stmts)) => {
                    compiler.compile_func(decl.data.symbol, func_type, stmts, decl.location)
                }
                None => compiler.declare_func(decl.data.symbol, false).map(|_| ()),
                _ => unreachable!("functions can only be initialized by a FunctionBody"),
            },
            Type::Void | Type::Error => unreachable!("parser let an incomplete type through"),
            _ => {
                if let Some(Initializer::FunctionBody(_)) = &decl.data.init {
                    unreachable!("only functions should have a function body")
                }
                compiler.store_static(decl.data.symbol, decl.data.init, decl.location)
            }
        };
        if let Err(e) = current {
            err = Some(e);
            break;
        }
    }
    let warns = compiler.error_handler.warnings;
    let (result, ir_warnings) = match err {
        Some(err) => (Err(err), warns),
        _ => (Ok(compiler.module), warns),
    };
    program.warnings.extend(ir_warnings);
    Program {
        result: result.map_err(|errs| vec_deque![errs]),
        warnings: program.warnings,
        files: program.files,
    }
}

pub fn assemble(product: Product, output: &Path) -> Result<(), crate::parser::Error> {
    use std::fs::File;
    use std::io::{self, Write};

    let bytes = product.emit().map_err(crate::parser::Error::Platform)?;
    File::create(output)?.write_all(&bytes).map_err(io::Error::into)
}

pub fn link(obj_file: &Path, output: &Path) -> Result<(), std::io::Error> {
    use std::io::{Error, ErrorKind};
    use std::process::Command;

    // link the .o file using host linker
    let status = Command::new("cc")
        .args([obj_file, Path::new("-o"), output])
        .status()
        .map_err(|err| {
            if err.kind() == ErrorKind::NotFound {
                Error::new(
                    ErrorKind::NotFound,
                    "could not find host cc (for linking). Is it on your PATH?",
                )
            } else {
                err
            }
        })?;
    if !status.success() {
        Err(Error::new(ErrorKind::Other, "linking program failed"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn test_compile_error_semantic() {
    assert_eq!(
        CompileError::semantic(Location::default().with("".to_string())).data,
        Error::Semantic(SemanticError::Generic("".to_string())),
    );
}
