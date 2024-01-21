use cranelift::codegen::cursor::Cursor;
use cranelift::frontend::Switch;
use cranelift::prelude::{Block, FunctionBuilder, InstBuilder};
use cranelift_codegen::ir::Type;
use cranelift_codegen::ir::types::I32;


use super::{Compiler, BlockState};
use crate::saltwater_parser::data::{
    hir::{Expr, Stmt, StmtType},
    *,
};

impl Compiler {
    pub(super) fn compile_all(
        &mut self,
        stmts: Vec<Stmt>,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        for stmt in stmts {
            self.compile_stmt(stmt, builder)?;
        }
        Ok(())
    }

    pub(super) fn compile_stmt(&mut self,
        stmt: Stmt,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        
        if self.current_block_state == BlockState::Filled && !is_jump_target(&stmt.data) {
            return Err(stmt.location.error(SemanticError::UnreachableStatement));
        }
        
        match stmt.data {
            StmtType::Compound(stmts) => self.compile_all(stmts, builder),
            // INVARIANT: symbol has not yet been declared in this scope
            StmtType::Decl(decls) => {
                for decl in decls {
                    self.declare_stack(decl.data, decl.location, builder)?;
                }
                Ok(())
            }
            StmtType::Expr(expr) => {
                self.compile_expr(expr, builder)?;
                Ok(())
            }
            StmtType::Return(expr) => {
                let mut ret = vec![];
                if let Some(e) = expr {
                    let val = self.compile_expr(e, builder)?;
                    ret.push(val.ir_val);
                }
                builder.ins().return_(&ret);
                self.current_block_state = BlockState::Filled;
                Ok(())
            }
            StmtType::If(condition, body, otherwise) => {
                self.if_stmt(condition, *body, otherwise, builder)
            }
            StmtType::While(condition, body) => self.while_stmt(Some(condition), *body, builder),
            StmtType::Break | StmtType::Continue => {
                self.loop_exit(stmt.data == StmtType::Break, stmt.location, builder)
            }
            StmtType::For(init, condition, post_loop, body) => self.for_loop(
                *init,
                condition.map(|e| *e),
                post_loop.map(|e| *e),
                *body,
                stmt.location,
                builder,
            ),
            StmtType::Do(body, condition) => self.do_loop(*body, condition, builder),
            StmtType::Switch(condition, body) => self.switch(condition, *body, builder),
            StmtType::Label(name, inner) => {
                let new_block = builder.create_block();
                self.jump_to_block(new_block, builder);
                self.switch_to_block(new_block, builder);
                if let Some(previous) = self.labels.insert(name, new_block) {
                    Err(stmt
                        .location
                        .error(SemanticError::LabelRedeclaration(previous)))
                } else {
                    self.compile_stmt(*inner, builder)
                }
            }
            StmtType::Goto(name) => match self.labels.get(&name) {
                Some(block) => {
                    self.jump_to_block(*block, builder);
                    Ok(())
                }
                None => Err(stmt.location.error(SemanticError::UndeclaredLabel(name))),
            },
            StmtType::Case(constexpr, inner) => {
                self.case(constexpr.into(), *inner, stmt.location, builder)
            }
            StmtType::Default(inner) => self.default(*inner, stmt.location, builder),
        }

    }


    fn if_stmt(
        &mut self,
        condition: Expr,
        body: Stmt,
        otherwise: Option<Box<Stmt>>,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        // If condtion is zero:
        //      If else_block exists, jump to else_block + compile_all
        //      Otherwise, jump to end_block
        //  Otherwise:
        //      Fallthrough to if_body + compile_all
        //      If else_block exists, jump to end_block + compile_all
        //      Otherwise, fallthrough to end_block
        let condition = self.compile_expr(condition, builder)?;
        let (if_body, end_body) = (builder.create_block(), builder.create_block());
        if let Some(other) = otherwise {
            let else_body = builder.create_block();
            builder.ins().brif(condition.ir_val, if_body, &[], else_body, &[]);

            self.switch_to_block(if_body, builder);
            self.compile_stmt(body, builder)?;
            let if_has_return = self.current_block_state == BlockState::Filled;
            self.jump_to_block(end_body, builder);

            self.switch_to_block(else_body, builder);
            self.compile_stmt(*other, builder)?;
            if self.current_block_state != BlockState::Filled {
                self.jump_to_block(end_body, builder);
                self.switch_to_block(end_body, builder);
            // if we returned in both 'if' and 'else' blocks, all following code is unreachable
            // this is the case where we returned in 'else' but not 'if'
            } else if !if_has_return {
                self.switch_to_block(end_body, builder);
            }
        } else {
            builder.ins().brif(condition.ir_val, if_body, &[], end_body, &[]);

            self.switch_to_block(if_body, builder);
            self.compile_stmt(body, builder)?;
            self.jump_to_block(end_body, builder);

            self.switch_to_block(end_body, builder);
        };
        Ok(())
    }
    /// Enter a loop context:
    /// - Create a new start and end block
    /// - Switch to the start block
    /// - Return (start, end, previous_last_saw_loop)
    fn enter_loop(&mut self, builder: &mut FunctionBuilder) -> (Block, Block, Block, bool) {
        let (header_body, loop_body, end_body) = (builder.create_block(), builder.create_block(), builder.create_block());
        self.loops.push((loop_body, end_body));
        let old_saw_loop = self.last_saw_loop;
        self.last_saw_loop = true;

        // self.jump_to_block(loop_body, builder);
        builder.ins().jump(header_body, &[]);
        self.switch_to_block(header_body, builder);
        (header_body, loop_body, end_body, old_saw_loop)
    }
    /// Exit a loop
    fn exit_loop(&mut self, old_saw_loop: bool) {
        self.loops.pop();
        self.last_saw_loop = old_saw_loop;
    }
    fn while_stmt(
        &mut self,
        maybe_condition: Option<Expr>,
        body: Stmt,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        let (header_body, loop_body, end_body, old_saw_loop) = self.enter_loop(builder);

        // for loops can loop forever: `for (;;) {}`
        if let Some(condition) = maybe_condition {
            let condition = self.compile_expr(condition, builder)?;
            builder.ins().brif(condition.ir_val, loop_body, &[], end_body, &[]);
        }else{
            self.jump_to_block(loop_body, builder);
        }

        self.switch_to_block(loop_body, builder);
        self.compile_stmt(body, builder)?;
        self.jump_to_block(header_body, builder);


        self.switch_to_block(end_body, builder);
        self.exit_loop(old_saw_loop);

        Ok(())
    }

    // pub(super) fn fallthrough(&mut self, builder: &mut FunctionBuilder) {
    //     let bb = builder.create_block();
    //     self.jump_to_block(bb, builder);
    //     self.switch_to_block(bb, builder);
    // }

    fn do_loop(
        &mut self,
        body: Stmt,
        condition: Expr,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        let (header_body, loop_body, end_body, old_saw_loop) = self.enter_loop(builder);

        self.compile_stmt(body, builder)?;
        if self.current_block_state == BlockState::Filled {
            return Err(condition
                .location
                .error(SemanticError::UnreachableStatement));
        }
        self.jump_to_block(loop_body, builder);

        self.switch_to_block(loop_body, builder);
        let condition = self.compile_expr(condition, builder)?;
        builder.ins().brif(condition.ir_val, header_body, &[], end_body, &[]);

        self.switch_to_block(end_body, builder);
        self.exit_loop(old_saw_loop);

        Ok(())
    }
    fn for_loop(
        &mut self,
        init: Stmt,
        condition: Option<Expr>,
        post_loop: Option<Expr>,
        mut body: Stmt,
        location: Location,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        self.compile_stmt(init, builder)?;
        if let Some(post_loop) = post_loop {
            let post_loop = Stmt {
                data: StmtType::Expr(post_loop),
                location,
            };
            if let Stmt {
                data: StmtType::Compound(stmts),
                ..
            } = &mut body
            {
                stmts.push(post_loop);
            } else {
                body = Stmt {
                    data: StmtType::Compound(vec![body, post_loop]),
                    location,
                };
            };
        }
        self.while_stmt(condition, body, builder)
    }
    fn switch(
        &mut self,
        condition: Expr,
        body: Stmt,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        let cond_val = self.compile_expr(condition, builder)?;
        // works around https://github.com/CraneStation/cranelift/issues/1057
        // instead of switching to back to the current block to emit the Switch,
        // fill a new dummy block
        let dummy_block = builder.create_block();
        self.jump_to_block(dummy_block, builder);

        let start_block = builder.create_block();
        self.switch_to_block(start_block, builder);

        let old_saw_loop = self.last_saw_loop;
        self.last_saw_loop = false;

        self.switches
            .push((Switch::new(), None, builder.create_block()));
        self.compile_stmt(body, builder)?;
        let (switch, default, end) = self.switches.pop().unwrap();
        self.last_saw_loop = old_saw_loop;

        self.jump_to_block(end, builder);

        self.switch_to_block(dummy_block, builder);


        switch.emit(
            builder,
            cond_val.ir_val,
            if let Some(default) = default {
                default
            } else {
                end
            },
        );
        self.switch_to_block(end, builder);
        Ok(())
    }
    fn case(
        &mut self,
        constexpr: u128,
        stmt: Stmt,
        location: Location,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        let (switch, _, _) = match self.switches.last_mut() {
            Some(x) => x,
            None => {
                return Err(location.error(SemanticError::CaseOutsideSwitch { is_default: false }))
            }
        };

        if switch.entries().contains_key(&constexpr) {
            return Err(location.error(SemanticError::DuplicateCase { is_default: false }));
        }

        if self.current_block_state == BlockState::Empty {
            let current = builder.cursor().current_block().unwrap();
            switch.set_entry(constexpr, current);
        } else {
            let new = builder.create_block();
            switch.set_entry(constexpr, new);
            self.jump_to_block(new, builder);
            self.switch_to_block(new, builder);
        };
        self.compile_stmt(stmt, builder)
    }

    fn default(
        &mut self,
        inner: Stmt,
        location: Location,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {

        match self.switches.last() {
            Some((_, Some(_), _)) => {
                return Err(location.error(SemanticError::DuplicateCase { is_default: true }))
            },
            Some(_) => (),
            None => {
                return Err(location.error(SemanticError::CaseOutsideSwitch { is_default: true }));
            }
        };

        let default_block = if self.current_block_state == BlockState::Empty {
            builder.cursor().current_block().unwrap()
        } else {
            let new = builder.create_block();
            self.jump_to_block(new, builder);
            self.switch_to_block(new, builder);
            new
        };

        let (_, default, _) = self.switches.last_mut().unwrap();
        *default = Some(default_block);

        self.compile_stmt(inner, builder)
    }

    fn loop_exit(
        &mut self,
        is_break: bool,
        location: Location,
        builder: &mut FunctionBuilder,
    ) -> CompileResult<()> {
        if self.last_saw_loop {
            // break from loop
            if let Some((loop_start, loop_end)) = self.loops.last() {
                if is_break {
                    self.jump_to_block(*loop_end, builder);
                } else {
                    self.jump_to_block(*loop_start, builder);
                }
                Ok(())
            } else {
                semantic_err!(
                    format!(
                        "'{}' statement not in loop or switch statement",
                        if is_break { "break" } else { "continue" }
                    ),
                    location
                );
            }
        } else if !is_break {
            semantic_err!("'continue' not in loop".into(), location);
        } else {
            // break from switch
            let (_, _, end_block) = self
                .switches
                .last()
                .expect("should be in a switch if last_saw_loop is false");
            self.jump_to_block(*end_block, builder);
            Ok(())
        }
    }
    #[inline]
    fn jump_to_block(&mut self, block: Block, builder: &mut FunctionBuilder) {
        if self.current_block_state != BlockState::Filled {
            builder.ins().jump(block, &[]);
            self.current_block_state = BlockState::Filled;
        }
    }

    fn switch_to_block(&mut self, block: Block, builder: &mut FunctionBuilder) {
        builder.switch_to_block(block);
        self.current_block_state = BlockState::Empty;
    }

    // fn seal_block(&self, block: Block, builder: &mut FunctionBuilder) {
    //     builder.seal_block(block)
    // }

}

fn is_jump_target(stmt: &StmtType) -> bool {
    match stmt {
        StmtType::Case(_, _) | StmtType::Default(_) | StmtType::Label(_, _) => true,
        _ => false,
    }
}
