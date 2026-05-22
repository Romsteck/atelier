//! Semantic validation of a parsed `FlowDef`.
//!
//! `parse_flow_toml` only performs structural (serde) checks. This pass
//! catches the mistakes that would otherwise fail silently or surface as an
//! opaque `Internal` error at run time:
//!
//! - duplicate step ids (→ `step_outputs` collisions, ambiguous records)
//! - `parent` / `needs` / `output_step` pointing at a non-existent step
//! - a mistyped `parent_branch` on an `if` child (→ branch silently skipped)
//! - `needs` cycles (→ would deadlock the topological order)
//! - step kinds parsed but not implemented by the executor (`switch`,
//!   `scope`, `terminate`) — reject loudly instead of a runtime 500
//! - `while.max_iterations == 0` (→ misleading "exceeded" error)
//!
//! Called by `FlowEngineBuilder::build` and by the daemon's registry loader.
//! NOT called by `parse_flow_toml` — the Atelier viewer must still be able to
//! display structurally-valid-but-semantically-broken flows.

use std::collections::{HashMap, HashSet};

use crate::definition::{FlowDef, StepDef, StepKind};
use crate::error::{FlowError, FlowResult};

/// Run all semantic checks on a flow definition. Returns the first problem
/// found as an `InvalidDefinition` error.
pub fn validate_flow(flow: &FlowDef) -> FlowResult<()> {
    let mut ids: HashSet<&str> = HashSet::with_capacity(flow.steps.len());
    for step in &flow.steps {
        if !ids.insert(step.id.as_str()) {
            return Err(invalid(format!("duplicate step id `{}`", step.id)));
        }
    }
    let by_id: HashMap<&str, &StepDef> =
        flow.steps.iter().map(|s| (s.id.as_str(), s)).collect();

    for step in &flow.steps {
        match &step.kind {
            StepKind::Switch { .. } => return Err(unimplemented_kind(&step.id, "switch")),
            StepKind::Scope => return Err(unimplemented_kind(&step.id, "scope")),
            StepKind::Terminate { .. } => return Err(unimplemented_kind(&step.id, "terminate")),
            StepKind::While { max_iterations, .. } => {
                if *max_iterations == 0 {
                    return Err(invalid(format!(
                        "step `{}`: while.max_iterations must be > 0",
                        step.id
                    )));
                }
            }
            _ => {}
        }

        if let Some(parent) = &step.parent {
            if !ids.contains(parent.as_str()) {
                return Err(invalid(format!(
                    "step `{}` references unknown parent `{}`",
                    step.id, parent
                )));
            }
            // A child of an `if` must land on `then` / `else`; a typo would
            // otherwise drop the child silently (executor compares the
            // branch string and just skips a non-match).
            if let Some(p) = by_id.get(parent.as_str()) {
                if matches!(p.kind, StepKind::If { .. }) {
                    if let Some(branch) = &step.parent_branch {
                        if branch != "then" && branch != "else" {
                            return Err(invalid(format!(
                                "step `{}`: parent_branch `{}` invalid for an `if` \
                                 (expected `then` or `else`)",
                                step.id, branch
                            )));
                        }
                    }
                }
            }
        }

        for need in &step.needs {
            if need == &step.id {
                return Err(invalid(format!("step `{}` needs itself", step.id)));
            }
            if !ids.contains(need.as_str()) {
                return Err(invalid(format!(
                    "step `{}` needs unknown step `{}`",
                    step.id, need
                )));
            }
        }
    }

    if let Some(out) = &flow.output_step {
        if !ids.contains(out.as_str()) {
            return Err(invalid(format!(
                "output_step `{}` does not match any step id",
                out
            )));
        }
    }

    detect_needs_cycle(flow, &by_id)?;
    Ok(())
}

/// Iterative tri-colour DFS over the `needs` graph. Assumes every `needs`
/// target already resolves (checked above).
fn detect_needs_cycle(flow: &FlowDef, by_id: &HashMap<&str, &StepDef>) -> FlowResult<()> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let mut mark: HashMap<&str, Mark> =
        flow.steps.iter().map(|s| (s.id.as_str(), Mark::White)).collect();

    for start in flow.steps.iter().map(|s| s.id.as_str()) {
        if mark.get(start).copied() != Some(Mark::White) {
            continue;
        }
        // Stack of (node, next-needs-index-to-visit).
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        mark.insert(start, Mark::Grey);
        while let Some(&(node, idx)) = stack.last() {
            let needs = by_id.get(node).map(|s| s.needs.as_slice()).unwrap_or(&[]);
            if idx < needs.len() {
                stack.last_mut().unwrap().1 += 1;
                let next = needs[idx].as_str();
                match mark.get(next).copied() {
                    Some(Mark::Grey) => {
                        return Err(invalid(format!(
                            "`needs` cycle detected involving step `{next}`"
                        )));
                    }
                    Some(Mark::White) => {
                        mark.insert(next, Mark::Grey);
                        stack.push((next, 0));
                    }
                    _ => {}
                }
            } else {
                mark.insert(node, Mark::Black);
                stack.pop();
            }
        }
    }
    Ok(())
}

fn invalid(msg: String) -> FlowError {
    FlowError::InvalidDefinition(msg)
}

fn unimplemented_kind(step_id: &str, kind: &str) -> FlowError {
    FlowError::InvalidDefinition(format!(
        "step `{step_id}` uses kind `{kind}` which is not implemented yet"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::parse_flow_toml;

    fn flow(toml: &str) -> FlowDef {
        parse_flow_toml(toml).expect("parse")
    }

    #[test]
    fn accepts_a_well_formed_flow() {
        let f = flow(
            r#"
            name = "ok"
            [[steps]]
            id = "a"
            kind = "compose"
            value = "x"
            [[steps]]
            id = "b"
            kind = "compose"
            value = "y"
            needs = ["a"]
        "#,
        );
        assert!(validate_flow(&f).is_ok());
    }

    #[test]
    fn rejects_duplicate_ids() {
        let f = flow(
            r#"
            name = "dup"
            [[steps]]
            id = "a"
            kind = "compose"
            value = "x"
            [[steps]]
            id = "a"
            kind = "compose"
            value = "y"
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("duplicate step id"), "{e}");
    }

    #[test]
    fn rejects_dangling_parent() {
        let f = flow(
            r#"
            name = "dangle"
            [[steps]]
            id = "child"
            parent = "ghost"
            kind = "compose"
            value = "x"
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("unknown parent"), "{e}");
    }

    #[test]
    fn rejects_dangling_needs() {
        let f = flow(
            r#"
            name = "n"
            [[steps]]
            id = "a"
            kind = "compose"
            value = "x"
            needs = ["ghost"]
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("needs unknown step"), "{e}");
    }

    #[test]
    fn rejects_needs_cycle() {
        let f = flow(
            r#"
            name = "cycle"
            [[steps]]
            id = "a"
            kind = "compose"
            value = "x"
            needs = ["b"]
            [[steps]]
            id = "b"
            kind = "compose"
            value = "y"
            needs = ["a"]
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("cycle"), "{e}");
    }

    #[test]
    fn rejects_unimplemented_kind() {
        let f = flow(
            r#"
            name = "sw"
            [[steps]]
            id = "s"
            kind = "switch"
            on = "{{ input.x }}"
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("switch") && e.contains("not implemented"), "{e}");
    }

    #[test]
    fn rejects_zero_max_iterations() {
        let f = flow(
            r#"
            name = "w"
            [[steps]]
            id = "loop"
            kind = "while"
            cond = "true"
            max_iterations = 0
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("max_iterations"), "{e}");
    }

    #[test]
    fn rejects_bad_if_branch() {
        let f = flow(
            r#"
            name = "iff"
            [[steps]]
            id = "gate"
            kind = "if"
            cond = "true"
            [[steps]]
            id = "kid"
            parent = "gate"
            parent_branch = "thenn"
            kind = "compose"
            value = "x"
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("parent_branch"), "{e}");
    }

    #[test]
    fn rejects_dangling_output_step() {
        let f = flow(
            r#"
            name = "o"
            output_step = "ghost"
            [[steps]]
            id = "a"
            kind = "compose"
            value = "x"
        "#,
        );
        let e = validate_flow(&f).unwrap_err().to_string();
        assert!(e.contains("output_step"), "{e}");
    }
}
