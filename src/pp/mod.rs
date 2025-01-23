/// This module handle 4 phases of c translation pipeline
///
/// phase 1 - Source File
/// phase 2 - Line Joiner
/// phase 3 - Tokenizer
/// phase 4 - Expander
mod eat;
mod expander;
mod headers;
mod line_joiner;
mod sources;
mod token;
mod tokenizer;

pub use eat::Eat;
pub use expander::Expander;
pub use headers::Headers;
pub use line_joiner::LineJoiner;
pub use sources::SourceFile;
pub use token::{Ident, Token};
pub use tokenizer::Tokenizer;
