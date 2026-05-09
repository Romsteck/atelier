//! DAG executor.
//!
//! Walks the parent → children index, evaluates step params through `expr`,
//! dispatches to connectors / actions / primitives, and accumulates a tree
//! of `StepRecord`s. The records are returned alongside the result so the
//! caller can persist a partial trace even when the run errors out.
//!
//! v1 primitives implemented: `connector`, `action`, `compose`, `set_var`,
//! `append_to_var`, `increment_var`, `if`, `for_each`. Others land later.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::connector::Connector;
use crate::definition::{FlowDef, StepDef, StepKind, index_children};
use crate::engine::ActionFn;
use crate::error::{FlowError, FlowResult};
use crate::expr::{eval_bool, substitute, ExprContext};

/// One persisted node in the run tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub record_id: String,
    pub step_id: String,
    pub parent_record_id: Option<String>,
    pub kind: String,
    /// Sub-discriminator surfacing the *what* under the *kind*:
    /// - `connector` → `"<connector>.<op>"` (e.g. `"dataverse.list"`)
    /// - `action`    → action name (e.g. `"compute_risk_score"`)
    /// - primitives  → `None`
    /// Used by the Studio to label a step at a glance without expanding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub branch: Option<String>,
    pub iteration_index: Option<u32>,
    pub status: String,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: i64,
    pub attempt: u32,
}

pub struct ExecutorInput<'a> {
    pub flow: &'a FlowDef,
    pub run_id: String,
    pub flow_input: Value,
    pub connectors: &'a HashMap<String, Arc<dyn Connector>>,
    pub actions: &'a HashMap<String, ActionFn>,
}

pub(crate) struct RunState {
    input: Value,
    vars: HashMap<String, Value>,
    step_outputs: HashMap<String, Value>,
    iter_stack: Vec<HashMap<String, Value>>,
    pub records: Vec<StepRecord>,
}

impl ExprContext for RunState {
    fn step_output(&self, step_id: &str) -> Option<&Value> {
        self.step_outputs.get(step_id)
    }
    fn input(&self) -> &Value { &self.input }
    fn var(&self, name: &str) -> Option<&Value> { self.vars.get(name) }
    fn iter_var(&self, name: &str) -> Option<&Value> {
        for frame in self.iter_stack.iter().rev() {
            if let Some(v) = frame.get(name) { return Some(v); }
        }
        None
    }
}

/// Run the flow against fresh state. Returns the (last) output if the run
/// reached the end, plus the records accumulated during execution. The
/// records vector is always returned, even on error — the caller persists
/// partial traces for failed runs.
pub async fn execute(input: ExecutorInput<'_>) -> (FlowResult<Value>, Vec<StepRecord>) {
    let mut state = RunState {
        input: input.flow_input.clone(),
        vars: HashMap::new(),
        step_outputs: HashMap::new(),
        iter_stack: Vec::new(),
        records: Vec::new(),
    };

    let children_idx = index_children(input.flow);
    let top_level: Vec<&StepDef> = children_idx
        .get(&None)
        .cloned()
        .unwrap_or_default();

    let ctx = ExecCtx {
        children_idx: &children_idx,
        connectors: input.connectors,
        actions: input.actions,
    };

    let mut last_output = Value::Null;
    let mut result: FlowResult<Value> = Ok(Value::Null);
    for step in &top_level {
        match run_step(&ctx, &mut state, step, None, None, None).await {
            Ok(v) => last_output = v,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    if result.is_ok() { result = Ok(last_output); }

    (result, state.records)
}

struct ExecCtx<'a> {
    children_idx: &'a std::collections::BTreeMap<Option<String>, Vec<&'a StepDef>>,
    connectors: &'a HashMap<String, Arc<dyn Connector>>,
    actions: &'a HashMap<String, ActionFn>,
}

#[allow(clippy::too_many_arguments)]
fn run_step<'a>(
    ctx: &'a ExecCtx<'a>,
    state: &'a mut RunState,
    step: &'a StepDef,
    parent_record_id: Option<String>,
    branch: Option<String>,
    iteration_index: Option<u32>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = FlowResult<Value>> + Send + 'a>> {
    Box::pin(run_step_inner(ctx, state, step, parent_record_id, branch, iteration_index))
}

async fn run_step_inner(
    ctx: &ExecCtx<'_>,
    state: &mut RunState,
    step: &StepDef,
    parent_record_id: Option<String>,
    branch: Option<String>,
    iteration_index: Option<u32>,
) -> FlowResult<Value> {
    let started = Utc::now();
    let record_id = uuid::Uuid::new_v4().to_string();
    let kind_label = step_kind_label(&step.kind);
    let detail = match &step.kind {
        StepKind::Connector { connector, op } => Some(format!("{connector}.{op}")),
        StepKind::Action { action } => Some(action.clone()),
        _ => None,
    };

    // Substitute params eagerly except when the step is a control primitive
    // that consumes its own raw expression (`if.cond`, `for_each.over`, …).
    let resolved_params: FlowResult<Value> = match &step.kind {
        StepKind::If { .. }
        | StepKind::Switch { .. }
        | StepKind::ForEach { .. }
        | StepKind::While { .. }
        | StepKind::Filter { .. }
        | StepKind::Sort { .. }
        | StepKind::Dedupe { .. }
        | StepKind::Partition { .. }
        | StepKind::GroupBy { .. }
        | StepKind::Select { .. }
        | StepKind::Join { .. }
        | StepKind::ParseJson { .. }
        | StepKind::Take { .. }
        | StepKind::Length { .. } => Ok(Value::Null),
        _ => substitute(&step.params, state),
    };

    let resolved_params = match resolved_params {
        Ok(v) => v,
        Err(e) => {
            push_record(
                state, record_id, step, parent_record_id, branch, iteration_index,
                kind_label, detail, started, None, None, Some(e.to_string()),
            );
            return Err(FlowError::StepFailed {
                step_id: step.id.clone(),
                message: format!("param substitution failed: {e}"),
            });
        }
    };

    let outcome: FlowResult<Value> = match &step.kind {
        StepKind::Connector { connector, op } => {
            run_connector(ctx, connector, op, &resolved_params).await
        }
        StepKind::Action { action } => {
            run_action(ctx, action, &resolved_params).await
        }
        StepKind::Compose { value } => {
            substitute(value, state)
        }
        StepKind::SetVar { name, value } => {
            let resolved = substitute(value, state)?;
            state.vars.insert(name.clone(), resolved.clone());
            Ok(resolved)
        }
        StepKind::AppendToVar { name, value } => {
            let resolved = substitute(value, state)?;
            let entry = state.vars.entry(name.clone()).or_insert_with(|| Value::Array(vec![]));
            if let Value::Array(arr) = entry {
                arr.push(resolved.clone());
                Ok(entry.clone())
            } else {
                Err(FlowError::StepFailed {
                    step_id: step.id.clone(),
                    message: format!("var `{name}` is not an array"),
                })
            }
        }
        StepKind::IncrementVar { name, by } => {
            let resolved_by = substitute(by, state)?;
            let inc = resolved_by.as_f64().unwrap_or(1.0);
            let entry = state.vars.entry(name.clone()).or_insert_with(|| serde_json::json!(0.0));
            let current = entry.as_f64().unwrap_or(0.0);
            *entry = serde_json::json!(current + inc);
            Ok(entry.clone())
        }
        StepKind::If { cond } => {
            let truthy = eval_bool(cond, state)?;
            let chosen = if truthy { "then" } else { "else" };
            let children: Vec<&StepDef> = ctx
                .children_idx
                .get(&Some(step.id.clone()))
                .cloned()
                .unwrap_or_default();
            let mut last = Value::Null;
            for child in children {
                let child_branch = child.parent_branch.as_deref().unwrap_or("then");
                if child_branch != chosen { continue; }
                last = run_step(
                    ctx, state, child,
                    Some(record_id.clone()),
                    Some(chosen.to_string()),
                    None,
                ).await?;
            }
            Ok(serde_json::json!({ "branch": chosen, "output": last }))
        }
        StepKind::ForEach { over, r#as, concurrency: _ } => {
            // Expression `over` is a path like `steps.X.output`; wrap in
            // `{{ }}` so substitute() preserves the array type.
            let collection = substitute(&Value::String(format!("{{{{ {over} }}}}")), state)?;
            let items = match collection {
                Value::Array(a) => a,
                other => return Err(FlowError::StepFailed {
                    step_id: step.id.clone(),
                    message: format!("for_each `over` must resolve to an array, got {other:?}"),
                }),
            };

            let children: Vec<&StepDef> = ctx
                .children_idx
                .get(&Some(step.id.clone()))
                .cloned()
                .unwrap_or_default();

            let mut iterations: Vec<Value> = Vec::with_capacity(items.len());
            for (idx, item) in items.into_iter().enumerate() {
                state.iter_stack.push(make_iter_frame(r#as, item, idx));
                let mut last = Value::Null;
                let mut iter_err: Option<FlowError> = None;
                for child in &children {
                    match run_step(
                        ctx, state, *child,
                        Some(record_id.clone()),
                        None,
                        Some(idx as u32),
                    ).await {
                        Ok(v) => last = v,
                        Err(e) => { iter_err = Some(e); break; }
                    }
                }
                state.iter_stack.pop();
                if let Some(e) = iter_err { return Err(e); }
                iterations.push(last);
            }
            Ok(Value::Array(iterations))
        }
        StepKind::Filter { from, cond } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let mut out = Vec::with_capacity(coll.len());
            for (idx, item) in coll.into_iter().enumerate() {
                state.iter_stack.push(make_iter_frame("iter", item.clone(), idx));
                let keep = eval_bool(cond, state);
                state.iter_stack.pop();
                if keep? { out.push(item); }
            }
            Ok(Value::Array(out))
        }
        StepKind::Sort { from, by, desc } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let mut indexed: Vec<(Value, Value)> = Vec::with_capacity(coll.len());
            for (idx, item) in coll.into_iter().enumerate() {
                state.iter_stack.push(make_iter_frame("iter", item.clone(), idx));
                let key = substitute(&Value::String(format!("{{{{ {by} }}}}")), state)?;
                state.iter_stack.pop();
                indexed.push((key, item));
            }
            indexed.sort_by(|a, b| compare_values(&a.0, &b.0));
            if *desc { indexed.reverse(); }
            Ok(Value::Array(indexed.into_iter().map(|(_, v)| v).collect()))
        }
        StepKind::Select { from, project } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let mut out = Vec::with_capacity(coll.len());
            for (idx, item) in coll.into_iter().enumerate() {
                state.iter_stack.push(make_iter_frame("iter", item, idx));
                let projected = substitute(project, state);
                state.iter_stack.pop();
                out.push(projected?);
            }
            Ok(Value::Array(out))
        }
        StepKind::Take { from, count } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let n = (*count as usize).min(coll.len());
            Ok(Value::Array(coll.into_iter().take(n).collect()))
        }
        StepKind::Dedupe { from, by } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut out = Vec::with_capacity(coll.len());
            for (idx, item) in coll.into_iter().enumerate() {
                let key = if let Some(by_expr) = by {
                    state.iter_stack.push(make_iter_frame("iter", item.clone(), idx));
                    let v = substitute(&Value::String(format!("{{{{ {by_expr} }}}}")), state);
                    state.iter_stack.pop();
                    match v? {
                        Value::String(s) => s,
                        other => other.to_string(),
                    }
                } else {
                    serde_json::to_string(&item).unwrap_or_default()
                };
                if seen.insert(key) { out.push(item); }
            }
            Ok(Value::Array(out))
        }
        StepKind::GroupBy { from, by } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let mut groups: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
            for (idx, item) in coll.into_iter().enumerate() {
                state.iter_stack.push(make_iter_frame("iter", item.clone(), idx));
                let key_val = substitute(&Value::String(format!("{{{{ {by} }}}}")), state);
                state.iter_stack.pop();
                let key_str = match key_val? {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                groups.entry(key_str).or_default().push(item);
            }
            let obj: serde_json::Map<String, Value> = groups
                .into_iter()
                .map(|(k, v)| (k, Value::Array(v)))
                .collect();
            Ok(Value::Object(obj))
        }
        StepKind::Partition { from, cond } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let mut yes = Vec::new();
            let mut no = Vec::new();
            for (idx, item) in coll.into_iter().enumerate() {
                state.iter_stack.push(make_iter_frame("iter", item.clone(), idx));
                let keep = eval_bool(cond, state);
                state.iter_stack.pop();
                if keep? { yes.push(item); } else { no.push(item); }
            }
            Ok(serde_json::json!({ "matching": yes, "non_matching": no }))
        }
        StepKind::Length { from } => {
            let resolved = substitute(&Value::String(format!("{{{{ {from} }}}}")), state)?;
            let n = match resolved {
                Value::Array(a) => a.len(),
                Value::String(s) => s.chars().count(),
                Value::Object(o) => o.len(),
                Value::Null => 0,
                other => return Err(FlowError::StepFailed {
                    step_id: step.id.clone(),
                    message: format!("length: expected array/string/object/null, got {other:?}"),
                }),
            };
            Ok(Value::from(n))
        }
        StepKind::Join { from, separator } => {
            let coll = resolve_collection(from, state, &step.id)?;
            let parts: Vec<String> = coll.into_iter().map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            }).collect();
            Ok(Value::String(parts.join(separator)))
        }
        StepKind::ParseJson { from } => {
            let resolved = substitute(&Value::String(format!("{{{{ {from} }}}}")), state)?;
            let s = match resolved {
                Value::String(s) => s,
                other => return Err(FlowError::StepFailed {
                    step_id: step.id.clone(),
                    message: format!("parse_json input must be string, got {other:?}"),
                }),
            };
            serde_json::from_str(&s).map_err(|e| FlowError::StepFailed {
                step_id: step.id.clone(),
                message: format!("parse_json: {e}"),
            })
        }
        other => Err(FlowError::Internal(format!(
            "step kind `{}` not yet implemented", step_kind_label(other),
        ))),
    };

    let (output_opt, error_msg) = match outcome {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e.to_string())),
    };

    push_record(
        state, record_id, step, parent_record_id, branch, iteration_index,
        kind_label, detail, started,
        Some(resolved_params),
        output_opt.clone(),
        error_msg.clone(),
    );

    if let Some(msg) = error_msg {
        return Err(FlowError::StepFailed {
            step_id: step.id.clone(),
            message: msg,
        });
    }

    let out = output_opt.unwrap_or(Value::Null);
    state.step_outputs.insert(step.id.clone(), out.clone());
    Ok(out)
}

async fn run_connector(
    ctx: &ExecCtx<'_>,
    connector: &str,
    op: &str,
    params: &Value,
) -> FlowResult<Value> {
    let c = ctx.connectors
        .get(connector)
        .ok_or_else(|| FlowError::UnknownConnector(connector.into()))?;
    c.call(op, params.clone()).await
}

async fn run_action(
    ctx: &ExecCtx<'_>,
    action: &str,
    params: &Value,
) -> FlowResult<Value> {
    let f = ctx.actions
        .get(action)
        .ok_or_else(|| FlowError::UnknownAction(action.into()))?;
    f(params.clone()).await
}

#[allow(clippy::too_many_arguments)]
fn push_record(
    state: &mut RunState,
    record_id: String,
    step: &StepDef,
    parent_record_id: Option<String>,
    branch: Option<String>,
    iteration_index: Option<u32>,
    kind: &'static str,
    detail: Option<String>,
    started: DateTime<Utc>,
    input: Option<Value>,
    output: Option<Value>,
    error: Option<String>,
) {
    let ended = Utc::now();
    let duration_ms = (ended - started).num_milliseconds();
    let status = if error.is_some() { "failed" } else { "success" };
    state.records.push(StepRecord {
        record_id,
        step_id: step.id.clone(),
        parent_record_id,
        kind: kind.to_string(),
        detail,
        branch,
        iteration_index,
        status: status.to_string(),
        input,
        output,
        error,
        started_at: started,
        ended_at: Some(ended),
        duration_ms,
        attempt: 1,
    });
}

/// Resolve an `over`/`from`-style path expression to an array. Returns a
/// `StepFailed` error rooted at the calling step when the expression
/// resolves to something else.
fn resolve_collection(expr: &str, state: &RunState, step_id: &str) -> FlowResult<Vec<Value>> {
    let resolved = substitute(&Value::String(format!("{{{{ {expr} }}}}")), state)?;
    match resolved {
        Value::Array(a) => Ok(a),
        other => Err(FlowError::StepFailed {
            step_id: step_id.to_string(),
            message: format!("`{expr}` must resolve to an array, got {other:?}"),
        }),
    }
}

/// Build a one-frame iter scope binding the named iter variable plus its
/// `<name>_index` counterpart so flows can reference the position. Data
/// primitives (filter/sort/select/…) all use the default name `iter` and
/// `iter_index`; `for_each` uses the user's `as` value (default `iter`).
fn make_iter_frame(name: &str, item: Value, index: usize) -> HashMap<String, Value> {
    let mut frame = HashMap::new();
    frame.insert(name.to_string(), item);
    frame.insert(format!("{name}_index"), Value::from(index));
    frame
}

/// Order two JSON values for `sort`. Numbers compare numerically, strings
/// lexicographically. Null sorts before everything; mixed types fall back
/// to a stable string-based comparison so the sort is total.
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Number(x), Value::Number(y)) => {
            let xf = x.as_f64().unwrap_or(0.0);
            let yf = y.as_f64().unwrap_or(0.0);
            xf.partial_cmp(&yf).unwrap_or(Ordering::Equal)
        }
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => a.to_string().cmp(&b.to_string()),
    }
}

fn step_kind_label(kind: &StepKind) -> &'static str {
    match kind {
        StepKind::Connector { .. } => "connector",
        StepKind::Action { .. } => "action",
        StepKind::If { .. } => "if",
        StepKind::Switch { .. } => "switch",
        StepKind::ForEach { .. } => "for_each",
        StepKind::While { .. } => "while",
        StepKind::Scope => "scope",
        StepKind::Terminate { .. } => "terminate",
        StepKind::SetVar { .. } => "set_var",
        StepKind::AppendToVar { .. } => "append_to_var",
        StepKind::IncrementVar { .. } => "increment_var",
        StepKind::Compose { .. } => "compose",
        StepKind::Select { .. } => "select",
        StepKind::Filter { .. } => "filter",
        StepKind::Join { .. } => "join",
        StepKind::ParseJson { .. } => "parse_json",
        StepKind::GroupBy { .. } => "group_by",
        StepKind::Sort { .. } => "sort",
        StepKind::Dedupe { .. } => "dedupe",
        StepKind::Partition { .. } => "partition",
        StepKind::Take { .. } => "take",
        StepKind::Length { .. } => "length",
    }
}
