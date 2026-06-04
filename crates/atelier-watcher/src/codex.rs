use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::memory::Memory;
use crate::scandef::{Gate, ScanDef};
use crate::MAX_OPEN_FINDINGS;

/// Configuration for invoking the Codex CLI. Populated from env in main.rs.
#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// Binary name or path. Default "codex".
    pub bin: String,
    /// Args before the prompt. Default ["exec", "--json", "--sandbox", "read-only"].
    /// Confirmed against codex-cli 0.134: `exec` reads the prompt from stdin
    /// and `-s/--sandbox read-only` is a valid policy. `--json` makes Codex emit
    /// one JSONL event per line on stdout (flushed per event) — required for the
    /// live transcript: human mode block-buffers stdout, so nothing streams. The
    /// Atelier MCP server is registered once in `~/.codex/config.toml` (via
    /// `codex mcp add`), not passed per-invocation.
    pub args: Vec<String>,
    /// Per-run wall-clock timeout.
    pub timeout: Duration,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            bin: "codex".to_string(),
            args: vec![
                "exec".into(),
                "--json".into(),
                "--sandbox".into(),
                "read-only".into(),
            ],
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
    /// True when the run was killed via its cancel oneshot (user-requested
    /// stop). Distinct from `failed` so the caller records a clean `cancelled`.
    pub cancelled: bool,
}

#[derive(Clone)]
pub struct CodexRunner {
    cfg: CodexConfig,
}

impl CodexRunner {
    pub fn new(cfg: CodexConfig) -> Self {
        Self { cfg }
    }

    /// Build the full prompt for a run from the app's scan definition (its
    /// agent-authored `prompt` template) + dynamic context (diff/data, memory).
    /// `diff` is None for a full-codebase review or a data-gated scan.
    /// `open_now` is the count of currently-open findings; it's injected so Codex
    /// limits itself to the most important issues within the remaining budget.
    pub fn build_prompt(
        &self,
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
        // scan queries the DB itself via `pm_query`); for code-gated scans it's
        // the git diff (or a full-review fallback). The scan-specific framing of
        // a data scan lives in its own prompt body, not here.
        let diff_block = if scan.gate == Gate::Data {
            "Ce scan est piloté par les DONNÉES (pas de diff de code). Identifie toi-même \
             le matériel à analyser en interrogeant la base avec `pm_query` (SELECT read-only) \
             — le watermark de fraîcheur est en mémoire `last_run` (clé `scan_watermark`)."
                .to_string()
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

    /// Spawn the Codex CLI in `work_dir` with `prompt` on stdin. Returns a
    /// `CodexExec` describing the outcome. A missing binary is reported via
    /// `spawn_error` (not an `Err`) so callers can record a clean `failed` run.
    ///
    /// stdout is read line by line: each line is handed to `on_line` (for live
    /// streaming to the UI) and accumulated so token parsing still sees the
    /// whole output. The read loop races against `cancel` (a oneshot fired on
    /// user stop) and the configured timeout; either kills the child and
    /// returns early. The caller keeps the matching `Sender` alive for the run,
    /// so `cancel` only resolves on an explicit stop.
    pub async fn exec(
        &self,
        work_dir: &PathBuf,
        prompt: &str,
        mut cancel: oneshot::Receiver<()>,
        mut on_line: impl FnMut(&str) + Send,
    ) -> CodexExec {
        let mut cmd = Command::new(&self.cfg.bin);
        cmd.current_dir(work_dir);
        cmd.args(&self.cfg.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Own process group so a cancel/timeout can kill Codex AND the shells it
        // spawns in one shot. A plain SIGKILL to the direct child orphans those
        // children — they linger for seconds until their pipe breaks.
        #[cfg(unix)]
        cmd.process_group(0);

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
                    cancelled: false,
                };
            }
        };

        // pid doubles as the process-group id (process_group(0) above), so
        // `-pid` targets the whole group on kill.
        let child_pid = child.id();

        if let Some(mut stdin) = child.stdin.take() {
            let p = prompt.to_string();
            // Write + drop stdin so Codex sees EOF.
            if let Err(e) = stdin.write_all(p.as_bytes()).await {
                warn!(?e, "failed to write prompt to codex stdin");
            }
            let _ = stdin.shutdown().await;
            drop(stdin);
        }

        // Drain stderr in the background so the pipe never blocks the child.
        let stderr_task = child.stderr.take().map(|err| {
            tokio::spawn(async move {
                let mut buf = String::new();
                let _ = BufReader::new(err).read_to_string(&mut buf).await;
                buf
            })
        });

        // Read stdout line by line, racing cancellation + timeout against EOF.
        let mut acc = String::new();
        let mut cancelled = false;
        let mut timed_out = false;
        if let Some(out) = child.stdout.take() {
            let mut lines = BufReader::new(out).lines();
            let deadline = tokio::time::sleep(self.cfg.timeout);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut cancel => { cancelled = true; break; }
                    _ = &mut deadline => { timed_out = true; break; }
                    next = lines.next_line() => match next {
                        Ok(Some(l)) => {
                            on_line(&l);
                            acc.push_str(&l);
                            acc.push('\n');
                        }
                        Ok(None) => break, // EOF — process finished writing.
                        Err(e) => {
                            warn!(?e, "codex stdout read error");
                            break;
                        }
                    },
                }
            }
        }

        if cancelled || timed_out {
            // SIGKILL the whole group (codex + its child shells), then the
            // direct child as a fallback.
            #[cfg(unix)]
            if let Some(pid) = child_pid {
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            }
            let _ = child.start_kill();
        }
        let status = child.wait().await.ok();
        let stderr = match stderr_task {
            Some(h) => h.await.unwrap_or_default(),
            None => String::new(),
        };

        if timed_out {
            return CodexExec {
                exit_ok: false,
                stdout: acc,
                stderr: format!("codex timed out after {:?}", self.cfg.timeout),
                tokens_in: None,
                tokens_out: None,
                spawn_error: None,
                cancelled: false,
            };
        }
        if cancelled {
            return CodexExec {
                exit_ok: false,
                stdout: acc,
                stderr,
                tokens_in: None,
                tokens_out: None,
                spawn_error: None,
                cancelled: true,
            };
        }

        let (tokens_in, tokens_out) = parse_tokens(&acc, &stderr);
        debug!(
            exit = status.as_ref().and_then(|s| s.code()),
            stdout_len = acc.len(),
            "codex exec done"
        );
        CodexExec {
            exit_ok: status.map(|s| s.success()).unwrap_or(false),
            stdout: acc,
            stderr,
            tokens_in,
            tokens_out,
            spawn_error: None,
            cancelled: false,
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
