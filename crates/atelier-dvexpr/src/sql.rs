//! SQL emission for the dataverse expression language.
//!
//! Two modes:
//! - **Inline** — literals rendered directly into SQL, suitable for `GENERATED ALWAYS AS (...)
//!   STORED` column DDL where bind parameters are not allowed.
//! - **Parameterized** — literals pushed into a sink (`$1`, `$2`, …), suitable for `$filter`
//!   predicates baked into a `WHERE` clause.
//!
//! Only **pure** expressions (refs to `this.*` only, no `now`/`user`/lookups) can be emitted in
//! inline mode. Parameterized mode additionally accepts context references when the caller
//! supplies their resolved values (timestamp, user uuid, etc.).

use crate::ast::{BinOp, ContextRoot, Literal, UnOp};
use crate::checker::TypedExpr;
use crate::error::{Error, Result};
use crate::types::Type;

/// A parameter in a parameterized SQL emission.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Null,
    Timestamp(String), // ISO8601 — bound as TIMESTAMPTZ
    Date(String),
    Uuid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Inline literals, refuse context refs.
    Inline,
    /// Push literals to the param sink.
    Parameterized,
}

/// Resolved values for the impure context roots, used in parameterized mode.
#[derive(Debug, Clone, Default)]
pub struct ContextValues {
    pub now: Option<String>,    // RFC3339 timestamp
    pub today: Option<String>,  // ISO8601 date
    pub user: Option<String>,   // UUID
    pub app: Option<String>,    // UUID
}

/// Emits SQL for a typed expression.
pub struct SqlEmitter<'a> {
    pub mode: Mode,
    pub ctx: &'a ContextValues,
    /// Maps `this.<col>` refs to their column name in the target table. Defaults to the bare
    /// column name. Override to add a table prefix (e.g., `t.qty`).
    pub this_prefix: Option<&'a str>,
    pub params: Vec<Param>,
}

impl<'a> SqlEmitter<'a> {
    pub fn new_inline() -> Self {
        Self {
            mode: Mode::Inline,
            ctx: &EMPTY_CTX,
            this_prefix: None,
            params: Vec::new(),
        }
    }

    pub fn new_parameterized(ctx: &'a ContextValues) -> Self {
        Self {
            mode: Mode::Parameterized,
            ctx,
            this_prefix: None,
            params: Vec::new(),
        }
    }

    pub fn with_prefix(mut self, prefix: &'a str) -> Self {
        self.this_prefix = Some(prefix);
        self
    }

    /// Emits SQL. Returns the SQL string; `self.params` carries the bind values in
    /// parameterized mode.
    pub fn emit(&mut self, expr: &TypedExpr) -> Result<String> {
        match expr {
            TypedExpr::Literal { lit, .. } => self.emit_literal(lit),

            TypedExpr::Path { path, .. } => self.emit_path(path),

            TypedExpr::Context { root, .. } => self.emit_context(*root),

            TypedExpr::Binary { op, lhs, rhs, .. } => {
                // SQL idiom for NULL comparisons. `col = NULL` and `col <> NULL`
                // are both always NULL (never true) per ANSI SQL three-valued
                // logic, so the user-facing dvexpr `col == null` / `col != null`
                // would silently match nothing. We rewrite to `IS NULL` /
                // `IS NOT NULL`. This also avoids the `text = bigint` operator
                // mismatch when sqlx defaults `Param::Null` to bigint.
                if matches!(op, BinOp::Eq | BinOp::Ne) {
                    let lhs_is_null = matches!(lhs.as_ref(), TypedExpr::Literal { lit: Literal::Null, .. });
                    let rhs_is_null = matches!(rhs.as_ref(), TypedExpr::Literal { lit: Literal::Null, .. });
                    match (lhs_is_null, rhs_is_null) {
                        (false, true) => {
                            let l = self.emit(lhs)?;
                            let kw = if matches!(op, BinOp::Eq) { "IS NULL" } else { "IS NOT NULL" };
                            return Ok(format!("({}) {}", l, kw));
                        }
                        (true, false) => {
                            let r = self.emit(rhs)?;
                            let kw = if matches!(op, BinOp::Eq) { "IS NULL" } else { "IS NOT NULL" };
                            return Ok(format!("({}) {}", r, kw));
                        }
                        (true, true) => {
                            // null == null → constant TRUE, null != null → FALSE.
                            let v = matches!(op, BinOp::Eq);
                            return Ok(if v { "TRUE" } else { "FALSE" }.to_string());
                        }
                        (false, false) => {}
                    }
                }
                let l = self.emit(lhs)?;
                let r = self.emit(rhs)?;
                Ok(format_binary(*op, &l, &r))
            }

            TypedExpr::Unary { op, rhs, .. } => {
                let r = self.emit(rhs)?;
                Ok(match op {
                    UnOp::Neg => format!("(-({}))", r),
                    UnOp::Not => format!("(NOT ({}))", r),
                })
            }

            TypedExpr::Call { name, args, ty } => self.emit_call(name, args, *ty),

            TypedExpr::If { cond, then_, else_, .. } => {
                let c = self.emit(cond)?;
                let t = self.emit(then_)?;
                let e = self.emit(else_)?;
                Ok(format!("(CASE WHEN {} THEN {} ELSE {} END)", c, t, e))
            }
        }
    }

    fn emit_literal(&mut self, lit: &Literal) -> Result<String> {
        match self.mode {
            Mode::Inline => Ok(match lit {
                Literal::Int(v) => v.to_string(),
                Literal::Float(v) => format_float(*v),
                Literal::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
                Literal::Null => "NULL".to_string(),
                Literal::Text(s) => format!("'{}'", s.replace('\'', "''")),
            }),
            Mode::Parameterized => {
                let p = match lit {
                    Literal::Int(v) => Param::Int(*v),
                    Literal::Float(v) => Param::Float(*v),
                    Literal::Bool(b) => Param::Bool(*b),
                    Literal::Null => Param::Null,
                    Literal::Text(s) => Param::Text(s.clone()),
                };
                self.params.push(p);
                Ok(format!("${}", self.params.len()))
            }
        }
    }

    fn emit_path(&mut self, path: &[String]) -> Result<String> {
        // `this.col` — local column; cross-row paths only valid in Parameterized mode and only
        // after the read-time stitching has resolved the row.
        if path.first().map(String::as_str) == Some("this") && path.len() == 2 {
            let col = quote_ident(&path[1]);
            return Ok(match self.this_prefix {
                Some(p) => format!("{}.{}", p, col),
                None => col,
            });
        }
        // Cross-row → impure → must not reach SQL emitter.
        Err(Error::Eval(format!(
            "cross-row path `{}` cannot be lowered to SQL — evaluate at runtime",
            path.join(".")
        )))
    }

    fn emit_context(&mut self, root: ContextRoot) -> Result<String> {
        if self.mode == Mode::Inline {
            return Err(Error::Eval(format!(
                "context root `{}` not allowed in inline (GENERATED) mode",
                root.as_str()
            )));
        }
        let value = match root {
            ContextRoot::Now => self.ctx.now.as_ref().map(|s| Param::Timestamp(s.clone())),
            ContextRoot::Today => self.ctx.today.as_ref().map(|s| Param::Date(s.clone())),
            ContextRoot::User => self.ctx.user.as_ref().map(|s| Param::Uuid(s.clone())),
            ContextRoot::App => self.ctx.app.as_ref().map(|s| Param::Uuid(s.clone())),
        };
        let value = value.ok_or_else(|| {
            Error::Eval(format!("context root `{}` not provided", root.as_str()))
        })?;
        self.params.push(value);
        Ok(format!("${}", self.params.len()))
    }

    fn emit_call(&mut self, name: &str, args: &[TypedExpr], ty: Type) -> Result<String> {
        let _ = ty;
        let upper = name.to_ascii_uppercase();
        let parts: Vec<String> = args
            .iter()
            .map(|a| self.emit(a))
            .collect::<Result<_>>()?;
        Ok(match upper.as_str() {
            "ABS" => format!("ABS({})", parts[0]),
            "ROUND" => match parts.len() {
                1 => format!("ROUND(({})::numeric)::double precision", parts[0]),
                2 => format!("ROUND(({})::numeric, {})::double precision", parts[0], parts[1]),
                _ => unreachable!("checker enforces arity"),
            },
            "FLOOR" => format!("FLOOR({})::bigint", parts[0]),
            "CEIL" => format!("CEIL({})::bigint", parts[0]),
            "COALESCE" => format!("COALESCE({})", parts.join(", ")),
            "LEN" => format!("char_length({})", parts[0]),
            "UPPER" => format!("upper({})", parts[0]),
            "LOWER" => format!("lower({})", parts[0]),
            "CONCAT" => {
                if parts.len() == 1 {
                    parts.into_iter().next().unwrap()
                } else {
                    let joined = parts.join(" || ");
                    format!("({})", joined)
                }
            }
            "PARSE_TIMESTAMP" => format!("({})::timestamptz", parts[0]),
            "PARSE_DATE" => format!("({})::date", parts[0]),
            "SUBSTR" => match parts.len() {
                2 => format!("substr({}, {})", parts[0], parts[1]),
                3 => format!("substr({}, {}, {})", parts[0], parts[1], parts[2]),
                _ => unreachable!("checker enforces arity"),
            },
            "IF" => {
                // checker desugars to TypedExpr::If, but defensive fallback:
                format!(
                    "(CASE WHEN {} THEN {} ELSE {} END)",
                    parts[0], parts[1], parts[2]
                )
            }
            _ => return Err(Error::UnknownFunction(name.into())),
        })
    }
}

fn format_binary(op: BinOp, l: &str, r: &str) -> String {
    let sym = match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "=",
        BinOp::Ne => "<>",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "AND",
        BinOp::Or => "OR",
        BinOp::Coalesce => return format!("COALESCE({}, {})", l, r),
    };
    format!("({}) {} ({})", l, sym, r)
}

fn quote_ident(name: &str) -> String {
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

fn format_float(v: f64) -> String {
    if v.is_finite() {
        let s = format!("{}", v);
        if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{}.0", s)
        }
    } else if v.is_nan() {
        "'NaN'::double precision".to_string()
    } else if v.is_sign_positive() {
        "'Infinity'::double precision".to_string()
    } else {
        "'-Infinity'::double precision".to_string()
    }
}

const EMPTY_CTX: ContextValues = ContextValues {
    now: None,
    today: None,
    user: None,
    app: None,
};

// Sanity: ContextValues must be valid for static lifetime usage above.
const _: fn() = || {
    let _: &'static ContextValues = &EMPTY_CTX;
};

// SAFETY of `&EMPTY_CTX` borrow above: `EMPTY_CTX` is `'static`. We need a `&'a ContextValues`
// in `new_inline()`; since `'static` outlives any `'a`, the reference is valid. The shorter
// lifetime in the field is fine because we do not call any inline-context paths from
// `new_inline()`.
//
// The `_const _: fn()` above is a compile-time check that `EMPTY_CTX` has `'static` storage;
// nothing more.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::{Checker, MapSchema};
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::types::{NumKind, Type};

    fn schema() -> MapSchema {
        MapSchema::new()
            .with(&["this", "qty"], Type::Number(NumKind::Int))
            .with(&["this", "price"], Type::Number(NumKind::Float))
            .with(&["this", "name"], Type::Text)
    }

    fn typed(s: &str) -> TypedExpr {
        let toks = tokenize(s).unwrap();
        let ast = parse(&toks).unwrap();
        let sch = schema();
        let mut c = Checker::new(&sch);
        c.check(&ast).unwrap()
    }

    #[test]
    fn inline_arith() {
        let e = typed("qty * price + 1");
        let mut em = SqlEmitter::new_inline();
        let sql = em.emit(&e).unwrap();
        assert_eq!(sql, "((qty) * (price)) + (1)");
    }

    #[test]
    fn inline_if() {
        let e = typed("if(qty > 0, qty * price, 0)");
        let mut em = SqlEmitter::new_inline();
        let sql = em.emit(&e).unwrap();
        assert!(sql.contains("CASE WHEN"));
    }

    #[test]
    fn inline_concat_string_literal() {
        let e = typed("CONCAT('hello, ', name)");
        let mut em = SqlEmitter::new_inline();
        let sql = em.emit(&e).unwrap();
        assert!(sql.contains("'hello, '"));
        assert!(sql.contains("||"));
    }

    #[test]
    fn parameterized_filter() {
        let e = typed("qty > 10 && name == 'foo'");
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&e).unwrap();
        assert!(sql.contains("$1"));
        assert!(sql.contains("$2"));
        assert_eq!(em.params.len(), 2);
    }

    #[test]
    fn inline_rejects_context() {
        // due_at is timestamp; ref to `now` is impure
        let sch = MapSchema::new().with(&["this", "due_at"], Type::Timestamp);
        let toks = tokenize("due_at < now").unwrap();
        let ast = parse(&toks).unwrap();
        let mut c = Checker::new(&sch);
        let typed = c.check(&ast).unwrap();
        let mut em = SqlEmitter::new_inline();
        let err = em.emit(&typed).unwrap_err();
        assert!(matches!(err, Error::Eval(_)));
    }

    #[test]
    fn null_eq_lowers_to_is_null() {
        // `name == null` must become `name IS NULL` so it (a) actually
        // matches NULL rows (`= NULL` always yields NULL per ANSI three-
        // valued logic) and (b) avoids binding a typed Param::Null whose
        // sqlx default `Option::<i64>::None` triggers `text = bigint`
        // operator-not-found errors against text columns.
        let e = typed("name == null");
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&e).unwrap();
        assert!(sql.contains("IS NULL"), "expected IS NULL, got {}", sql);
        assert!(!sql.contains("="), "should not contain `=`, got {}", sql);
        assert_eq!(em.params.len(), 0, "no param should be pushed for null comparison");
    }

    #[test]
    fn null_neq_lowers_to_is_not_null() {
        let e = typed("name != null");
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&e).unwrap();
        assert!(sql.contains("IS NOT NULL"), "expected IS NOT NULL, got {}", sql);
        assert_eq!(em.params.len(), 0);
    }

    #[test]
    fn null_on_left_hand_side_also_lowers() {
        // The order shouldn't matter — `null == name` is equivalent to
        // `name == null`.
        let e = typed("null == name");
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&e).unwrap();
        assert!(sql.contains("IS NULL"), "expected IS NULL, got {}", sql);
        assert_eq!(em.params.len(), 0);
    }

    #[test]
    fn parse_timestamp_compiles_with_typed_comparison() {
        // `updated_at > parse_timestamp('2026-01-01T00:00:00Z')` was rejected
        // before — dvexpr has no native timestamp literal grammar, so the only
        // way to put a Timestamp on the right of `>` is via PARSE_TIMESTAMP.
        let sch = MapSchema::new()
            .with(&["this", "updated_at"], Type::Timestamp);
        let toks = tokenize("updated_at > parse_timestamp('2026-01-01T00:00:00Z')").unwrap();
        let ast = parse(&toks).unwrap();
        let mut c = Checker::new(&sch);
        let typed = c.check(&ast).unwrap();
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&typed).unwrap();
        assert!(sql.contains("::timestamptz"), "expected timestamptz cast, got: {}", sql);
        assert!(sql.contains("updated_at"));
        // The text literal becomes a bound param.
        assert_eq!(em.params.len(), 1);
        assert!(matches!(&em.params[0], Param::Text(s) if s == "2026-01-01T00:00:00Z"));
    }

    #[test]
    fn parse_date_yields_date_type() {
        let sch = MapSchema::new()
            .with(&["this", "due_day"], Type::Date);
        let toks = tokenize("due_day == parse_date('2026-05-06')").unwrap();
        let ast = parse(&toks).unwrap();
        let mut c = Checker::new(&sch);
        let typed = c.check(&ast).unwrap();
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&typed).unwrap();
        assert!(sql.contains("::date"), "expected ::date cast, got: {}", sql);
    }

    #[test]
    fn null_eq_null_is_constant_true() {
        let e = typed("null == null");
        let ctx = ContextValues::default();
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&e).unwrap();
        assert_eq!(sql, "TRUE");
    }

    #[test]
    fn parameterized_resolves_context() {
        let sch = MapSchema::new().with(&["this", "due_at"], Type::Timestamp);
        let toks = tokenize("due_at < now").unwrap();
        let ast = parse(&toks).unwrap();
        let mut c = Checker::new(&sch);
        let typed = c.check(&ast).unwrap();
        let ctx = ContextValues {
            now: Some("2026-05-05T12:00:00Z".into()),
            ..Default::default()
        };
        let mut em = SqlEmitter::new_parameterized(&ctx);
        let sql = em.emit(&typed).unwrap();
        assert!(sql.contains("due_at"));
        assert!(sql.contains("$1"));
        assert_eq!(em.params.len(), 1);
        assert!(matches!(&em.params[0], Param::Timestamp(s) if s == "2026-05-05T12:00:00Z"));
    }
}
