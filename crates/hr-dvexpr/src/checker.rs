use crate::ast::{BinOp, ContextRoot, Expr, Literal, UnOp};
use crate::error::{Error, Result};
use crate::types::{NumKind, Type};
use std::collections::HashMap;

/// Resolves identifier paths to their declared types.
///
/// `this.<col>` (or bare `<col>`) refers to a column of the current row.
/// `<rel>.<col>` (or `<rel>.<rel>.<col>`, max depth 2) refers to a column reachable through a
/// declared lookup relation. Returning `None` means the path is unknown.
pub trait ColumnSchema {
    fn lookup(&self, path: &[String]) -> Option<Type>;
}

/// Convenience in-memory implementation backed by a flat map. Keys MUST start with the root
/// (e.g., `["this", "qty"]` or `["customer", "name"]`).
#[derive(Debug, Default, Clone)]
pub struct MapSchema {
    map: HashMap<Vec<String>, Type>,
}

impl MapSchema {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, path: &[&str], ty: Type) -> Self {
        self.map
            .insert(path.iter().map(|s| s.to_string()).collect(), ty);
        self
    }
}

impl ColumnSchema for MapSchema {
    fn lookup(&self, path: &[String]) -> Option<Type> {
        self.map.get(path).copied()
    }
}

/// A typed AST: same shape as `Expr` but every node carries its inferred `Type`.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedExpr {
    Literal {
        lit: Literal,
        ty: Type,
    },
    Path {
        path: Vec<String>,
        ty: Type,
    },
    Context {
        root: ContextRoot,
        ty: Type,
    },
    Binary {
        op: BinOp,
        lhs: Box<TypedExpr>,
        rhs: Box<TypedExpr>,
        ty: Type,
    },
    Unary {
        op: UnOp,
        rhs: Box<TypedExpr>,
        ty: Type,
    },
    Call {
        name: String,
        args: Vec<TypedExpr>,
        ty: Type,
    },
    If {
        cond: Box<TypedExpr>,
        then_: Box<TypedExpr>,
        else_: Box<TypedExpr>,
        ty: Type,
    },
}

impl TypedExpr {
    pub fn ty(&self) -> Type {
        match self {
            TypedExpr::Literal { ty, .. }
            | TypedExpr::Path { ty, .. }
            | TypedExpr::Context { ty, .. }
            | TypedExpr::Binary { ty, .. }
            | TypedExpr::Unary { ty, .. }
            | TypedExpr::Call { ty, .. }
            | TypedExpr::If { ty, .. } => *ty,
        }
    }
}

/// Purity classification:
/// - `Pure` ⇒ depends only on `this.*` columns; safe to compile to a Postgres GENERATED column.
/// - `Runtime` ⇒ refs to other-row paths, `now()`, `today()`, `user()`, `app()`. Must be evaluated
///   at read-time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Purity {
    Pure,
    Runtime,
}

use serde::{Deserialize, Serialize};

pub struct Checker<'a> {
    schema: &'a dyn ColumnSchema,
    purity: Purity,
}

impl<'a> Checker<'a> {
    pub fn new(schema: &'a dyn ColumnSchema) -> Self {
        Self {
            schema,
            purity: Purity::Pure,
        }
    }

    pub fn purity(&self) -> Purity {
        self.purity
    }

    fn taint(&mut self) {
        self.purity = Purity::Runtime;
    }

    pub fn check(&mut self, expr: &Expr) -> Result<TypedExpr> {
        match expr {
            Expr::Literal(lit) => {
                let ty = match lit {
                    Literal::Int(_) => Type::Number(NumKind::Int),
                    Literal::Float(_) => Type::Number(NumKind::Float),
                    Literal::Text(_) => Type::Text,
                    Literal::Bool(_) => Type::Bool,
                    Literal::Null => Type::Null,
                };
                Ok(TypedExpr::Literal {
                    lit: lit.clone(),
                    ty,
                })
            }

            Expr::Path(path) => {
                // Sugar: bare `col` → `this.col`.
                let canonical: Vec<String> = if path.first().map(String::as_str) == Some("this") {
                    path.clone()
                } else if path.len() == 1 {
                    let mut v = Vec::with_capacity(2);
                    v.push("this".to_string());
                    v.push(path[0].clone());
                    v
                } else {
                    // Cross-row reference (e.g., customer.name) → impure.
                    self.taint();
                    path.clone()
                };

                let ty = self.schema.lookup(&canonical).ok_or_else(|| {
                    Error::UnknownIdent(canonical.join("."))
                })?;
                Ok(TypedExpr::Path { path: canonical, ty })
            }

            Expr::Context(root) => {
                self.taint();
                let ty = match root {
                    ContextRoot::Now => Type::Timestamp,
                    ContextRoot::Today => Type::Date,
                    ContextRoot::User => Type::Uuid,
                    ContextRoot::App => Type::Uuid,
                };
                Ok(TypedExpr::Context { root: *root, ty })
            }

            Expr::Binary { op, lhs, rhs } => {
                let l = self.check(lhs)?;
                let r = self.check(rhs)?;
                let ty = check_binary(*op, l.ty(), r.ty())?;
                Ok(TypedExpr::Binary {
                    op: *op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                    ty,
                })
            }

            Expr::Unary { op, rhs } => {
                let r = self.check(rhs)?;
                let ty = match op {
                    UnOp::Neg => {
                        if !r.ty().is_number() && r.ty() != Type::Null {
                            return Err(Error::Type(format!(
                                "unary `-` requires Number, got {}",
                                r.ty()
                            )));
                        }
                        r.ty()
                    }
                    UnOp::Not => {
                        if r.ty() != Type::Bool && r.ty() != Type::Null {
                            return Err(Error::Type(format!(
                                "unary `!` requires Bool, got {}",
                                r.ty()
                            )));
                        }
                        Type::Bool
                    }
                };
                Ok(TypedExpr::Unary {
                    op: *op,
                    rhs: Box::new(r),
                    ty,
                })
            }

            Expr::Call { name, args } => self.check_call(name, args),

            Expr::If { cond, then_, else_ } => {
                let c = self.check(cond)?;
                let t = self.check(then_)?;
                let e = self.check(else_)?;
                if c.ty() != Type::Bool && c.ty() != Type::Null {
                    return Err(Error::Type(format!(
                        "IF condition must be Bool, got {}",
                        c.ty()
                    )));
                }
                let ty = Type::unify(t.ty(), e.ty()).ok_or_else(|| {
                    Error::Type(format!(
                        "IF branches have incompatible types: {} vs {}",
                        t.ty(),
                        e.ty()
                    ))
                })?;
                Ok(TypedExpr::If {
                    cond: Box::new(c),
                    then_: Box::new(t),
                    else_: Box::new(e),
                    ty,
                })
            }
        }
    }

    fn check_call(&mut self, name: &str, args: &[Expr]) -> Result<TypedExpr> {
        let typed_args: Vec<TypedExpr> = args
            .iter()
            .map(|a| self.check(a))
            .collect::<Result<_>>()?;

        let arg_tys: Vec<Type> = typed_args.iter().map(|a| a.ty()).collect();
        let upper = name.to_ascii_uppercase();
        let ty = match upper.as_str() {
            "ABS" => {
                expect_arity(name, 1, args.len())?;
                expect_number(name, &arg_tys[0])?;
                arg_tys[0]
            }
            "ROUND" => {
                if args.len() != 1 && args.len() != 2 {
                    return Err(Error::Arity {
                        name: name.into(),
                        expected: "1 or 2".into(),
                        got: args.len(),
                    });
                }
                expect_number(name, &arg_tys[0])?;
                if args.len() == 2 {
                    expect_int(name, &arg_tys[1])?;
                }
                Type::Number(NumKind::Float)
            }
            "FLOOR" | "CEIL" => {
                expect_arity(name, 1, args.len())?;
                expect_number(name, &arg_tys[0])?;
                Type::Number(NumKind::Int)
            }
            "COALESCE" => {
                if args.is_empty() {
                    return Err(Error::Arity {
                        name: name.into(),
                        expected: "≥1".into(),
                        got: 0,
                    });
                }
                let mut acc = arg_tys[0];
                for t in &arg_tys[1..] {
                    acc = Type::unify(acc, *t).ok_or_else(|| {
                        Error::Type(format!(
                            "COALESCE arguments have incompatible types: {} vs {}",
                            acc, t
                        ))
                    })?;
                }
                acc
            }
            "LEN" => {
                expect_arity(name, 1, args.len())?;
                expect_text(name, &arg_tys[0])?;
                Type::Number(NumKind::Int)
            }
            "UPPER" | "LOWER" => {
                expect_arity(name, 1, args.len())?;
                expect_text(name, &arg_tys[0])?;
                Type::Text
            }
            "CONCAT" => {
                if args.is_empty() {
                    return Err(Error::Arity {
                        name: name.into(),
                        expected: "≥1".into(),
                        got: 0,
                    });
                }
                for t in &arg_tys {
                    expect_text(name, t)?;
                }
                Type::Text
            }
            "SUBSTR" => {
                if args.len() != 2 && args.len() != 3 {
                    return Err(Error::Arity {
                        name: name.into(),
                        expected: "2 or 3".into(),
                        got: args.len(),
                    });
                }
                expect_text(name, &arg_tys[0])?;
                expect_int(name, &arg_tys[1])?;
                if args.len() == 3 {
                    expect_int(name, &arg_tys[2])?;
                }
                Type::Text
            }
            "IF" => {
                expect_arity(name, 3, args.len())?;
                if arg_tys[0] != Type::Bool && arg_tys[0] != Type::Null {
                    return Err(Error::Type(format!(
                        "IF condition must be Bool, got {}",
                        arg_tys[0]
                    )));
                }
                Type::unify(arg_tys[1], arg_tys[2]).ok_or_else(|| {
                    Error::Type(format!(
                        "IF branches have incompatible types: {} vs {}",
                        arg_tys[1], arg_tys[2]
                    ))
                })?
            }
            // ── Time literal constructors ─────────────────────────────
            // `parse_timestamp('2026-05-06T15:00:00Z')` yields a Timestamp
            // value, enabling comparisons like `updated_at > parse_timestamp(...)`.
            // dvexpr has no native timestamp/date literal grammar; these
            // helpers fill that gap. Validation of the string format
            // happens at SQL emit time (PG cast `::timestamptz` rejects
            // malformed input).
            "PARSE_TIMESTAMP" => {
                expect_arity(name, 1, args.len())?;
                expect_text(name, &arg_tys[0])?;
                Type::Timestamp
            }
            "PARSE_DATE" => {
                expect_arity(name, 1, args.len())?;
                expect_text(name, &arg_tys[0])?;
                Type::Date
            }
            _ => return Err(Error::UnknownFunction(name.into())),
        };

        Ok(TypedExpr::Call {
            name: upper,
            args: typed_args,
            ty,
        })
    }
}

fn expect_arity(name: &str, expected: usize, got: usize) -> Result<()> {
    if expected != got {
        return Err(Error::Arity {
            name: name.into(),
            expected: expected.to_string(),
            got,
        });
    }
    Ok(())
}

fn expect_number(name: &str, ty: &Type) -> Result<()> {
    if !ty.is_number() && *ty != Type::Null {
        return Err(Error::Type(format!(
            "{} requires Number, got {}",
            name, ty
        )));
    }
    Ok(())
}

fn expect_int(name: &str, ty: &Type) -> Result<()> {
    if *ty != Type::Number(NumKind::Int) && *ty != Type::Null {
        return Err(Error::Type(format!(
            "{} requires Int, got {}",
            name, ty
        )));
    }
    Ok(())
}

fn expect_text(name: &str, ty: &Type) -> Result<()> {
    if *ty != Type::Text && *ty != Type::Null {
        return Err(Error::Type(format!(
            "{} requires Text, got {}",
            name, ty
        )));
    }
    Ok(())
}

fn check_binary(op: BinOp, l: Type, r: Type) -> Result<Type> {
    use BinOp::*;
    if op.is_arithmetic() {
        if op == Add && l == Type::Text && r == Type::Text {
            // No string `+`; force CONCAT for clarity.
            return Err(Error::Type(
                "use CONCAT(a, b) for string concatenation, not `+`".into(),
            ));
        }
        if !l.is_number() && l != Type::Null {
            return Err(Error::Type(format!(
                "arithmetic `{:?}` requires Number lhs, got {}",
                op, l
            )));
        }
        if !r.is_number() && r != Type::Null {
            return Err(Error::Type(format!(
                "arithmetic `{:?}` requires Number rhs, got {}",
                op, r
            )));
        }
        // / always promotes to Float; others use unification.
        let unified = Type::unify(l, r).unwrap_or(Type::Number(NumKind::Float));
        if op == Div {
            return Ok(Type::Number(NumKind::Float));
        }
        return Ok(unified);
    }

    if op.is_comparison() {
        // Equality is allowed across any compatible types; ordered comparisons need a sensible
        // ordering (numbers, text, dates, timestamps).
        if op == Eq || op == Ne {
            // Allow any pair where unify succeeds OR either side is Null.
            if l == Type::Null || r == Type::Null || Type::unify(l, r).is_some() {
                return Ok(Type::Bool);
            }
            return Err(Error::Type(format!(
                "cannot compare {} to {} for equality",
                l, r
            )));
        }
        // ordered
        let ok = matches!(
            (l, r),
            (Type::Number(_), Type::Number(_))
                | (Type::Text, Type::Text)
                | (Type::Date, Type::Date)
                | (Type::Timestamp, Type::Timestamp)
                | (Type::Date, Type::Timestamp)
                | (Type::Timestamp, Type::Date)
                | (Type::Null, _)
                | (_, Type::Null)
        );
        if !ok {
            return Err(Error::Type(format!(
                "ordered comparison `{:?}` not defined for {} and {}",
                op, l, r
            )));
        }
        return Ok(Type::Bool);
    }

    if op.is_logical() {
        if (l != Type::Bool && l != Type::Null) || (r != Type::Bool && r != Type::Null) {
            return Err(Error::Type(format!(
                "logical `{:?}` requires Bool operands, got {} and {}",
                op, l, r
            )));
        }
        return Ok(Type::Bool);
    }

    if op == Coalesce {
        return Type::unify(l, r).ok_or_else(|| {
            Error::Type(format!(
                "?? operands have incompatible types: {} vs {}",
                l, r
            ))
        });
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn schema() -> MapSchema {
        MapSchema::new()
            .with(&["this", "qty"], Type::Number(NumKind::Int))
            .with(&["this", "price"], Type::Number(NumKind::Float))
            .with(&["this", "name"], Type::Text)
            .with(&["this", "due_at"], Type::Timestamp)
            .with(&["this", "active"], Type::Bool)
            .with(&["customer", "name"], Type::Text)
    }

    fn check_str(s: &str) -> Result<(TypedExpr, Purity)> {
        let toks = tokenize(s)?;
        let ast = parse(&toks)?;
        let sch = schema();
        let mut c = Checker::new(&sch);
        let typed = c.check(&ast)?;
        Ok((typed, c.purity()))
    }

    #[test]
    fn pure_arithmetic() {
        let (te, p) = check_str("qty * price").unwrap();
        assert_eq!(te.ty(), Type::Number(NumKind::Float));
        assert_eq!(p, Purity::Pure);
    }

    #[test]
    fn impure_via_now() {
        let (te, p) = check_str("if(due_at < now, 'late', 'ok')").unwrap();
        assert_eq!(te.ty(), Type::Text);
        assert_eq!(p, Purity::Runtime);
    }

    #[test]
    fn impure_via_lookup() {
        let (_, p) = check_str("customer.name").unwrap();
        assert_eq!(p, Purity::Runtime);
    }

    #[test]
    fn type_error_text_plus_text() {
        let err = check_str("name + name").unwrap_err();
        assert!(matches!(err, Error::Type(_)));
    }

    #[test]
    fn type_error_qty_plus_name() {
        let err = check_str("qty + name").unwrap_err();
        assert!(matches!(err, Error::Type(_)));
    }

    #[test]
    fn coalesce() {
        let (te, _) = check_str("qty ?? 0").unwrap();
        assert_eq!(te.ty(), Type::Number(NumKind::Int));
    }

    #[test]
    fn unknown_ident() {
        let err = check_str("nope").unwrap_err();
        assert!(matches!(err, Error::UnknownIdent(_)));
    }

    #[test]
    fn unknown_function() {
        let err = check_str("BOGUS(qty)").unwrap_err();
        assert!(matches!(err, Error::UnknownFunction(_)));
    }
}
