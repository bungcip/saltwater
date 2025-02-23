use std::fmt;

#[derive(Clone, Debug)]
pub struct Ident {
    pub string: String,
    pub physical_line: usize,
}

impl PartialEq for Ident {
    fn eq(&self, other: &Ident) -> bool {
        self[..] == other[..]
    }
}

impl std::ops::Deref for Ident {
    type Target = str;
    fn deref(&self) -> &str {
        &self.string
    }
}

impl PartialEq<str> for Ident {
    fn eq(&self, other: &str) -> bool {
        &self[..] == other
    }
}

impl PartialEq<&'_ str> for Ident {
    fn eq(&self, &other: &&str) -> bool {
        &self[..] == other
    }
}

/// Preprocessor token
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Newline,
    Whitespace,
    Punct(char),
    Ident(Ident),
    Literal(String),
    Error(char),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::Newline => f.write_str("\n"),
            Token::Whitespace => f.write_str(" "),
            Token::Punct(c) | Token::Error(c) => write!(f, "{}", c),
            Token::Ident(s) => f.write_str(s),
            Token::Literal(s) => f.write_str(s),
        }
    }
}
