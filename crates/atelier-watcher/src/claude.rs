//! Claude Agent SDK scan driver. Spawns the headless read-only scan runner
//! (`runner/src/scan.js`) as `hr-studio` (OAuth subscription, never root), feeds
//! it one prompt on stdin, and streams its NDJSON stdout line by line — same
//! `ScanExec` contract as the Codex driver, so `service.rs` is agnostic. The
//! agent records findings via the MCP `findings_upsert` tool over
//! `…/mcp?scope=surveillance` (read-only whitelist enforced server-side), so
//! findings still flow without any stdout parsing here.
//!
//! Spawn pattern mirrors `atelier_api::routes::agent` (the Studio agent runner):
//! `sudo -n -H -u hr-studio --preserve-env=CLAUDE_CONFIG_DIR -- node scan.js`,
//! own process group so a cancel/timeout SIGKILLs the whole sudo→node→claude
//! subtree. The MCP token goes through stdin (the init JSON), never argv/env —
//! sudo journalises its environment, so an env-passed token would leak.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::runner::ScanExec;

/// Configuration for invoking the Claude scan runner. Populated from env in
/// main.rs. The MCP bearer token is intentionally NOT stored here — it's read
/// from `MCP_TOKEN` at exec time and written to the child's stdin, so it never
/// lands in a `Debug`-printed config nor in sudo's journalised env.
#[derive(Debug, Clone)]
pub struct ClaudeScanConfig {
    /// Node binary path. Default "/usr/bin/node".
    pub node_bin: String,
    /// User the runner runs as (OAuth credentials live in its HOME). Default "hr-studio".
    pub run_as_user: String,
    /// `CLAUDE_CONFIG_DIR` — holds `.credentials.json` (OAuth subscription).
    pub claude_config_dir: String,
    /// Path to the headless scan runner script. Default "/opt/atelier/runner/src/scan.js".
    pub scan_script: String,
    /// MCP endpoint, already including `?scope=surveillance` (the read-only whitelist).
    pub mcp_endpoint: String,
    /// Model id. `None` → the SDK resolves the subscription default (Opus).
    pub model: Option<String>,
    /// Reasoning effort (`low|medium|high|xhigh|max`). Default `max` (deepest
    /// analysis). `None` omits it — required if `model` is set to a tier that
    /// rejects effort (Haiku).
    pub effort: Option<String>,
    /// Per-run wall-clock timeout.
    pub timeout: Duration,
}

impl Default for ClaudeScanConfig {
    fn default() -> Self {
        Self {
            node_bin: "/usr/bin/node".into(),
            run_as_user: "hr-studio".into(),
            claude_config_dir: "/var/lib/hr-studio/.claude".into(),
            scan_script: "/opt/atelier/runner/src/scan.js".into(),
            mcp_endpoint: "http://127.0.0.1:4100/mcp?scope=surveillance".into(),
            model: None,
            effort: Some("max".into()),
            timeout: Duration::from_secs(600),
        }
    }
}

#[derive(Clone)]
pub struct ClaudeRunner {
    cfg: ClaudeScanConfig,
}

impl ClaudeRunner {
    pub fn new(cfg: ClaudeScanConfig) -> Self {
        Self { cfg }
    }

    /// Spawn the scan runner in `work_dir` (the app's `src/`) with `prompt` as
    /// the single user turn. Returns a `ScanExec`. A spawn failure (e.g. the
    /// sudoers rule missing, node absent) is reported via `spawn_error` (not an
    /// `Err`) so the caller records a clean `failed`.
    ///
    /// stdout (NDJSON) is read line by line: each raw line is handed to
    /// `on_line` (the frontend live console parses the `{t:…}` events) and
    /// accumulated. The final `{t:"result", usage:{…}}` line yields the token
    /// counts. The read loop races `cancel` (user stop) + the timeout; either
    /// SIGKILLs the process group. There is no resumable session to preserve
    /// (`persistSession:false` in scan.js), so a hard kill on cancel is clean.
    pub async fn exec(
        &self,
        work_dir: &PathBuf,
        prompt: &str,
        mut cancel: oneshot::Receiver<()>,
        mut on_line: impl FnMut(&str) + Send,
    ) -> ScanExec {
        // sudo → node scan.js, as hr-studio, with only CLAUDE_CONFIG_DIR crossing
        // sudo's env reset (non-secret). Mirrors agent.rs `runner_command`.
        let mut cmd = Command::new("sudo");
        cmd.arg("-n")
            .arg("-H")
            .arg("-u")
            .arg(&self.cfg.run_as_user)
            .arg("--preserve-env=CLAUDE_CONFIG_DIR")
            .arg("--")
            .arg(&self.cfg.node_bin)
            .arg(&self.cfg.scan_script);
        cmd.current_dir(work_dir);
        cmd.env("CLAUDE_CONFIG_DIR", &self.cfg.claude_config_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Own process group so a cancel/timeout SIGKILLs sudo + node + the
        // `claude` native binary the SDK forks (grand-child) in one shot.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ScanExec {
                    exit_ok: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    tokens_in: None,
                    tokens_out: None,
                    spawn_error: Some(format!("spawn scan runner (sudo node) failed: {e}")),
                    cancelled: false,
                };
            }
        };

        // pid doubles as the process-group id (process_group(0) above).
        let child_pid = child.id();

        // Init JSON consumed by scan.js (camelCase keys). The MCP token passes
        // HERE, via stdin — neither Atelier nor sudo journalise the pipe. Single
        // turn: write the init line, then EOF (no further input).
        let init = serde_json::json!({
            "prompt": prompt,
            "cwd": work_dir.to_string_lossy(),
            "model": self.cfg.model,
            "effort": self.cfg.effort,
            "mcpEndpoint": self.cfg.mcp_endpoint,
            "mcpToken": std::env::var("MCP_TOKEN").ok(),
        });
        if let Some(mut stdin) = child.stdin.take() {
            let line = format!("{init}\n");
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                warn!(?e, "failed to write init to scan runner stdin");
            }
            let _ = stdin.flush().await;
            let _ = stdin.shutdown().await; // EOF — single-turn, runner awaits nothing else.
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

        // Read stdout (NDJSON) line by line, racing cancel + timeout against EOF.
        let mut acc = String::new();
        let mut tokens_in: Option<i32> = None;
        let mut tokens_out: Option<i32> = None;
        let mut session_id: Option<String> = None;
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
                            // Capture the session id (for cleanup) + tokens (final result event).
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(l.trim()) {
                                match v.get("t").and_then(|x| x.as_str()) {
                                    Some("system") if session_id.is_none() => {
                                        session_id = v
                                            .get("session_id")
                                            .and_then(|x| x.as_str())
                                            .map(String::from);
                                    }
                                    Some("result") => {
                                        if let Some(u) = v.get("usage") {
                                            let g = |k: &str| {
                                                u.get(k).and_then(|x| x.as_i64()).map(|n| n as i32)
                                            };
                                            tokens_in = g("input_tokens");
                                            tokens_out = g("output_tokens");
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            acc.push_str(&l);
                            acc.push('\n');
                        }
                        Ok(None) => break, // EOF — runner finished.
                        Err(e) => {
                            warn!(?e, "scan runner stdout read error");
                            break;
                        }
                    },
                }
            }
        }

        if cancelled || timed_out {
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

        // Best-effort: delete the SDK session so the scan never pollutes the Studio
        // conversation list. `persistSession:false` is ignored by the native binary
        // 0.3.167 (verified e2e: the session file is written anyway), so we remove it
        // explicitly. Runs for ALL outcomes (success/fail/cancel) since it's before the
        // return branches — covering the cancel case where scan.js was SIGKILLed and
        // could not self-clean.
        if let Some(sid) = session_id.clone() {
            self.cleanup_session(work_dir, &sid).await;
        }

        if timed_out {
            return ScanExec {
                exit_ok: false,
                stdout: acc,
                stderr: format!("scan runner timed out after {:?}", self.cfg.timeout),
                tokens_in: None,
                tokens_out: None,
                spawn_error: None,
                cancelled: false,
            };
        }
        if cancelled {
            return ScanExec {
                exit_ok: false,
                stdout: acc,
                stderr,
                tokens_in: None,
                tokens_out: None,
                spawn_error: None,
                cancelled: true,
            };
        }

        debug!(
            exit = status.as_ref().and_then(|s| s.code()),
            stdout_len = acc.len(),
            "claude scan exec done"
        );
        ScanExec {
            exit_ok: status.map(|s| s.success()).unwrap_or(false),
            stdout: acc,
            stderr,
            tokens_in,
            tokens_out,
            spawn_error: None,
            cancelled: false,
        }
    }

    /// Best-effort removal of a persisted SDK session via a short-lived
    /// `scan.js` invocation in `op:delete` mode (same sudo→node→hr-studio path).
    /// Bounded + errors swallowed: a cleanup failure must never affect the run's
    /// recorded outcome. `work_dir` must match the scan's cwd so deleteSession
    /// resolves the right session-scope directory.
    async fn cleanup_session(&self, work_dir: &PathBuf, session_id: &str) {
        let mut cmd = Command::new("sudo");
        cmd.arg("-n")
            .arg("-H")
            .arg("-u")
            .arg(&self.cfg.run_as_user)
            .arg("--preserve-env=CLAUDE_CONFIG_DIR")
            .arg("--")
            .arg(&self.cfg.node_bin)
            .arg(&self.cfg.scan_script);
        cmd.current_dir(work_dir);
        cmd.env("CLAUDE_CONFIG_DIR", &self.cfg.claude_config_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(?e, "scan session cleanup spawn failed");
                return;
            }
        };
        let init = serde_json::json!({
            "op": "delete",
            "sessionId": session_id,
            "cwd": work_dir.to_string_lossy(),
        });
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(format!("{init}\n").as_bytes()).await;
            let _ = stdin.flush().await;
            let _ = stdin.shutdown().await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(20), child.wait()).await;
    }
}
