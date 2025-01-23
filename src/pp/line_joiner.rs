/// basically copying from https://github.com/eddyb/cprep/
use std::str::Chars;

use crate::pp::Eat;

/// Phase2 of translation
#[derive(Clone)]
pub struct LineJoiner<'a> {
    pub raw_chars: Chars<'a>,
    pub physical_line: usize,
}

impl<'a> LineJoiner<'a> {
    pub fn new(s: &'a str) -> Self {
        LineJoiner {
            raw_chars: s.chars(),
            physical_line: 1,
        }
    }
}

impl Iterator for LineJoiner<'_> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        loop {
            let c = self.raw_chars.next()?;
            if c == '\\' && self.raw_chars.eat('\n') {
                self.physical_line += 1;
                continue;
            }

            if c == '\n' {
                self.physical_line += 1;
            }
            return Some(c);
        }
    }
}
