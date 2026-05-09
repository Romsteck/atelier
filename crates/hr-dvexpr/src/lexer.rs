use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // Literals
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Null,
    // Identifiers
    Ident(String),
    // Punctuation
    LParen,
    RParen,
    Comma,
    Dot,
    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    Coalesce, // ??
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub offset: usize,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            '(' => {
                out.push(Token { tok: Tok::LParen, offset: start });
                i += 1;
            }
            ')' => {
                out.push(Token { tok: Tok::RParen, offset: start });
                i += 1;
            }
            ',' => {
                out.push(Token { tok: Tok::Comma, offset: start });
                i += 1;
            }
            '.' => {
                out.push(Token { tok: Tok::Dot, offset: start });
                i += 1;
            }
            '+' => {
                out.push(Token { tok: Tok::Plus, offset: start });
                i += 1;
            }
            '-' => {
                out.push(Token { tok: Tok::Minus, offset: start });
                i += 1;
            }
            '*' => {
                out.push(Token { tok: Tok::Star, offset: start });
                i += 1;
            }
            '/' => {
                out.push(Token { tok: Tok::Slash, offset: start });
                i += 1;
            }
            '%' => {
                out.push(Token { tok: Tok::Percent, offset: start });
                i += 1;
            }
            '=' => {
                if peek(bytes, i + 1) == Some('=') {
                    out.push(Token { tok: Tok::EqEq, offset: start });
                    i += 2;
                } else {
                    return Err(Error::Lex {
                        offset: start,
                        message: "expected `==`, found single `=`".into(),
                    });
                }
            }
            '!' => {
                if peek(bytes, i + 1) == Some('=') {
                    out.push(Token { tok: Tok::NotEq, offset: start });
                    i += 2;
                } else {
                    out.push(Token { tok: Tok::Bang, offset: start });
                    i += 1;
                }
            }
            '<' => {
                if peek(bytes, i + 1) == Some('=') {
                    out.push(Token { tok: Tok::Le, offset: start });
                    i += 2;
                } else {
                    out.push(Token { tok: Tok::Lt, offset: start });
                    i += 1;
                }
            }
            '>' => {
                if peek(bytes, i + 1) == Some('=') {
                    out.push(Token { tok: Tok::Ge, offset: start });
                    i += 2;
                } else {
                    out.push(Token { tok: Tok::Gt, offset: start });
                    i += 1;
                }
            }
            '&' => {
                if peek(bytes, i + 1) == Some('&') {
                    out.push(Token { tok: Tok::AndAnd, offset: start });
                    i += 2;
                } else {
                    return Err(Error::Lex {
                        offset: start,
                        message: "expected `&&`, found single `&`".into(),
                    });
                }
            }
            '|' => {
                if peek(bytes, i + 1) == Some('|') {
                    out.push(Token { tok: Tok::OrOr, offset: start });
                    i += 2;
                } else {
                    return Err(Error::Lex {
                        offset: start,
                        message: "expected `||`, found single `|`".into(),
                    });
                }
            }
            '?' => {
                if peek(bytes, i + 1) == Some('?') {
                    out.push(Token { tok: Tok::Coalesce, offset: start });
                    i += 2;
                } else {
                    return Err(Error::Lex {
                        offset: start,
                        message: "expected `??`, found single `?`".into(),
                    });
                }
            }
            '\'' => {
                let (s, end) = read_string(bytes, i)?;
                out.push(Token { tok: Tok::Text(s), offset: start });
                i = end;
            }
            '0'..='9' => {
                let (tok, end) = read_number(bytes, i)?;
                out.push(Token { tok, offset: start });
                i = end;
            }
            c if is_ident_start(c) => {
                let (name, end) = read_ident(bytes, i);
                let tok = match name.as_str() {
                    "true" => Tok::Bool(true),
                    "false" => Tok::Bool(false),
                    "null" => Tok::Null,
                    _ => Tok::Ident(name),
                };
                out.push(Token { tok, offset: start });
                i = end;
            }
            _ => {
                return Err(Error::Lex {
                    offset: start,
                    message: format!("unexpected character `{}`", c),
                });
            }
        }
    }
    Ok(out)
}

fn peek(bytes: &[u8], i: usize) -> Option<char> {
    bytes.get(i).map(|b| *b as char)
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn read_ident(bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start;
    while i < bytes.len() && is_ident_continue(bytes[i] as char) {
        i += 1;
    }
    let s = std::str::from_utf8(&bytes[start..i]).unwrap().to_string();
    (s, i)
}

fn read_number(bytes: &[u8], start: usize) -> Result<(Tok, usize)> {
    let mut i = start;
    let mut saw_dot = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit() {
            i += 1;
        } else if c == '.' && !saw_dot {
            // Lookahead: must be followed by a digit to be a fractional part (not a member access).
            if i + 1 < bytes.len() && (bytes[i + 1] as char).is_ascii_digit() {
                saw_dot = true;
                i += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    let s = std::str::from_utf8(&bytes[start..i]).unwrap();
    if saw_dot {
        let v: f64 = s.parse().map_err(|_| Error::Lex {
            offset: start,
            message: format!("invalid float `{}`", s),
        })?;
        Ok((Tok::Float(v), i))
    } else {
        let v: i64 = s.parse().map_err(|_| Error::Lex {
            offset: start,
            message: format!("invalid integer `{}`", s),
        })?;
        Ok((Tok::Int(v), i))
    }
}

fn read_string(bytes: &[u8], start: usize) -> Result<(String, usize)> {
    debug_assert_eq!(bytes[start] as char, '\'');
    let mut i = start + 1;
    let mut out = String::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '\'' {
            if peek(bytes, i + 1) == Some('\'') {
                // Escaped single-quote: ''
                out.push('\'');
                i += 2;
            } else {
                return Ok((out, i + 1));
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    Err(Error::Lex {
        offset: start,
        message: "unterminated string literal".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn ints_floats() {
        assert_eq!(toks("42"), vec![Tok::Int(42)]);
        assert_eq!(toks("3.14"), vec![Tok::Float(3.14)]);
        assert_eq!(toks("0"), vec![Tok::Int(0)]);
    }

    #[test]
    fn dot_after_int_is_member_access() {
        // `1.foo` would be an error/strange — but `this.qty` should lex cleanly,
        // and `1 + 2` should work. The key invariant: `.` not followed by a digit is the Dot tok.
        assert_eq!(
            toks("this.qty"),
            vec![Tok::Ident("this".into()), Tok::Dot, Tok::Ident("qty".into())]
        );
    }

    #[test]
    fn strings() {
        assert_eq!(toks("'hello'"), vec![Tok::Text("hello".into())]);
        assert_eq!(
            toks("'it''s'"),
            vec![Tok::Text("it's".into())]
        );
    }

    #[test]
    fn keywords() {
        assert_eq!(toks("true false null"), vec![Tok::Bool(true), Tok::Bool(false), Tok::Null]);
    }

    #[test]
    fn operators() {
        assert_eq!(
            toks("== != <= >= && || ?? !"),
            vec![
                Tok::EqEq,
                Tok::NotEq,
                Tok::Le,
                Tok::Ge,
                Tok::AndAnd,
                Tok::OrOr,
                Tok::Coalesce,
                Tok::Bang
            ]
        );
    }

    #[test]
    fn unterminated_string() {
        assert!(tokenize("'abc").is_err());
    }
}
