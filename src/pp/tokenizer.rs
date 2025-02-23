use crate::pp::{Eat, Ident, LineJoiner, Token};
use std::fmt;

// FIXME(eddyb) avoid using `Eat` with a `Tok` iterator.
#[derive(Clone)]
pub struct Tokenizer<'a> {
    phase2: LineJoiner<'a>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(s: &'a str) -> Self {
        Tokenizer {
            phase2: LineJoiner::new(s),
        }
    }
}

impl Iterator for Tokenizer<'_> {
    type Item = Token;
    fn next(&mut self) -> Option<Token> {
        let mut eat_whitespace = || {
            self.phase2
                .try_eat(|phase2| {
                    match phase2.next()? {
                        ' ' | '\t' => {}

                        '/' if phase2.eat('/') => while phase2.eat_if(|&c| c != '\n').is_some() {},
                        '/' if phase2.eat('*') => {
                            while let Some(c) = phase2.next() {
                                if c == '*' && phase2.eat('/') {
                                    break;
                                }
                            }
                        }
                        _ => return None,
                    };
                    Some(())
                })
                .is_some()
        };

        if eat_whitespace() {
            // Collapse multiple `Token::Whitespace` into one token.
            while eat_whitespace() {}

            return Some(Token::Whitespace);
        }

        let physical_line = self.phase2.physical_line;
        let c = self.phase2.next()?;
        Some(match c {
            '\n' => Token::Newline,

            '{' | '}' | '[' | ']' | '#' | '(' | ')' | '<' | '>' | '%' | ':' | ';' | '?' | '*' | '+' | '-' | '/'
            | '^' | '&' | '|' | '~' | '!' | '=' | ',' => Token::Punct(c),

            '_' | 'a'..='z' | 'A'..='Z' => {
                let mut ident = String::new();
                ident.push(c);
                while let Some(c) = self
                    .phase2
                    .eat_if(|c| matches!(c, '_' | '0'..='9' | 'a'..='z' | 'A'..='Z'))
                {
                    ident.push(c);
                }
                Token::Ident(Ident {
                    string: ident,
                    physical_line,
                })
            }

            '.' | '0'..='9' => {
                let (dot, digit) = if c == '.' {
                    (Some(c), self.phase2.eat_if(|c| c.is_ascii_digit()))
                } else {
                    (None, Some(c))
                };
                if let (Some('.'), None) = (dot, digit) {
                    Token::Punct('.')
                } else {
                    let mut lit = String::new();
                    lit.extend(dot);
                    lit.extend(digit);
                    while let Some((c, c2)) = self.phase2.try_eat(|line_joiner| {
                        let c = line_joiner.next()?;
                        Some(match c {
                            'e' | 'E' | 'p' | 'P' if line_joiner.eat('-') => (c, Some('-')),
                            'e' | 'E' | 'p' | 'P' if line_joiner.eat('+') => (c, Some('+')),

                            '.' | '0'..='9' | 'a'..='z' | 'A'..='Z' => (c, None),

                            '\'' => {
                                let c2 = line_joiner.next()?;
                                if !matches!(c2, '0'..='9' | 'a'..='z' | 'A'..='Z') {
                                    return None;
                                }
                                (c, Some(c2))
                            }

                            _ => return None,
                        })
                    }) {
                        lit.push(c);
                        lit.extend(c2);
                    }
                    Token::Literal(lit)
                }
            }

            // FIXME(eddyb) implement raw string literal support.
            '\'' | '"' => {
                let quote = c;
                let mut lit = String::new();
                lit.push(quote);
                while let Some(c) = self.phase2.next() {
                    lit.push(c);
                    if c == quote {
                        break;
                    }
                    if c == '\\' {
                        lit.extend(self.phase2.next());
                    }
                }
                Token::Literal(lit)
            }

            _ => Token::Error(c),
        })
    }
}

// FIXME(eddyb) avoid cloning, build ropes, etc.
#[derive(Clone)]
pub struct Group {
    pub parts: Vec<GroupPart>,
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for part in &self.parts {
            write!(f, "{}", part)?;
        }
        Ok(())
    }
}

// FIXME(eddyb) avoid cloning, build ropes, etc.
#[derive(Clone)]
pub enum GroupPart {
    Verbatim(Vec<Token>),
    Directive {
        maybe_name: Option<Ident>,
        tokens: Vec<Token>,
    },
    IfElse {
        cond: Vec<Token>,
        then: Group,
        else_: Group,
    },
}

impl fmt::Display for GroupPart {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GroupPart::Verbatim(tokens) => {
                for tok in tokens {
                    write!(f, "{}", tok)?;
                }
                Ok(())
            }
            GroupPart::Directive { maybe_name, tokens } => {
                write!(f, "#")?;
                if let Some(name) = maybe_name {
                    f.write_str(name)?;
                }
                write!(f, " ")?;
                for tok in tokens {
                    write!(f, "{}", tok)?;
                }
                writeln!(f)
            }
            GroupPart::IfElse { cond, then, else_ } => {
                write!(f, "#if ")?;
                for tok in cond {
                    write!(f, "{}", tok)?;
                }
                writeln!(f)?;
                write!(f, "{}", then)?;
                if !else_.parts.is_empty() {
                    writeln!(f, "#else")?;
                    write!(f, "{}", else_)?;
                }
                writeln!(f, "#endif")
            }
        }
    }
}

impl Tokenizer<'_> {
    fn then_else_groups(&mut self) -> (Group, Group) {
        let mut then = Group { parts: vec![] };
        let mut else_ = Group { parts: vec![] };
        while let Some(part) = self.group_part() {
            if let GroupPart::Directive {
                maybe_name: Some(name),
                tokens,
            } = &part
            {
                if name == "endif" && tokens.is_empty() {
                    break;
                }
                if name == "else" && tokens.is_empty() {
                    while let Some(part) = self.group_part() {
                        if let GroupPart::Directive {
                            maybe_name: Some(name),
                            tokens,
                        } = &part
                        {
                            if name == "endif" && tokens.is_empty() {
                                break;
                            }
                        }

                        else_.parts.push(part);
                    }
                    break;
                }
                if name == "elif" {
                    let (elif_then, elif_else) = self.then_else_groups();
                    else_.parts.push(GroupPart::IfElse {
                        // FIXME(eddyb) remove clone.
                        cond: tokens.clone(),
                        then: elif_then,
                        else_: elif_else,
                    });
                    break;
                }
            }

            then.parts.push(part);
        }

        (then, else_)
    }

    pub fn group_part(&mut self) -> Option<GroupPart> {
        if self.eat(Token::Punct('#')) {
            self.eat(Token::Whitespace);

            let maybe_name = match self.eat_if(|tok| matches!(tok, Token::Ident(_))) {
                Some(Token::Ident(name)) => Some(name),
                _ => None,
            };

            self.eat(Token::Whitespace);

            let mut tokens = vec![];

            for tok in self.by_ref() {
                if let Token::Newline = tok {
                    break;
                }
                tokens.push(tok);
            }
            if let Some(Token::Whitespace) = tokens.last() {
                tokens.pop();
            }

            if let Some(name) = &maybe_name {
                if let "if" | "ifdef" | "ifndef" = &name[..] {
                    let mut cond = vec![];
                    if let "ifdef" | "ifndef" = &name[..] {
                        if name == "ifndef" {
                            cond.push(Token::Punct('!'));
                        }
                        cond.push(Token::Ident(Ident {
                            string: "defined".to_string(),
                            physical_line: name.physical_line,
                        }));
                        cond.push(Token::Whitespace);
                    }
                    cond.extend(tokens);

                    let (then, else_) = self.then_else_groups();

                    return Some(GroupPart::IfElse { cond, then, else_ });
                }
            }

            return Some(GroupPart::Directive { maybe_name, tokens });
        }

        let mut verbatim_tokens = vec![];
        let mut directive_allowed = true;
        while let Some(tok) = self.next() {
            match tok {
                Token::Newline => directive_allowed = true,
                Token::Whitespace => {}
                _ => directive_allowed = false,
            }

            verbatim_tokens.push(tok);

            if directive_allowed && self.phase2.clone().next() == Some('#') {
                break;
            }
        }
        if verbatim_tokens.is_empty() {
            None
        } else {
            Some(GroupPart::Verbatim(verbatim_tokens))
        }
    }

    pub fn group(&mut self) -> Group {
        let mut parts = vec![];
        while let Some(part) = self.group_part() {
            parts.push(part);
        }
        Group { parts }
    }
}
