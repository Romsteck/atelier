//! Flow definition types and TOML parser.
//!
//! Definitions live in source under `{slug}/src/flows/*.toml` and use a
//! **flat** layout: every step is a top-level `[[steps]]` entry referencing
//! its `parent` by id. The engine reconstructs the tree at load time. This
//! keeps the on-disk format easy to scan, lint, and (later) generate from a
//! visual editor.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::{FlowError, FlowResult};

/// Top-level flow definition parsed from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowDef {
    pub name: String,

    #[serde(default)]
    pub description: Option<String>,

    /// Schema for the input the flow expects when triggered. Free-form JSON
    /// for v1 — formal validation comes later.
    #[serde(default)]
    pub input: Option<serde_json::Value>,

    /// When set, the run's final output is the output of the named step
    /// rather than the last top-level step. Lets flows that end in a control
    /// primitive (e.g. `if`, which wraps its result in `{branch, output}`)
    /// surface the inner value without an extra `compose` terminal step.
    #[serde(default)]
    pub output_step: Option<String>,

    #[serde(default, rename = "steps")]
    pub steps: Vec<StepDef>,
}

/// A single step in a flow. Steps reference a parent by id when they live
/// inside a control primitive (e.g. inside a `for_each` body or a `then`
/// branch of an `if`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    pub id: String,

    /// Parent step id (None for top-level siblings).
    #[serde(default)]
    pub parent: Option<String>,

    /// For nested steps inside branched primitives — `then` / `else` /
    /// `case:<name>` for `if`/`switch`, `body` (default) for `for_each`/
    /// `while`/`scope`. Engine ignores when not applicable.
    #[serde(default)]
    pub parent_branch: Option<String>,

    /// Sibling-level dependencies. The executor blocks the step until all
    /// referenced step ids have finished.
    #[serde(default)]
    pub needs: Vec<String>,

    #[serde(flatten)]
    pub kind: StepKind,

    /// Free-form params evaluated against the run context. Expression
    /// substitution (`{{ ... }}`) happens at execution time.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Discriminator for the kind of work a step performs.
///
/// `kind = "connector"` → calls a connector op
/// `kind = "action"`    → calls a registered Rust action
/// otherwise            → a built-in primitive (`if`, `for_each`, …)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepKind {
    /// Call a registered connector operation.
    Connector {
        connector: String,
        op: String,
    },
    /// Call a registered Rust action by name.
    Action {
        action: String,
    },
    /// `if` primitive: branches on a `cond` expression.
    If {
        cond: String,
    },
    /// `switch` primitive: dispatches on an `on` expression.
    Switch {
        on: String,
    },
    /// `cond_select` primitive: evaluates a list of `when` arms (first
    /// matching `cond` wins) and returns its `value` directly — no
    /// `{branch, output}` wrapper. Ergonomic alternative to a terminal `if`
    /// when the flow's job is "pick a value out of N cases".
    CondSelect {
        #[serde(default)]
        when: Vec<CondSelectArm>,
        #[serde(default)]
        default: Option<serde_json::Value>,
    },
    /// `for_each` primitive: iterates over `over`, exposing each item as
    /// `as` (default `iter`).
    ForEach {
        over: String,
        #[serde(default = "default_iter_var")]
        r#as: String,
        #[serde(default = "default_concurrency")]
        concurrency: u32,
    },
    /// `while` primitive: loops while `cond` is truthy. `max_iterations`
    /// is mandatory to prevent runaway loops.
    While {
        cond: String,
        max_iterations: u32,
    },
    /// `scope` primitive: groups steps with try/catch/finally semantics.
    Scope,
    /// Early exit — `status` is `success`/`failed`/`cancelled`.
    Terminate {
        #[serde(default = "default_terminate_status")]
        status: String,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Variable mutations.
    SetVar { name: String, value: serde_json::Value },
    AppendToVar { name: String, value: serde_json::Value },
    IncrementVar {
        name: String,
        #[serde(default = "default_increment")]
        by: serde_json::Value,
    },
    /// Data operations.
    Compose { value: serde_json::Value },
    Select { from: String, project: serde_json::Value },
    Filter { from: String, cond: String },
    Join { from: String, separator: String },
    ParseJson { from: String },
    GroupBy { from: String, by: String },
    Sort { from: String, by: String, #[serde(default)] desc: bool },
    Dedupe { from: String, #[serde(default)] by: Option<String> },
    Partition { from: String, cond: String },
    /// Take the first `count` items of an array — equivalent to a slice/limit.
    Take { from: String, count: u32 },
    /// Length of an array, string or object. `null` resolves to 0.
    Length { from: String },
}

/// One arm of a `cond_select` step. `value` follows the same substitution
/// rules as `compose.value`: bare JSON literal or `{{ … }}` placeholder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CondSelectArm {
    pub cond: String,
    pub value: serde_json::Value,
}

fn default_iter_var() -> String { "iter".into() }
fn default_concurrency() -> u32 { 1 }
fn default_terminate_status() -> String { "success".into() }
fn default_increment() -> serde_json::Value { serde_json::json!(1) }

/// Parse a TOML string into a `FlowDef`. Performs no semantic validation
/// beyond serde structural checks — the engine validates references at
/// load time.
pub fn parse_flow_toml(src: &str) -> FlowResult<FlowDef> {
    toml::from_str(src).map_err(|e| FlowError::InvalidDefinition(e.to_string()))
}

/// Build a parent → children index from the flat steps list. Useful for the
/// executor to walk the tree, and for the persistence layer to compute the
/// `parent_step_id` column.
pub fn index_children(flow: &FlowDef) -> BTreeMap<Option<String>, Vec<&StepDef>> {
    let mut idx: BTreeMap<Option<String>, Vec<&StepDef>> = BTreeMap::new();
    for step in &flow.steps {
        idx.entry(step.parent.clone()).or_default().push(step);
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_flow() {
        let toml = r#"
            name = "hello"

            [[steps]]
            id = "compose_value"
            kind = "compose"
            value = "world"
        "#;
        let def = parse_flow_toml(toml).expect("parse");
        assert_eq!(def.name, "hello");
        assert_eq!(def.steps.len(), 1);
        assert_eq!(def.steps[0].id, "compose_value");
    }

    #[test]
    fn parses_nested_for_each_with_parent_ref() {
        let toml = r#"
            name = "scan"

            [[steps]]
            id = "loop"
            kind = "for_each"
            over = "{{ input.items }}"

            [[steps]]
            id = "inner"
            parent = "loop"
            kind = "compose"
            value = "{{ @iter }}"
        "#;
        let def = parse_flow_toml(toml).expect("parse");
        let idx = index_children(&def);
        assert_eq!(idx.get(&None).unwrap().len(), 1);
        assert_eq!(idx.get(&Some("loop".into())).unwrap().len(), 1);
    }
}
