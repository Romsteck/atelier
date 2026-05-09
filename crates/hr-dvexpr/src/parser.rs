use crate::ast::{BinOp, ContextRoot, Expr, Literal, UnOp};
use crate::error::{Error, Result};
use crate::lexer::{Tok, Token};

pub fn parse(tokens: &[Token]) -> Result<Expr> {
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_or()?;
    if p.pos < tokens.len() {
        return Err(Error::Parse {
            offset: tokens[p.pos].offset,
            message: format!("unexpected token after expression: {:?}", tokens[p.pos].tok),
        });
    }
    Ok(expr)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn peek_offset(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|t| t.offset)
            .unwrap_or_else(|| self.tokens.last().map(|t| t.offset).unwrap_or(0))
    }

    fn bump(&mut self) -> Option<&'a Tok> {
        let t = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(&t.tok)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        match self.peek() {
            Some(cur) if cur == t => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn expect(&mut self, expected: &Tok) -> Result<()> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(Error::Parse {
                offset: self.peek_offset(),
                message: format!("expected {:?}, found {:?}", expected, self.peek()),
            })
        }
    }

    // expr := or
    // or   := and ("||" and)*
    fn parse_or(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_and()?;
        while self.eat(&Tok::OrOr) {
            let rhs = self.parse_and()?;
            lhs = Expr::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    // and := cmp ("&&" cmp)*
    fn parse_and(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_cmp()?;
        while self.eat(&Tok::AndAnd) {
            let rhs = self.parse_cmp()?;
            lhs = Expr::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    // cmp := add (cmp_op add)?
    fn parse_cmp(&mut self) -> Result<Expr> {
        let lhs = self.parse_add()?;
        let op = match self.peek() {
            Some(Tok::EqEq) => Some(BinOp::Eq),
            Some(Tok::NotEq) => Some(BinOp::Ne),
            Some(Tok::Lt) => Some(BinOp::Lt),
            Some(Tok::Le) => Some(BinOp::Le),
            Some(Tok::Gt) => Some(BinOp::Gt),
            Some(Tok::Ge) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let rhs = self.parse_add()?;
            return Ok(Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            });
        }
        Ok(lhs)
    }

    // add := mul (("+"|"-") mul)*
    fn parse_add(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_mul()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    // mul := unary (("*"|"/"|"%") unary)*
    fn parse_mul(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                Some(Tok::Percent) => BinOp::Mod,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_unary()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    // unary := ("-"|"!") unary | postfix
    fn parse_unary(&mut self) -> Result<Expr> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.pos += 1;
                let rhs = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    rhs: Box::new(rhs),
                })
            }
            Some(Tok::Bang) => {
                self.pos += 1;
                let rhs = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    rhs: Box::new(rhs),
                })
            }
            _ => self.parse_postfix(),
        }
    }

    // postfix := primary ("??" primary)*
    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_primary()?;
        while self.eat(&Tok::Coalesce) {
            let rhs = self.parse_primary()?;
            lhs = Expr::Binary {
                op: BinOp::Coalesce,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    // primary := number | string | bool | null | ident_chain | call | "(" expr ")"
    fn parse_primary(&mut self) -> Result<Expr> {
        let off = self.peek_offset();
        match self.peek() {
            Some(Tok::Int(v)) => {
                let v = *v;
                self.pos += 1;
                Ok(Expr::Literal(Literal::Int(v)))
            }
            Some(Tok::Float(v)) => {
                let v = *v;
                self.pos += 1;
                Ok(Expr::Literal(Literal::Float(v)))
            }
            Some(Tok::Text(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(Expr::Literal(Literal::Text(s)))
            }
            Some(Tok::Bool(b)) => {
                let b = *b;
                self.pos += 1;
                Ok(Expr::Literal(Literal::Bool(b)))
            }
            Some(Tok::Null) => {
                self.pos += 1;
                Ok(Expr::Literal(Literal::Null))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.parse_or()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                let name = name.clone();
                self.pos += 1;
                // Function call?
                if matches!(self.peek(), Some(Tok::LParen)) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.parse_or()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                    }
                    self.expect(&Tok::RParen)?;

                    // Reserved-form for `if(cond, then, else)` to give precise type errors.
                    if name.eq_ignore_ascii_case("if") && args.len() == 3 {
                        let mut it = args.into_iter();
                        let cond = it.next().unwrap();
                        let then_ = it.next().unwrap();
                        let else_ = it.next().unwrap();
                        return Ok(Expr::If {
                            cond: Box::new(cond),
                            then_: Box::new(then_),
                            else_: Box::new(else_),
                        });
                    }

                    // Context functions with parens: now(), today(), user(), app().
                    if let Some(root) = ContextRoot::from_name(&name) {
                        if !args.is_empty() {
                            return Err(Error::Arity {
                                name,
                                expected: "0".into(),
                                got: args.len(),
                            });
                        }
                        return Ok(Expr::Context(root));
                    }

                    return Ok(Expr::Call { name, args });
                }

                // Ident chain: `a.b.c` or context-root without parens (`now`, `today`, …).
                if matches!(self.peek(), Some(Tok::Dot)) {
                    let mut path = vec![name];
                    while self.eat(&Tok::Dot) {
                        match self.bump() {
                            Some(Tok::Ident(part)) => path.push(part.clone()),
                            other => {
                                return Err(Error::Parse {
                                    offset: off,
                                    message: format!(
                                        "expected identifier after `.`, found {:?}",
                                        other
                                    ),
                                });
                            }
                        }
                    }
                    return Ok(Expr::Path(path));
                }

                // Bare identifier — could be context root or current-row column ref (sugar for this.X).
                if let Some(root) = ContextRoot::from_name(&name) {
                    return Ok(Expr::Context(root));
                }
                Ok(Expr::Path(vec![name]))
            }
            other => Err(Error::Parse {
                offset: off,
                message: format!("expected expression, found {:?}", other),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn p(s: &str) -> Expr {
        let toks = tokenize(s).unwrap();
        parse(&toks).unwrap()
    }

    #[test]
    fn precedence_arith() {
        // 1 + 2 * 3 == 1 + (2 * 3)
        match p("1 + 2 * 3") {
            Expr::Binary { op: BinOp::Add, lhs, rhs } => {
                assert!(matches!(*lhs, Expr::Literal(Literal::Int(1))));
                match *rhs {
                    Expr::Binary { op: BinOp::Mul, .. } => {}
                    other => panic!("expected Mul rhs, got {:?}", other),
                }
            }
            other => panic!("expected Add at root, got {:?}", other),
        }
    }

    #[test]
    fn comparison_non_assoc() {
        // single comparison ok; chaining `a < b < c` is rejected (the second `<` has no LHS context).
        let _ = p("qty < 10");
        let toks = tokenize("a < b < c").unwrap();
        // It will parse `a < b` then try to consume `< c` outside parse_cmp — an error.
        assert!(parse(&toks).is_err());
    }

    #[test]
    fn coalesce_right_assoc_via_chain() {
        // a ?? b ?? c parses left-assoc ((a??b)??c) — that's fine semantically for `??`.
        let _ = p("a ?? b ?? c");
    }

    #[test]
    fn function_call_and_if() {
        match p("if(qty > 0, qty * price, 0)") {
            Expr::If { .. } => {}
            other => panic!("expected If, got {:?}", other),
        }
        match p("ROUND(price, 2)") {
            Expr::Call { name, args } => {
                assert_eq!(name, "ROUND");
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn path_chain() {
        match p("customer.address.city") {
            Expr::Path(p) => assert_eq!(p, vec!["customer", "address", "city"]),
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn context_no_parens() {
        match p("now") {
            Expr::Context(ContextRoot::Now) => {}
            other => panic!("expected Context::Now, got {:?}", other),
        }
        match p("user()") {
            Expr::Context(ContextRoot::User) => {}
            other => panic!("expected Context::User, got {:?}", other),
        }
    }

    #[test]
    fn unary() {
        let _ = p("-x");
        let _ = p("!flag");
        let _ = p("--3"); // (-(-3))
    }
}
