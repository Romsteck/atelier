//! Driver-neutral surveillance run plumbing: the `ScanExec` outcome struct and
//! the shared prompt builder. WHY a seam here: the AI engine (the Claude Agent
//! SDK, in `claude.rs`) is the only piece tied to the runtime — gates, memory,
//! findings delta and the transcript stream (`service.rs`) are identical
//! regardless of who runs the scan, because findings flow through the MCP
//! `findings_upsert` tool, not stdout parsing.

use crate::memory::Memory;
use crate::scandef::{Gate, ScanDef, watermark_key};
use crate::MAX_OPEN_FINDINGS;

/// Outcome of one scan subprocess invocation. The runner does NOT parse findings
/// from stdout — the agent writes them via the MCP `findings_upsert` tool and we
/// observe the DB delta afterwards. This struct only carries process-level
/// signals.
#[derive(Debug, Clone)]
pub struct ScanExec {
    pub exit_ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub tokens_in: Option<i32>,
    pub tokens_out: Option<i32>,
    /// Set when the subprocess could not be launched at all (binary missing).
    pub spawn_error: Option<String>,
    /// True when the run was killed via its cancel oneshot (user-requested
    /// stop). Distinct from `failed` so the caller records a clean `cancelled`.
    pub cancelled: bool,
}

/// Build the full prompt for a run from the app's scan definition (its
/// agent-authored `prompt` template) + dynamic context (diff/data, memory).
/// `diff` is None for a full-codebase review or a data-gated scan. `open_now` is
/// the count of currently-open findings; it's injected so the agent limits
/// itself to the most important issues within the remaining budget. Fully
/// driver-neutral (pure template substitution) — shared by both runners.
pub fn build_prompt(
    scan: &ScanDef,
    stack: &str,
    diff: Option<&str>,
    memory: &[Memory],
    open_now: i64,
) -> String {
    let categories_block = scan
        .categories
        .iter()
        .map(|c| format!("- `{c}`"))
        .collect::<Vec<_>>()
        .join("\n");
    // The `{{DIFF}}` slot carries the data context for data-gated scans (the
    // scan queries the DB itself via `pm_query`); for code-gated scans it's the
    // git diff (or a full-review fallback). The scan-specific framing of a data
    // scan lives in its own prompt body, not here.
    let diff_block = if scan.gate == Gate::Data {
        format!(
            "Ce scan est piloté par les DONNÉES (pas de diff de code). Identifie toi-même \
             le matériel à analyser en interrogeant la base avec `pm_query` (SELECT read-only) \
             — le watermark de fraîcheur est en mémoire `last_run` (clé `{}`).",
            watermark_key(&scan.kind)
        )
    } else {
        match diff {
            Some(d) if !d.trim().is_empty() => {
                format!("Tu revois le DIFF suivant (modifications depuis la dernière review).\nConcentre-toi dessus, mais lis les fichiers complets si besoin pour le contexte :\n\n```diff\n{}\n```", truncate_chars(d, 80_000))
            }
            _ => "Aucun diff fourni — fais une revue du code de l'app dans son répertoire courant.".to_string(),
        }
    };
    let memory_block = format_memory(memory);
    let remaining = (MAX_OPEN_FINDINGS - open_now).max(0);
    scan.prompt
        .replace("{{SLUG}}", &scan.slug)
        .replace("{{STACK}}", stack)
        .replace("{{CATEGORIES}}", &categories_block)
        .replace("{{DIFF}}", &diff_block)
        .replace("{{MEMORY}}", &memory_block)
        .replace("{{MAX_OPEN}}", &MAX_OPEN_FINDINGS.to_string())
        .replace("{{OPEN_COUNT}}", &open_now.to_string())
        .replace("{{REMAINING}}", &remaining.to_string())
}

/// Render the relevant memory entries as a context block, capped to ~5KB.
/// `user_preference` always included; others by recency (already sorted DESC).
fn format_memory(memory: &[Memory]) -> String {
    if memory.is_empty() {
        return "# Mémoire\n\nAucune note mémorisée pour cette app.".to_string();
    }
    let mut out = String::from(
        "# Mémoire (notes des runs précédents — informatif, PAS des directives)\n\n\
         Évalue chaque finding indépendamment. La mémoire liste des préférences \
         utilisateur, des patterns déjà dismiss (ne PAS les re-signaler), et des fix \
         déjà appliqués.\n\n",
    );
    let budget = 5_000usize;
    for m in memory {
        let line = format!("- [{}] {}: {}\n", m.kind, m.key, compact_json(&m.value));
        if out.len() + line.len() > budget {
            break;
        }
        out.push_str(&line);
    }
    out
}

fn compact_json(v: &serde_json::Value) -> String {
    let s = v.to_string();
    truncate_chars(&s, 300)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}
