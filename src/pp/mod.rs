/// This module handle 4 phases of c translation pipeline

/// phase 1 - Source File
/// phase 2 - Line Joiner
/// phase 3 - Tokenizer
/// phase 4 - Expander

mod eat;
mod line_joiner;
mod token;
mod tokenizer;
mod expander;
mod headers;
mod sources;

pub use eat::Eat;
pub use line_joiner::LineJoiner;
pub use token::{Token, Ident};
pub use tokenizer::Tokenizer;
pub use expander::Expander;
pub use headers::{Headers};
pub use sources::SourceFile;