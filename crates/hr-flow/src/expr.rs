//! Expression substitution and boolean evaluation.
//!
//! Two surfaces:
//!
//! - `substitute(value, ctx)` — walks a JSON value, replacing every
//!   `{{ ... }}` placeholder with the resolved JSON sub-value. A string
//!   that is *exactly* `{{ X }}` is replaced with the raw value of X
//!   (preserving its type — array, object, number, bool, …); a string
//!   that mixes literal text and placeholders is coerced to a string.
//! - `eval_bool(expr, ctx)` — evaluates a small condition language used by
//!   `if.cond` / `while.cond` / `filter.cond`. Supports `==`, `!=`, `<`,
//!   `<=`, `>`, `>=`, `&&`, `||`, `!`, parens, JSON literals, and path
//!   references resolved through `ctx`.
//!
//! Both are intentionally minimal — no full `dvexpr` integration in v1;
//! that lands when we need richer maths or string ops.

use serde_json::Value;

use crate::error::{FlowError, FlowResult};

/// Run state surface used by the resolver. Implemented by the executor.
pub trait ExprContext {
    fn step_output(&self, step_id: &str) -> Option<&Value>;
    fn input(&self) -> &Value;
    fn var(&self, name: &str) -> Option<&Value>;
    fn iter_var(&self, name: &str) -> Option<&Value>;
}

/// Recursively replace `{{ ... }}` placeholders in `value`.
pub fn substitute(value: &Value, ctx: &dyn ExprContext) -> FlowResult<Value> {
    match value {
        Value::String(s) => substitute_string(s, ctx),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for v in items {
                out.push(substitute(v, ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), substitute(v, ctx)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn substitute_string(s: &str, ctx: &dyn ExprContext) -> FlowResult<Value> {
    let placeholders = find_placeholders(s);
    if placeholders.is_empty() {
        return Ok(Value::String(s.to_string()));
    }

    // Whole-string placeholder: preserve original type.
    if placeholders.len() == 1 {
        let (start, end, inner) = &placeholders[0];
        if *start == 0 && *end == s.len() {
            return resolve_path(inner.trim(), ctx);
        }
    }

    // Mixed content: stringify each resolved value and splice.
    let mut out = String::new();
    let mut cursor = 0usize;
    for (start, end, inner) in placeholders {
        out.push_str(&s[cursor..start]);
        let resolved = resolve_path(inner.trim(), ctx)?;
        match resolved {
            Value::String(s) => out.push_str(&s),
            other => out.push_str(&other.to_string()),
        }
        cursor = end;
    }
    out.push_str(&s[cursor..]);
    Ok(Value::String(out))
}

/// Returns `(start, end, inner)` triples — `inner` is the path between the
/// braces, `start..end` covers `{{ ... }}` inclusive in the source.
fn find_placeholders(s: &str) -> Vec<(usize, usize, String)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(rel_end) = find_close(&bytes[i + 2..]) {
                let inner_start = i + 2;
                let inner_end = inner_start + rel_end;
                let end = inner_end + 2;
                let inner = s[inner_start..inner_end].to_string();
                out.push((i, end, inner));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find_close(after: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < after.len() {
        if after[i] == b'}' && after[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Resolve a path expression like `steps.fetch.output.id`, `input.user_id`,
/// `vars.total`, `@iter`, `@iter.amount`. Dot-separated traversal.
fn resolve_path(path: &str, ctx: &dyn ExprContext) -> FlowResult<Value> {
    let path = path.trim();
    if path.is_empty() {
        return Err(FlowError::Expression("empty path".into()));
    }

    // Bare literals usable inside `{{ … }}` so flows can inject them where
    // TOML can't (TOML has no `null` keyword). `{{ null }}`, `{{ true }}`,
    // `{{ false }}` resolve to the corresponding JSON value.
    match path {
        "null" => return Ok(Value::Null),
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }

    // Iteration-variable shorthand: `@iter` or `@iter.x`.
    if let Some(rest) = path.strip_prefix('@') {
        let (head, tail) = split_head(rest);
        let value = ctx
            .iter_var(head)
            .ok_or_else(|| FlowError::Expression(format!("iter var `{head}` not in scope")))?;
        return Ok(traverse(value, tail));
    }

    let (head, tail) = split_head(path);
    match head {
        "input" => Ok(traverse(ctx.input(), tail)),
        "vars" => {
            let (var_name, rest) = split_head(tail);
            let v = ctx
                .var(var_name)
                .ok_or_else(|| FlowError::Expression(format!("var `{var_name}` not set")))?;
            Ok(traverse(v, rest))
        }
        "steps" => {
            let (step_id, rest) = split_head(tail);
            let v = ctx
                .step_output(step_id)
                .ok_or_else(|| FlowError::Expression(format!("step `{step_id}` has no output yet")))?;
            // `steps.X.output.field` — drop the `output.` segment.
            let trimmed = rest.strip_prefix("output").unwrap_or(rest);
            let trimmed = trimmed.strip_prefix('.').unwrap_or(trimmed);
            Ok(traverse(v, trimmed))
        }
        _ => Err(FlowError::Expression(format!("unknown root `{head}`"))),
    }
}

fn split_head(path: &str) -> (&str, &str) {
    match path.split_once('.') {
        Some((h, t)) => (h, t),
        None => (path, ""),
    }
}

fn traverse(value: &Value, path: &str) -> Value {
    if path.is_empty() {
        return value.clone();
    }
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        // Object keys take precedence over positional indices: a numeric
        // *key* on an object (`{"0": …}`) must stay reachable. Only fall
        // back to array indexing when the current value is actually an array.
        current = match current {
            Value::Array(_) => match segment.parse::<usize>() {
                Ok(idx) => current.get(idx).unwrap_or(&Value::Null),
                Err(_) => &Value::Null,
            },
            _ => current.get(segment).unwrap_or(&Value::Null),
        };
    }
    current.clone()
}

// ---------------------------------------------------------------------------
// Boolean condition evaluation
// ---------------------------------------------------------------------------

/// Substitute `{{ ... }}` placeholders for use inside an expression that
/// will be re-parsed (e.g. `if.cond`). Differs from `substitute()` in that
/// resolved values are serialised back as **JSON literals** — strings keep
/// their quotes, `null` stays `null`, numbers keep their form. This way the
/// downstream parser sees `"Food" == null` instead of `Food == null`.
pub fn substitute_for_expr(s: &str, ctx: &dyn ExprContext) -> FlowResult<String> {
    let placeholders = find_placeholders(s);
    if placeholders.is_empty() {
        return Ok(s.to_string());
    }
    let mut out = String::new();
    let mut cursor = 0usize;
    for (start, end, inner) in placeholders {
        out.push_str(&s[cursor..start]);
        let resolved = resolve_path(inner.trim(), ctx)?;
        out.push_str(
            &serde_json::to_string(&resolved)
                .map_err(|e| FlowError::Expression(e.to_string()))?,
        );
        cursor = end;
    }
    out.push_str(&s[cursor..]);
    Ok(out)
}

/// Evaluate `expr` as a boolean.
///
/// Two paths:
/// 1. **Fast-path**: if `expr` (trimmed) is exactly a single whole-string
///    placeholder like `{{ steps.x.output }}`, the resolved value is fed
///    straight to `value_truthy()` — preserving Array/Object/Number
///    semantics. This is the canonical "is non-empty?" idiom and Power
///    Automate's Condition behaviour.
/// 2. **Parse path**: anything else is run through `substitute_for_expr`
///    (placeholders → JSON literals) then parsed by the precedence-climbing
///    evaluator (`==`, `<=`, `&&`, function calls, …).
pub fn eval_bool(expr: &str, ctx: &dyn ExprContext) -> FlowResult<bool> {
    let trimmed = expr.trim();
    let placeholders = find_placeholders(trimmed);
    if placeholders.len() == 1 {
        let (start, end, _) = &placeholders[0];
        if *start == 0 && *end == trimmed.len() {
            // Whole-string placeholder: preserve type via substitute().
            let resolved = substitute(&Value::String(trimmed.to_string()), ctx)?;
            return Ok(value_truthy(&resolved));
        }
    }

    let s = substitute_for_expr(expr, ctx)?;
    let mut parser = Parser::new(&s);
    let v = parser
        .parse_or()
        .map_err(|e| match e {
            FlowError::Expression(msg) => FlowError::Expression(format!("{msg} in `{s}`")),
            other => other,
        })?;
    parser.skip_ws();
    if parser.pos != parser.src.len() {
        return Err(FlowError::Expression(format!("trailing input in `{s}`")));
    }
    Ok(value_truthy(&v))
}

fn value_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self { Self { src: s.as_bytes(), pos: 0 } }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> { self.src.get(self.pos).copied() }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.src[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else { false }
    }

    fn parse_or(&mut self) -> FlowResult<Value> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.eat(b"||") {
                let right = self.parse_and()?;
                left = Value::Bool(value_truthy(&left) || value_truthy(&right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> FlowResult<Value> {
        let mut left = self.parse_cmp()?;
        loop {
            self.skip_ws();
            if self.eat(b"&&") {
                let right = self.parse_cmp()?;
                left = Value::Bool(value_truthy(&left) && value_truthy(&right));
            } else { break; }
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> FlowResult<Value> {
        let left = self.parse_unary()?;
        self.skip_ws();
        // Two-char operators first.
        let op = if self.eat(b"==") { Some("==") }
            else if self.eat(b"!=") { Some("!=") }
            else if self.eat(b"<=") { Some("<=") }
            else if self.eat(b">=") { Some(">=") }
            else if self.eat(b"<")  { Some("<") }
            else if self.eat(b">")  { Some(">") }
            else { None };

        let Some(op) = op else { return Ok(left); };
        let right = self.parse_unary()?;
        Ok(Value::Bool(compare(&left, op, &right)))
    }

    fn parse_unary(&mut self) -> FlowResult<Value> {
        self.skip_ws();
        if self.eat(b"!") {
            let v = self.parse_unary()?;
            return Ok(Value::Bool(!value_truthy(&v)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> FlowResult<Value> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let v = self.parse_or()?;
                self.skip_ws();
                if !self.eat(b")") {
                    return Err(FlowError::Expression("expected `)`".into()));
                }
                Ok(v)
            }
            Some(c @ (b'"' | b'\'')) => self.parse_string(c),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.parse_ident_or_call(),
            other => Err(FlowError::Expression(format!(
                "unexpected token at byte {}: {:?}", self.pos, other
            ))),
        }
    }

    /// Reads an identifier and either resolves it as a keyword (`true` /
    /// `false` / `null`) or, if followed by `(`, calls a built-in function.
    fn parse_ident_or_call(&mut self) -> FlowResult<Value> {
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' { self.pos += 1; } else { break; }
        }
        let ident = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| FlowError::Expression(e.to_string()))?
            .to_string();

        self.skip_ws();
        if self.eat(b"(") {
            let mut args = Vec::new();
            self.skip_ws();
            if self.peek() != Some(b')') {
                loop {
                    args.push(self.parse_or()?);
                    self.skip_ws();
                    if !self.eat(b",") { break; }
                }
            }
            self.skip_ws();
            if !self.eat(b")") {
                return Err(FlowError::Expression(format!(
                    "expected `)` after `{ident}(...)`"
                )));
            }
            apply_fn(&ident, args)
        } else {
            match ident.as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                "null" => Ok(Value::Null),
                other => Err(FlowError::Expression(format!("unknown identifier `{other}`"))),
            }
        }
    }

    /// Parse a JSON object literal `{"key": value, ...}`.
    ///
    /// Used to consume the JSON serialisation that `substitute_for_expr`
    /// injects when a placeholder resolves to an Object — without this the
    /// downstream tokenizer rejects the leading `{`. Strict-JSON: keys must
    /// be double-quoted strings, values recurse through `parse_atom` (no
    /// trailing commas, no comments).
    fn parse_object(&mut self) -> FlowResult<Value> {
        self.pos += 1; // opening `{`
        let mut map = serde_json::Map::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_ws();
            let key = match self.peek() {
                Some(c @ (b'"' | b'\'')) => match self.parse_string(c)? {
                    Value::String(s) => s,
                    _ => unreachable!("parse_string always returns Value::String"),
                },
                _ => {
                    return Err(FlowError::Expression(format!(
                        "expected string key in object at byte {}",
                        self.pos
                    )));
                }
            };
            self.skip_ws();
            if !self.eat(b":") {
                return Err(FlowError::Expression(format!(
                    "expected `:` after key `{key}` at byte {}",
                    self.pos
                )));
            }
            self.skip_ws();
            let value = self.parse_atom()?;
            map.insert(key, value);
            self.skip_ws();
            if self.eat(b",") {
                continue;
            }
            if self.eat(b"}") {
                break;
            }
            return Err(FlowError::Expression(format!(
                "expected `,` or `}}` in object at byte {}",
                self.pos
            )));
        }
        Ok(Value::Object(map))
    }

    /// Parse a JSON array literal `[v1, v2, ...]`. Same rationale as
    /// `parse_object` — covers the substitution of Array placeholders.
    fn parse_array(&mut self) -> FlowResult<Value> {
        self.pos += 1; // opening `[`
        let mut arr = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(arr));
        }
        loop {
            self.skip_ws();
            let value = self.parse_atom()?;
            arr.push(value);
            self.skip_ws();
            if self.eat(b",") {
                continue;
            }
            if self.eat(b"]") {
                break;
            }
            return Err(FlowError::Expression(format!(
                "expected `,` or `]` in array at byte {}",
                self.pos
            )));
        }
        Ok(Value::Array(arr))
    }

    fn parse_string(&mut self, delim: u8) -> FlowResult<Value> {
        self.pos += 1; // opening quote
        let start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] != delim {
            self.pos += 1;
        }
        if self.pos >= self.src.len() {
            return Err(FlowError::Expression("unterminated string".into()));
        }
        let s = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| FlowError::Expression(e.to_string()))?
            .to_string();
        self.pos += 1; // closing quote
        Ok(Value::String(s))
    }

    fn parse_number(&mut self) -> FlowResult<Value> {
        let start = self.pos;
        if self.src[self.pos] == b'-' { self.pos += 1; }
        while self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == b'.')
        {
            self.pos += 1;
        }
        let s = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| FlowError::Expression(e.to_string()))?;
        let n: f64 = s.parse().map_err(|e: std::num::ParseFloatError| FlowError::Expression(e.to_string()))?;
        Ok(serde_json::json!(n))
    }

}

/// Built-in functions callable from `if.cond` / `filter.cond` / etc.
///
/// `starts_with(s, prefix)`, `ends_with(s, suffix)`, `contains(s, needle)`,
/// `len(x)` — length of array, string, object, or 0 for null.
/// `lower(s)`, `upper(s)`.
fn apply_fn(name: &str, args: Vec<Value>) -> FlowResult<Value> {
    fn arity(name: &str, args: &[Value], expected: usize) -> FlowResult<()> {
        if args.len() != expected {
            return Err(FlowError::Expression(format!(
                "`{name}` takes {expected} arg(s), got {}",
                args.len()
            )));
        }
        Ok(())
    }
    fn as_str(v: &Value) -> FlowResult<&str> {
        v.as_str().ok_or_else(|| FlowError::Expression(format!(
            "expected string, got {v:?}"
        )))
    }

    match name {
        "starts_with" => {
            arity(name, &args, 2)?;
            Ok(Value::Bool(as_str(&args[0])?.starts_with(as_str(&args[1])?)))
        }
        "ends_with" => {
            arity(name, &args, 2)?;
            Ok(Value::Bool(as_str(&args[0])?.ends_with(as_str(&args[1])?)))
        }
        "contains" => {
            arity(name, &args, 2)?;
            Ok(Value::Bool(as_str(&args[0])?.contains(as_str(&args[1])?)))
        }
        "len" => {
            arity(name, &args, 1)?;
            let n = match &args[0] {
                Value::Array(a) => a.len(),
                Value::String(s) => s.chars().count(),
                Value::Object(o) => o.len(),
                Value::Null => 0,
                other => return Err(FlowError::Expression(format!(
                    "len(): expected array/string/object/null, got {other:?}"
                ))),
            };
            Ok(Value::from(n))
        }
        "lower" => {
            arity(name, &args, 1)?;
            Ok(Value::String(as_str(&args[0])?.to_lowercase()))
        }
        "upper" => {
            arity(name, &args, 1)?;
            Ok(Value::String(as_str(&args[0])?.to_uppercase()))
        }
        other => Err(FlowError::Expression(format!("unknown function `{other}`"))),
    }
}

fn compare(left: &Value, op: &str, right: &Value) -> bool {
    match op {
        "==" => left == right || numeric_eq(left, right),
        "!=" => !(left == right || numeric_eq(left, right)),
        "<" | "<=" | ">" | ">=" => {
            // Numeric path first (handles `Number op Number`).
            if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
                return match op {
                    "<" => l < r,
                    "<=" => l <= r,
                    ">" => l > r,
                    ">=" => l >= r,
                    _ => false,
                };
            }
            // String lex path — covers ISO-8601 dates and prefix-style range
            // tricks. Both sides must be strings; mixing types still returns
            // false (avoid surprises).
            if let (Value::String(l), Value::String(r)) = (left, right) {
                return match op {
                    "<" => l < r,
                    "<=" => l <= r,
                    ">" => l > r,
                    ">=" => l >= r,
                    _ => false,
                };
            }
            false
        }
        _ => false,
    }
}

fn numeric_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    struct TestCtx {
        input: Value,
        vars: HashMap<String, Value>,
        steps: HashMap<String, Value>,
        iter: HashMap<String, Value>,
    }

    impl ExprContext for TestCtx {
        fn step_output(&self, step_id: &str) -> Option<&Value> { self.steps.get(step_id) }
        fn input(&self) -> &Value { &self.input }
        fn var(&self, name: &str) -> Option<&Value> { self.vars.get(name) }
        fn iter_var(&self, name: &str) -> Option<&Value> { self.iter.get(name) }
    }

    fn ctx() -> TestCtx {
        let mut steps = HashMap::new();
        steps.insert("fetch".into(), json!({ "id": 42, "label": "hello" }));
        let mut vars = HashMap::new();
        vars.insert("total".into(), json!(7.5));
        let mut iter = HashMap::new();
        iter.insert("iter".into(), json!({ "amount": 100 }));
        TestCtx {
            input: json!({ "user_id": "abc" }),
            vars, steps, iter,
        }
    }

    #[test]
    fn substitutes_whole_string_preserves_type() {
        let v = substitute(&json!("{{ steps.fetch.output.id }}"), &ctx()).unwrap();
        assert_eq!(v, json!(42));
    }

    #[test]
    fn substitutes_mixed_string_coerces() {
        let v = substitute(&json!("hello {{ steps.fetch.output.label }}!"), &ctx()).unwrap();
        assert_eq!(v, json!("hello hello!"));
    }

    #[test]
    fn substitutes_in_array_and_object() {
        let v = substitute(&json!({
            "user": "{{ input.user_id }}",
            "items": ["{{ vars.total }}", 1]
        }), &ctx()).unwrap();
        assert_eq!(v, json!({ "user": "abc", "items": [7.5, 1] }));
    }

    #[test]
    fn substitutes_iter_var() {
        let v = substitute(&json!("{{ @iter.amount }}"), &ctx()).unwrap();
        assert_eq!(v, json!(100));
    }

    #[test]
    fn eval_bool_basic_compare() {
        assert!(eval_bool("{{ steps.fetch.output.id }} == 42", &ctx()).unwrap());
        assert!(!eval_bool("{{ vars.total }} > 100", &ctx()).unwrap());
        assert!(eval_bool("{{ vars.total }} > 5 && {{ vars.total }} < 10", &ctx()).unwrap());
    }

    #[test]
    fn eval_bool_string_eq() {
        assert!(eval_bool("\"abc\" == \"abc\"", &ctx()).unwrap());
        assert!(!eval_bool("\"abc\" == \"def\"", &ctx()).unwrap());
    }

    #[test]
    fn eval_bool_negation_parens() {
        assert!(eval_bool("!(1 == 2)", &ctx()).unwrap());
    }

    #[test]
    fn eval_bool_single_quote_string() {
        // 3a — String literals accept both `"..."` and `'...'` so users can
        // pick whichever doesn't clash with their TOML quoting.
        assert!(eval_bool("'abc' == 'abc'", &ctx()).unwrap());
        assert!(eval_bool("'abc' == \"abc\"", &ctx()).unwrap());
        assert!(!eval_bool("'abc' == 'def'", &ctx()).unwrap());
        // Empty literal — the canonical motivating case from the www flows.
        assert!(eval_bool("'' == ''", &ctx()).unwrap());
        assert!(eval_bool("'hello' != ''", &ctx()).unwrap());
    }

    #[test]
    fn eval_bool_object_literal_vs_null() {
        // 3b — substitute_for_expr injects `{...}` JSON when a placeholder
        // resolves to an Object; the parser must consume that literal so
        // `{{ X }} != null` works for arbitrary JSON values, not just scalars.
        let mut steps = HashMap::new();
        steps.insert("row".into(), json!({ "id": 1, "label": "x" }));
        steps.insert("none".into(), Value::Null);
        let c = TestCtx {
            input: Value::Null,
            vars: HashMap::new(),
            steps,
            iter: HashMap::new(),
        };
        assert!(eval_bool("{{ steps.row.output }} != null", &c).unwrap());
        assert!(!eval_bool("{{ steps.row.output }} == null", &c).unwrap());
        assert!(eval_bool("{{ steps.none.output }} == null", &c).unwrap());
        assert!(!eval_bool("{{ steps.none.output }} != null", &c).unwrap());
    }

    #[test]
    fn eval_bool_array_literal_vs_null() {
        // 3b cousin — same story for arrays.
        let mut steps = HashMap::new();
        steps.insert("rows".into(), json!([1, 2, 3]));
        steps.insert("empty".into(), json!([]));
        let c = TestCtx {
            input: Value::Null,
            vars: HashMap::new(),
            steps,
            iter: HashMap::new(),
        };
        assert!(eval_bool("{{ steps.rows.output }} != null", &c).unwrap());
        assert!(eval_bool("{{ steps.empty.output }} != null", &c).unwrap());
    }

    #[test]
    fn parse_object_and_array_literals_directly() {
        // The parser must also accept hand-written object/array literals so
        // future expression sugar (e.g. comparing two objects for equality)
        // composes cleanly.
        assert!(eval_bool("{\"a\": 1} == {\"a\": 1}", &ctx()).unwrap());
        assert!(!eval_bool("{\"a\": 1} == {\"a\": 2}", &ctx()).unwrap());
        assert!(eval_bool("[1, 2] == [1, 2]", &ctx()).unwrap());
        assert!(eval_bool("{} != null", &ctx()).unwrap());
        assert!(eval_bool("[] != null", &ctx()).unwrap());
    }

    #[test]
    fn traverse_prefers_object_key_over_array_index() {
        // A numeric *key* on an object must stay reachable; positional
        // indexing only applies when the value is genuinely an array.
        let mut steps = HashMap::new();
        steps.insert("s".into(), json!({ "0": "zero", "items": [10, 20, 30] }));
        let c = TestCtx {
            input: Value::Null,
            vars: HashMap::new(),
            steps,
            iter: HashMap::new(),
        };
        assert_eq!(
            substitute(&json!("{{ steps.s.output.0 }}"), &c).unwrap(),
            json!("zero")
        );
        assert_eq!(
            substitute(&json!("{{ steps.s.output.items.2 }}"), &c).unwrap(),
            json!(30)
        );
    }
}
