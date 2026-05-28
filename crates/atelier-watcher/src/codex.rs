use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::RunKind;
use crate::memory::Memory;

const PROMPT_CODE_REVIEW: &str = include_str!("prompts/code_review.md");
const PROMPT_SUGGESTIONS: &str = include_str!("prompts/suggestions.md");
const PROMPT_SECURITY: &str = include_str!("prompts/security.md");

/// Configuration for invoking the Codex CLI. Populated from env in main.rs.
#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// Binary name or path. Default "codex".
    pub bin: String,
    /// Args before the prompt. Default ["exec", "--sandbox", "read-only"].
    /// Confirmed against codex-cli 0.134: `exec` reads the prompt from stdin
    /// and `-s/--sandbox read-only` is a valid policy. The Atelier MCP server
    /// is registered once in `~/.codex/config.toml` (via `codex mcp add`), not
    /// passed per-invocation.
    pub args: Vec<String>,
    /// Per-run wall-clock timeout.
    pub timeout: Duration,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            bin: "codex".to_string(),
            args: vec!["exec".into(), "--sandbox".into(), "read-only".into()],
            timeout: Duration::from_secs(600),
        }
    }
}

/// Result of a Codex subprocess invocation. The runner does NOT parse findings
/// from stdout — Codex writes them via MCP `findings_upsert`. We observe the DB
/// delta afterwards. This struct only carries process-level signals.
#[derive(Debug, Clone)]
pub struct CodexExec {
    pub exit_ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub tokens_in: Option<i32>,
    pub tokens_out: Option<i32>,
    /// Set when the subprocess could not be launched at all (binary missing).
    pub spawn_error: Option<String>,
}

#[derive(Clone)]
pub struct CodexRunner {
    cfg: CodexConfig,
}

impl CodexRunner {
    pub fn new(cfg: CodexConfig) -> Self {
        Self { cfg }
    }

    /// Build the full prompt for a run from the embedded template + dynamic
    /// context (diff, memory). `diff` is None for a full-codebase review.
    pub fn build_prompt(
        &self,
        slug: &str,
        stack: &str,
        kind: RunKind,
        diff: Option<&str>,
        memory: &[Memory],
    ) -> String {
        let template = match kind {
            RunKind::CodeReview => PROMPT_CODE_REVIEW,
            RunKind::Suggestions => PROMPT_SUGGESTIONS,
            RunKind::Security => PROMPT_SECURITY,
        };
        let categories_block = kind
            .categories()
            .iter()
            .map(|c| format!("- `{c}`"))
            .collect::<Vec<_>>()
            .join("\n");
        let diff_block = match diff {
            Some(d) if !d.trim().is_empty() => {
                format!("Tu revois le DIFF suivant (modifications depuis la dernière review).\nConcentre-toi dessus, mais lis les fichiers complets si besoin pour le contexte :\n\n```diff\n{}\n```", truncate_chars(d, 80_000))
            }
            _ => "Aucun diff fourni — fais une revue du code de l'app dans son répertoire courant.".to_string(),
        };
        let memory_block = format_memory(memory);
        template
            .replace("{{SLUG}}", slug)
            .replace("{{STACK}}", stack)
            .replace("{{CATEGORIES}}", &categories_block)
            .replace("{{DIFF}}", &diff_block)
            .replace("{{MEMORY}}", &memory_block)
    }

    /// Spawn the Codex CLI in `work_dir` with `prompt` on stdin. Returns a
    /// `CodexExec` describing the outcome. A missing binary is reported via
    /// `spawn_error` (not an `Err`) so callers can record a clean `failed` run.
    pub async fn exec(&self, work_dir: &PathBuf, prompt: &str) -> CodexExec {
        let mut cmd = Command::new(&self.cfg.bin);
        cmd.current_dir(work_dir);
        cmd.args(&self.cfg.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return CodexExec {
                    exit_ok: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    tokens_in: None,
                    tokens_out: None,
                    spawn_error: Some(format!("spawn `{}` failed: {e}", self.cfg.bin)),
                };
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            let p = prompt.to_string();
            // Write + drop stdin so Codex sees EOF.
            if let Err(e) = stdin.write_all(p.as_bytes()).await {
                warn!(?e, "failed to write prompt to codex stdin");
            }
            let _ = stdin.shutdown().await;
            drop(stdin);
        }

        let output = match tokio::time::timeout(self.cfg.timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return CodexExec {
                    exit_ok: false,
                    stdout: String::new(),
                    stderr: format!("wait failed: {e}"),
                    tokens_in: None,
                    tokens_out: None,
                    spawn_error: None,
                };
            }
            Err(_) => {
                return CodexExec {
                    exit_ok: false,
                    stdout: String::new(),
                    stderr: format!("codex timed out after {:?}", self.cfg.timeout),
                    tokens_in: None,
                    tokens_out: None,
                    spawn_error: None,
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let (tokens_in, tokens_out) = parse_tokens(&stdout, &stderr);
        debug!(
            exit = output.status.code(),
            stdout_len = stdout.len(),
            "codex exec done"
        );
        CodexExec {
            exit_ok: output.status.success(),
            stdout,
            stderr,
            tokens_in,
            tokens_out,
            spawn_error: None,
        }
    }
}

/// Token accounting from Codex output. `codex exec` prints a trailing
/// `tokens used\n<N>` line (the number may use spaces as thousands separators,
/// e.g. `7 657`). We record that total in `tokens_in` (budget sums in+out, so
/// a single total in one field is enough). Falls back to structured keys if
/// present. Returns None if nothing parses — budget then degrades to 0.
fn parse_tokens(stdout: &str, stderr: &str) -> (Option<i32>, Option<i32>) {
    let haystack = format!("{stdout}\n{stderr}");
    // Primary: codex's "tokens used\n<N>" footer.
    if let Some(idx) = haystack.rfind("tokens used") {
        let rest = &haystack[idx + "tokens used".len()..];
        let mut seen_digit = false;
        let mut digits = String::new();
        for c in rest.chars() {
            if c.is_ascii_digit() {
                seen_digit = true;
                digits.push(c);
            } else if c == ' ' || c == '\u{a0}' || c == '\n' || c == '\r' || c == '\t' {
                // Allow whitespace/newline before and within the number
                // (thousands separators), but stop once digits started and we
                // hit a newline boundary.
                if seen_digit && (c == '\n' || c == '\r') {
                    break;
                }
            } else if seen_digit {
                break;
            }
        }
        if let Ok(n) = digits.parse::<i32>() {
            return (Some(n), None);
        }
    }
    // Fallback: structured keys (in case a future codex/--json emits them).
    let find_after = |needle: &str| -> Option<i32> {
        let i = haystack.find(needle)?;
        let rest = &haystack[i + needle.len()..];
        let d: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        d.parse().ok()
    };
    let tin = find_after("input_tokens").or_else(|| find_after("prompt_tokens"));
    let tout = find_after("output_tokens").or_else(|| find_after("completion_tokens"));
    (tin, tout)
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
        let line = format!(
            "- [{}] {}: {}\n",
            m.kind,
            m.key,
            compact_json(&m.value)
        );
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
