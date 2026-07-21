use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEvent {
    pub t: String,
    #[serde(flatten)]
    pub data: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkerExec {
    pub exit_ok: bool,
    pub cancelled: bool,
    pub failure_reason: Option<String>,
    pub error: Option<String>,
    pub final_report: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub session_id: Option<String>,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct EnginePolicy {
    pub codex_enabled: bool,
    pub auto_enabled: bool,
}

impl EnginePolicy {
    pub fn select(&self, requested: &str, effort: &str) -> &'static str {
        match requested {
            "codex" if self.codex_enabled => "codex",
            "claude" => "claude",
            "auto" if self.auto_enabled && self.codex_enabled && matches!(effort, "xs" | "s") => {
                "codex"
            }
            _ => "claude",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeWorkerEngine {
    pub node_bin: String,
    pub script: PathBuf,
    pub run_as_user: String,
    pub config_dir: PathBuf,
    pub mcp_endpoint: Option<String>,
    pub mcp_token: Option<String>,
    pub model: String,
    pub effort: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct CodexWorkerEngine {
    pub node_bin: String,
    pub script: PathBuf,
    pub run_as_user: String,
    pub codex_home: PathBuf,
    pub model: String,
    pub effort: String,
    pub timeout: Duration,
}

impl CodexWorkerEngine {
    pub async fn exec<F>(
        &self,
        cwd: &Path,
        prompt: &str,
        cancel: CancellationToken,
        on_event: F,
    ) -> WorkerExec
    where
        F: FnMut(&Value) + Send,
    {
        let init = json!({
            "prompt": prompt,
            "cwd": cwd.to_string_lossy(),
            "writeRoot": cwd.to_string_lossy(),
            "model": self.model,
            "effort": self.effort,
        });
        let mut cmd = if self.run_as_user.is_empty() {
            let mut c = Command::new(&self.node_bin);
            c.arg(&self.script);
            c
        } else {
            let mut c = Command::new("sudo");
            c.arg("-n")
                .arg("-H")
                .arg("-u")
                .arg(&self.run_as_user)
                .arg("--preserve-env=CODEX_HOME")
                .arg("--")
                .arg(&self.node_bin)
                .arg(&self.script);
            c
        };
        cmd.current_dir(cwd).env("CODEX_HOME", &self.codex_home);
        run_worker_command(cmd, init, self.timeout, cancel, on_event).await
    }
}

impl ClaudeWorkerEngine {
    pub async fn exec<F>(
        &self,
        cwd: &Path,
        prompt: &str,
        oauth_token: Option<&str>,
        cancel: CancellationToken,
        on_event: F,
    ) -> WorkerExec
    where
        F: FnMut(&Value) + Send,
    {
        let init = json!({
            "prompt": prompt,
            "cwd": cwd.to_string_lossy(),
            "writeRoot": cwd.to_string_lossy(),
            "model": self.model,
            "effort": self.effort,
            "mcpEndpoint": self.mcp_endpoint,
            "mcpToken": self.mcp_token,
            "oauthToken": oauth_token,
        });
        let mut cmd = if self.run_as_user.is_empty() {
            let mut c = Command::new(&self.node_bin);
            c.arg(&self.script);
            c
        } else {
            let mut c = Command::new("sudo");
            c.arg("-n")
                .arg("-H")
                .arg("-u")
                .arg(&self.run_as_user)
                .arg("--preserve-env=CLAUDE_CONFIG_DIR")
                .arg("--")
                .arg(&self.node_bin)
                .arg(&self.script);
            c
        };
        cmd.current_dir(cwd)
            .env("CLAUDE_CONFIG_DIR", &self.config_dir);
        let out = run_worker_command(cmd, init, self.timeout, cancel, on_event).await;
        // worker.js auto-nettoie sa session en fin de run normal ; un run tué
        // (timeout/cancel → SIGKILL groupe) ou en échec laisse sa session SDK
        // persistée derrière lui et polluerait la liste du Studio.
        if (out.cancelled || out.failure_reason.is_some())
            && let Some(sid) = out.session_id.clone()
        {
            self.cleanup_session(cwd, &sid).await;
        }
        out
    }

    /// Suppression best-effort d'une session SDK persistée (patron
    /// watcher/claude.rs::cleanup_session) : re-spawn de worker.js en mode
    /// `op:delete` (contrat runner : il émet `{t:'done'}` et sort en 0).
    /// Bornée + erreurs avalées — un échec de cleanup ne doit jamais changer
    /// l'issue enregistrée du run.
    pub async fn cleanup_session(&self, work_dir: &Path, session_id: &str) {
        let mut cmd = if self.run_as_user.is_empty() {
            let mut c = Command::new(&self.node_bin);
            c.arg(&self.script);
            c
        } else {
            let mut c = Command::new("sudo");
            c.arg("-n")
                .arg("-H")
                .arg("-u")
                .arg(&self.run_as_user)
                .arg("--preserve-env=CLAUDE_CONFIG_DIR")
                .arg("--")
                .arg(&self.node_bin)
                .arg(&self.script);
            c
        };
        cmd.current_dir(work_dir)
            .env("CLAUDE_CONFIG_DIR", &self.config_dir);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "pilot session cleanup spawn failed");
                return;
            }
        };
        let init = json!({
            "op": "delete",
            "sessionId": session_id,
            "cwd": work_dir.to_string_lossy(),
        });
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(format!("{init}\n").as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        if tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .is_err()
        {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

async fn run_worker_command<F>(
    mut cmd: Command,
    init: Value,
    timeout: Duration,
    cancel: CancellationToken,
    mut on_event: F,
) -> WorkerExec
where
    F: FnMut(&Value) + Send,
{
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return WorkerExec {
                failure_reason: Some("spawn_error".into()),
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
    };
    let pid = child.id();
    // Draine stderr en tâche de fond : le pipe était ouvert mais jamais lu →
    // un worker verbeux (> 64 Ko de stderr) se bloquait en écriture et le run
    // restait pendu jusqu'au timeout. On ne garde que la FIN (~16 Ko) : la
    // cause d'un crash (node/sudo/module manquant) est en queue de flux.
    let stderr_task = child.stderr.take().map(|err| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            let mut tail = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 16 * 1024 {
                    let mut cut = tail.len() - 12 * 1024;
                    while !tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    tail.drain(..cut);
                }
            }
            tail
        })
    });
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(init.to_string().as_bytes()).await.is_err()
            || stdin.write_all(b"\n").await.is_err()
        {
            kill_group(pid, &mut child).await;
            let tail = join_stderr(stderr_task).await;
            return WorkerExec {
                failure_reason: Some("spawn_error".into()),
                error: Some(with_stderr_tail("stdin worker indisponible".into(), &tail)),
                ..Default::default()
            };
        }
        let _ = stdin.shutdown().await;
    }
    let Some(stdout) = child.stdout.take() else {
        kill_group(pid, &mut child).await;
        let tail = join_stderr(stderr_task).await;
        return WorkerExec {
            failure_reason: Some("spawn_error".into()),
            error: Some(with_stderr_tail("stdout worker indisponible".into(), &tail)),
            ..Default::default()
        };
    };
    let mut reader = BufReader::new(stdout).lines();
    let mut out = WorkerExec::default();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                out.cancelled = true;
                out.failure_reason = Some("cancelled".into());
                kill_group(pid, &mut child).await;
                break;
            }
            _ = &mut deadline => {
                out.failure_reason = Some("timeout".into());
                out.error = Some(format!("worker timeout après {}s", timeout.as_secs()));
                kill_group(pid, &mut child).await;
                break;
            }
            line = reader.next_line() => match line {
                Ok(Some(line)) => {
                    if line.trim().is_empty() { continue; }
                    out.lines.push(line.clone());
                    if out.lines.len() > 1000 { out.lines.drain(0..200); }
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        on_event(&v);
                        match v.get("t").and_then(Value::as_str).unwrap_or("") {
                            "system" => out.session_id = v.get("session_id").and_then(Value::as_str).map(String::from),
                            "final_report" => out.final_report = v.get("text").and_then(Value::as_str).map(String::from),
                            "result" => {
                                out.tokens_in = v.pointer("/usage/input_tokens").and_then(Value::as_i64);
                                out.tokens_out = v.pointer("/usage/output_tokens").and_then(Value::as_i64);
                                if v.get("is_error").and_then(Value::as_bool) == Some(true) { out.failure_reason.get_or_insert("agent_error".into()); }
                            }
                            "error" => {
                                let code = v.get("code").and_then(Value::as_str).unwrap_or("agent_error");
                                out.failure_reason = Some(match code { "sdk_auth_failed" => "sdk_auth_failed", "mcp_auth_failed" => "mcp_error", _ => "agent_error" }.into());
                                out.error = v.get("message").and_then(Value::as_str).map(String::from);
                            }
                            "done" => { out.exit_ok = v.get("exit_ok").and_then(Value::as_bool).unwrap_or(true); }
                            _ => {}
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => { out.failure_reason = Some("agent_error".into()); out.error = Some(e.to_string()); break; }
            }
        }
    }
    let status = child.wait().await.ok();
    let stderr_tail = join_stderr(stderr_task).await;
    if !out.cancelled && out.failure_reason.is_none() {
        out.exit_ok = out.exit_ok && status.map(|s| s.success()).unwrap_or(false);
        if !out.exit_ok || out.final_report.is_none() {
            out.failure_reason = Some("agent_error".into());
            out.error
                .get_or_insert("worker terminé sans rapport final valide".into());
        }
    }
    // En échec, le tail stderr est souvent la seule trace exploitable (stack
    // node, refus sudoers…) — on l'annexe à l'erreur remontée/persistée.
    if !stderr_tail.trim().is_empty()
        && matches!(
            out.failure_reason.as_deref(),
            Some("spawn_error" | "agent_error" | "timeout")
        )
    {
        let base = out.error.take().unwrap_or_default();
        out.error = Some(with_stderr_tail(base, &stderr_tail));
    }
    out
}

/// Joint la tâche de drain stderr avec un budget court : après kill/EOF le
/// pipe se ferme tout seul, on ne veut jamais pendre sur un cleanup.
async fn join_stderr(task: Option<tokio::task::JoinHandle<String>>) -> String {
    match task {
        Some(h) => tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default(),
        None => String::new(),
    }
}

fn with_stderr_tail(base: String, tail: &str) -> String {
    let mut tail = tail.trim();
    if tail.is_empty() {
        return base;
    }
    // Borne l'extrait embarqué dans l'erreur (elle repart dans le prompt de
    // retry et en DB, tronquée à 4000 chars là-bas) — on garde la fin.
    if tail.len() > 4096 {
        let mut cut = tail.len() - 4096;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail = &tail[cut..];
    }
    if base.is_empty() {
        format!("stderr: {tail}")
    } else {
        format!("{base}\nstderr: {tail}")
    }
}

async fn kill_group(pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::EnginePolicy;

    #[test]
    fn engine_selection_is_conservative() {
        let disabled = EnginePolicy {
            codex_enabled: false,
            auto_enabled: false,
        };
        assert_eq!(disabled.select("codex", "xs"), "claude");
        assert_eq!(disabled.select("auto", "xs"), "claude");

        let enabled = EnginePolicy {
            codex_enabled: true,
            auto_enabled: true,
        };
        assert_eq!(enabled.select("codex", "xl"), "codex");
        assert_eq!(enabled.select("auto", "xs"), "codex");
        assert_eq!(enabled.select("auto", "s"), "codex");
        assert_eq!(enabled.select("auto", "m"), "claude");
        assert_eq!(enabled.select("unexpected", "xs"), "claude");
    }
}
