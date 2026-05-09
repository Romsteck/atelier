//! Runtime evaluator for impure (Runtime-purity) expressions.
//!
//! Used at READ time: the gateway has already fetched the target row and any expanded lookup
//! rows; the evaluator walks the typed AST and produces a `Value` that gets serialised back into
//! the JSON response.

use crate::ast::{BinOp, ContextRoot, Literal, UnOp};
use crate::checker::TypedExpr;
use crate::error::{Error, Result};
use crate::types::{NumKind, Type};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Null,
}

impl Value {
    pub fn ty(&self) -> Type {
        match self {
            Value::Int(_) => Type::Number(NumKind::Int),
            Value::Float(_) => Type::Number(NumKind::Float),
            Value::Text(_) => Type::Text,
            Value::Bool(_) => Type::Bool,
            Value::Null => Type::Null,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

/// Lookup interface for the row + expanded relations. Path `["this", "qty"]` for a same-row
/// column; `["customer", "name"]` for a 1-hop expand.
pub trait Row {
    fn get(&self, path: &[String]) -> Option<Value>;
}

#[derive(Debug, Default, Clone)]
pub struct MapRow {
    pub map: std::collections::HashMap<Vec<String>, Value>,
}

impl MapRow {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, path: &[&str], v: Value) -> Self {
        self.map.insert(path.iter().map(|s| s.to_string()).collect(), v);
        self
    }
}

impl Row for MapRow {
    fn get(&self, path: &[String]) -> Option<Value> {
        self.map.get(path).cloned()
    }
}

#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub now: Option<chrono::DateTime<chrono::Utc>>,
    pub today: Option<chrono::NaiveDate>,
    pub user: Option<uuid::Uuid>,
    pub app: Option<uuid::Uuid>,
}

pub fn eval(expr: &TypedExpr, row: &dyn Row, ctx: &EvalContext) -> Result<Value> {
    match expr {
        TypedExpr::Literal { lit, .. } => Ok(literal_to_value(lit)),

        TypedExpr::Path { path, .. } => Ok(row.get(path).unwrap_or(Value::Null)),

        TypedExpr::Context { root, .. } => Ok(match root {
            ContextRoot::Now => ctx
                .now
                .map(|d| Value::Text(d.to_rfc3339()))
                .unwrap_or(Value::Null),
            ContextRoot::Today => ctx
                .today
                .map(|d| Value::Text(d.to_string()))
                .unwrap_or(Value::Null),
            ContextRoot::User => ctx
                .user
                .map(|u| Value::Text(u.to_string()))
                .unwrap_or(Value::Null),
            ContextRoot::App => ctx
                .app
                .map(|u| Value::Text(u.to_string()))
                .unwrap_or(Value::Null),
        }),

        TypedExpr::Binary { op, lhs, rhs, .. } => {
            let l = eval(lhs, row, ctx)?;
            let r = eval(rhs, row, ctx)?;
            eval_binary(*op, l, r)
        }

        TypedExpr::Unary { op, rhs, .. } => {
            let r = eval(rhs, row, ctx)?;
            match (op, r) {
                (UnOp::Neg, Value::Int(v)) => Ok(Value::Int(-v)),
                (UnOp::Neg, Value::Float(v)) => Ok(Value::Float(-v)),
                (UnOp::Not, Value::Bool(v)) => Ok(Value::Bool(!v)),
                (_, Value::Null) => Ok(Value::Null),
                (op, v) => Err(Error::Eval(format!(
                    "unary {:?} not defined for {}",
                    op,
                    v.ty()
                ))),
            }
        }

        TypedExpr::Call { name, args, .. } => {
            let evaled: Vec<Value> = args
                .iter()
                .map(|a| eval(a, row, ctx))
                .collect::<Result<_>>()?;
            eval_call(name, &evaled)
        }

        TypedExpr::If { cond, then_, else_, .. } => {
            let c = eval(cond, row, ctx)?;
            match c {
                Value::Bool(true) => eval(then_, row, ctx),
                Value::Bool(false) | Value::Null => eval(else_, row, ctx),
                other => Err(Error::Eval(format!(
                    "IF condition must be Bool, got {}",
                    other.ty()
                ))),
            }
        }
    }
}

fn literal_to_value(lit: &Literal) -> Value {
    match lit {
        Literal::Int(v) => Value::Int(*v),
        Literal::Float(v) => Value::Float(*v),
        Literal::Text(s) => Value::Text(s.clone()),
        Literal::Bool(b) => Value::Bool(*b),
        Literal::Null => Value::Null,
    }
}

fn eval_binary(op: BinOp, l: Value, r: Value) -> Result<Value> {
    use BinOp::*;
    // Coalesce: distinct null semantics.
    if op == Coalesce {
        return Ok(if l.is_null() { r } else { l });
    }
    // Null propagation for arithmetic/comparison/logical (except `IF`).
    if matches!(l, Value::Null) || matches!(r, Value::Null) {
        // Logical with null: SQL three-valued logic — keep simple: NULL || true → true, etc.
        if op == And {
            if matches!(l, Value::Bool(false)) || matches!(r, Value::Bool(false)) {
                return Ok(Value::Bool(false));
            }
            return Ok(Value::Null);
        }
        if op == Or {
            if matches!(l, Value::Bool(true)) || matches!(r, Value::Bool(true)) {
                return Ok(Value::Bool(true));
            }
            return Ok(Value::Null);
        }
        return Ok(Value::Null);
    }

    if op.is_arithmetic() {
        // Promote ints to floats when either side is float OR for division.
        let promote = matches!(l, Value::Float(_)) || matches!(r, Value::Float(_)) || op == Div;
        if promote {
            let lf = l.as_float().unwrap();
            let rf = r.as_float().unwrap();
            return Ok(Value::Float(match op {
                Add => lf + rf,
                Sub => lf - rf,
                Mul => lf * rf,
                Div => {
                    if rf == 0.0 {
                        return Err(Error::Eval("division by zero".into()));
                    }
                    lf / rf
                }
                Mod => {
                    if rf == 0.0 {
                        return Err(Error::Eval("modulo by zero".into()));
                    }
                    lf % rf
                }
                _ => unreachable!(),
            }));
        }
        let li = l.as_int().unwrap();
        let ri = r.as_int().unwrap();
        return Ok(Value::Int(match op {
            Add => li.wrapping_add(ri),
            Sub => li.wrapping_sub(ri),
            Mul => li.wrapping_mul(ri),
            Mod => {
                if ri == 0 {
                    return Err(Error::Eval("modulo by zero".into()));
                }
                li % ri
            }
            _ => unreachable!(),
        }));
    }

    if op.is_comparison() {
        let cmp = compare(&l, &r).ok_or_else(|| {
            Error::Eval(format!(
                "cannot compare {} to {}",
                l.ty(),
                r.ty()
            ))
        })?;
        return Ok(Value::Bool(match op {
            Eq => cmp == std::cmp::Ordering::Equal,
            Ne => cmp != std::cmp::Ordering::Equal,
            Lt => cmp == std::cmp::Ordering::Less,
            Le => cmp != std::cmp::Ordering::Greater,
            Gt => cmp == std::cmp::Ordering::Greater,
            Ge => cmp != std::cmp::Ordering::Less,
            _ => unreachable!(),
        }));
    }

    if op.is_logical() {
        let lb = l.as_bool().unwrap();
        let rb = r.as_bool().unwrap();
        return Ok(Value::Bool(match op {
            And => lb && rb,
            Or => lb || rb,
            _ => unreachable!(),
        }));
    }

    unreachable!()
}

fn compare(l: &Value, r: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        _ => None,
    }
}

fn eval_call(name: &str, args: &[Value]) -> Result<Value> {
    let upper = name.to_ascii_uppercase();
    Ok(match upper.as_str() {
        "ABS" => match &args[0] {
            Value::Int(v) => Value::Int(v.wrapping_abs()),
            Value::Float(v) => Value::Float(v.abs()),
            Value::Null => Value::Null,
            v => return Err(Error::Eval(format!("ABS: bad arg {:?}", v))),
        },
        "ROUND" => match args[0] {
            Value::Float(v) => {
                let p = if args.len() == 2 {
                    args[1].as_int().ok_or_else(|| Error::Eval("ROUND: bad precision".into()))? as i32
                } else {
                    0
                };
                let f = 10f64.powi(p);
                Value::Float((v * f).round() / f)
            }
            Value::Int(v) => Value::Float(v as f64),
            Value::Null => Value::Null,
            ref v => return Err(Error::Eval(format!("ROUND: bad arg {:?}", v))),
        },
        "FLOOR" => match args[0] {
            Value::Float(v) => Value::Int(v.floor() as i64),
            Value::Int(v) => Value::Int(v),
            Value::Null => Value::Null,
            ref v => return Err(Error::Eval(format!("FLOOR: bad arg {:?}", v))),
        },
        "CEIL" => match args[0] {
            Value::Float(v) => Value::Int(v.ceil() as i64),
            Value::Int(v) => Value::Int(v),
            Value::Null => Value::Null,
            ref v => return Err(Error::Eval(format!("CEIL: bad arg {:?}", v))),
        },
        "COALESCE" => args
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null),
        "LEN" => match &args[0] {
            Value::Text(s) => Value::Int(s.chars().count() as i64),
            Value::Null => Value::Null,
            v => return Err(Error::Eval(format!("LEN: bad arg {:?}", v))),
        },
        "UPPER" => match &args[0] {
            Value::Text(s) => Value::Text(s.to_uppercase()),
            Value::Null => Value::Null,
            v => return Err(Error::Eval(format!("UPPER: bad arg {:?}", v))),
        },
        "LOWER" => match &args[0] {
            Value::Text(s) => Value::Text(s.to_lowercase()),
            Value::Null => Value::Null,
            v => return Err(Error::Eval(format!("LOWER: bad arg {:?}", v))),
        },
        "CONCAT" => {
            let mut out = String::new();
            for v in args {
                match v {
                    Value::Text(s) => out.push_str(s),
                    Value::Null => {}
                    _ => return Err(Error::Eval("CONCAT: non-text arg".into())),
                }
            }
            Value::Text(out)
        }
        "SUBSTR" => {
            let s = args[0].as_text().ok_or_else(|| Error::Eval("SUBSTR: bad string".into()))?;
            let start = args[1].as_int().ok_or_else(|| Error::Eval("SUBSTR: bad start".into()))?;
            // Postgres-style 1-based.
            let chars: Vec<char> = s.chars().collect();
            let from = (start - 1).max(0) as usize;
            let to = if args.len() == 3 {
                let len = args[2].as_int().ok_or_else(|| Error::Eval("SUBSTR: bad len".into()))?;
                (from + len.max(0) as usize).min(chars.len())
            } else {
                chars.len()
            };
            let out: String = chars[from.min(chars.len())..to].iter().collect();
            Value::Text(out)
        }
        "IF" => {
            // Defensive — checker desugars to TypedExpr::If.
            match args[0] {
                Value::Bool(true) => args[1].clone(),
                _ => args[2].clone(),
            }
        }
        // PARSE_TIMESTAMP/PARSE_DATE only carry semantic weight at SQL-emit time
        // (they cast text → tstz/date in PG so ordered comparisons type-check).
        // The runtime `Value` enum doesn't have Timestamp/Date variants — `now`
        // and `today` are themselves returned as `Value::Text` — so the runtime
        // evaluator just preserves the text form. Validation (RFC3339 etc.) is
        // deferred to PG, matching the SQL path.
        "PARSE_TIMESTAMP" | "PARSE_DATE" => match &args[0] {
            Value::Text(s) => Value::Text(s.clone()),
            Value::Null => Value::Null,
            other => return Err(Error::Eval(format!(
                "{name}: expected Text, got {:?}", other
            ))),
        },
        _ => return Err(Error::UnknownFunction(name.into())),
    })
}

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
            .with(&["this", "due_at"], Type::Timestamp)
    }

    fn typed(s: &str) -> TypedExpr {
        let toks = tokenize(s).unwrap();
        let ast = parse(&toks).unwrap();
        let sch = schema();
        let mut c = Checker::new(&sch);
        c.check(&ast).unwrap()
    }

    #[test]
    fn pure_arith() {
        let row = MapRow::new()
            .with(&["this", "qty"], Value::Int(3))
            .with(&["this", "price"], Value::Float(2.5));
        let ctx = EvalContext::default();
        let v = eval(&typed("qty * price"), &row, &ctx).unwrap();
        assert_eq!(v, Value::Float(7.5));
    }

    #[test]
    fn null_propagates_arith() {
        let row = MapRow::new()
            .with(&["this", "qty"], Value::Null)
            .with(&["this", "price"], Value::Float(2.5));
        let ctx = EvalContext::default();
        let v = eval(&typed("qty * price"), &row, &ctx).unwrap();
        assert!(v.is_null());
    }

    #[test]
    fn coalesce() {
        let row = MapRow::new()
            .with(&["this", "qty"], Value::Null)
            .with(&["this", "price"], Value::Float(2.5));
        let ctx = EvalContext::default();
        let v = eval(&typed("qty ?? 0"), &row, &ctx).unwrap();
        assert_eq!(v, Value::Int(0));
    }

    #[test]
    fn if_branch() {
        let row = MapRow::new()
            .with(&["this", "qty"], Value::Int(5))
            .with(&["this", "price"], Value::Float(2.0));
        let ctx = EvalContext::default();
        let v = eval(&typed("if(qty > 0, qty * price, 0)"), &row, &ctx).unwrap();
        assert_eq!(v, Value::Float(10.0));
    }

    #[test]
    fn now_context() {
        let row = MapRow::new();
        let now = chrono::Utc::now();
        let ctx = EvalContext {
            now: Some(now),
            ..Default::default()
        };
        let v = eval(&typed("now"), &row, &ctx).unwrap();
        match v {
            Value::Text(s) => assert!(!s.is_empty()),
            other => panic!("expected text, got {:?}", other),
        }
    }

    #[test]
    fn three_valued_logic() {
        let row = MapRow::new();
        let ctx = EvalContext::default();
        // null || true → true
        let v = eval(&typed("null || true"), &row, &ctx).unwrap();
        assert_eq!(v, Value::Bool(true));
        // null && false → false
        let v = eval(&typed("null && false"), &row, &ctx).unwrap();
        assert_eq!(v, Value::Bool(false));
        // null || false → null
        let v = eval(&typed("null || false"), &row, &ctx).unwrap();
        assert!(v.is_null());
    }
}
