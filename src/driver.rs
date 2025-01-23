use std::{fmt::Write, path::PathBuf};

use crate::pp;

pub fn preprocess_v1(buf: &str, filename: PathBuf) -> String {
    let headers = pp::Headers::with_search_paths(vec![]);
    let builtin_defines = {
        pp::SourceFile {
            path: PathBuf::new(),
            phase3_group: pp::Tokenizer::new("").group(),
        }
    };
    let builtin_defines = {
        let mut phase4 = pp::Expander::new(&builtin_defines, &headers);
        phase4.expand();
        phase4
    };

    // TODO: change to use proper error handling, for now we just return an empty string
    //       because other code will handle it
    let src = pp::SourceFile::load_from_string(filename, buf);
    let mut temp_buffer = String::new();
    for tok in pp::Expander::with_defines_from(&src, &builtin_defines).expand() {
        write!(&mut temp_buffer, "{}", tok).expect("failed to write to temp buffer");
    }
    temp_buffer
}
