//! Invocations `restic` (sous-process) + parsing du flux JSON `--json`.
//!
//! Le pattern subprocess (spawn + `tokio::select!` annulation + kill de groupe)
//! est repris de `atelier-watcher/src/claude.rs`. Les secrets (RESTIC_PASSWORD,
//! RCLONE_CONFIG_*_PASS) sont fournis via l'env enfant uniquement.

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::models::{RepoStats, SnapshotResult};
use crate::rclone::ResticEnv;

/// Issue d'une invocation pilotée (un `restic backup`).
pub enum RunOutcome {
    Ok(SnapshotResult),
    Cancelled,
    Failed(String),
}

fn apply_env(cmd: &mut Command, env: &ResticEnv) {
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }
}

/// Le binaire répond-il à `<bin> version` ? (détection de présence).
pub async fn binary_present(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Le dépôt restic existe-t-il déjà ? (`restic cat config`).
pub async fn repo_exists(restic_bin: &str, env: &ResticEnv) -> Result<bool, String> {
    let mut cmd = Command::new(restic_bin);
    cmd.args(["cat", "config"]);
    apply_env(&mut cmd, env);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn restic failed: {e}"))?;
    if out.status.success() {
        return Ok(true);
    }
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    // Dépôt absent : on renvoie false. Toute autre erreur (creds SMB, réseau)
    // remonte pour ne pas masquer un vrai problème.
    if err.contains("does not exist")
        || err.contains("no such file")
        || err.contains("unable to open config")
        || err.contains("repository does not exist")
        || err.contains("is not a directory")
    {
        Ok(false)
    } else {
        Err(format!("restic cat config: {}", err.trim().chars().take(400).collect::<String>()))
    }
}

pub async fn init(restic_bin: &str, env: &ResticEnv) -> Result<(), String> {
    let mut cmd = Command::new(restic_bin);
    cmd.arg("init");
    apply_env(&mut cmd, env);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn restic init failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("restic init: {}", err.trim().chars().take(400).collect::<String>()))
    }
}

/// Tente de retirer un verrou périmé (au boot, sans run actif).
pub async fn unlock(restic_bin: &str, env: &ResticEnv) -> Result<(), String> {
    let mut cmd = Command::new(restic_bin);
    cmd.args(["unlock"]);
    apply_env(&mut cmd, env);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.status()
        .await
        .map_err(|e| format!("spawn restic unlock failed: {e}"))?;
    Ok(())
}

/// `restic forget --group-by host,tags --keep-last <keep> --prune`.
pub async fn forget_prune(restic_bin: &str, env: &ResticEnv, keep: i32) -> Result<(), String> {
    let keep = keep.max(1).to_string();
    let mut cmd = Command::new(restic_bin);
    cmd.args([
        "forget",
        "--group-by",
        "host,tags",
        "--keep-last",
        &keep,
        "--prune",
    ]);
    apply_env(&mut cmd, env);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn restic forget failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("restic forget: {}", err.trim().chars().take(400).collect::<String>()))
    }
}

/// Statistiques du dépôt (taille brute + nombre de snapshots).
pub async fn stats(restic_bin: &str, env: &ResticEnv) -> Result<RepoStats, String> {
    let mut cmd = Command::new(restic_bin);
    cmd.args(["stats", "--mode", "raw-data", "--json"]);
    apply_env(&mut cmd, env);
    cmd.stderr(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn restic stats failed: {e}"))?;
    let mut total_size_bytes = 0i64;
    if out.status.success() {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            total_size_bytes = v.get("total_size").and_then(|n| n.as_i64()).unwrap_or(0);
        }
    }
    let snapshot_count = snapshot_count(restic_bin, env).await.unwrap_or(0);
    Ok(RepoStats {
        total_size_bytes,
        snapshot_count,
    })
}

async fn snapshot_count(restic_bin: &str, env: &ResticEnv) -> Result<i64, String> {
    let mut cmd = Command::new(restic_bin);
    cmd.args(["snapshots", "--json"]);
    apply_env(&mut cmd, env);
    cmd.stderr(Stdio::null());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn restic snapshots failed: {e}"))?;
    if !out.status.success() {
        return Ok(0);
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse snapshots: {e}"))?;
    Ok(v.as_array().map(|a| a.len() as i64).unwrap_or(0))
}

/// `restic backup --json --tag <tag> <paths...>` (git / config).
pub async fn backup_paths(
    restic_bin: &str,
    env: &ResticEnv,
    tag: &str,
    paths: &[std::path::PathBuf],
    cancel: &mut oneshot::Receiver<()>,
    on_status: impl FnMut(u64, Option<u64>),
) -> RunOutcome {
    let mut cmd = Command::new(restic_bin);
    cmd.args(["backup", "--json", "--tag", tag]);
    for p in paths {
        cmd.arg(p);
    }
    apply_env(&mut cmd, env);
    drive(cmd, cancel, on_status).await
}

/// Pipeline `bash -c 'set -o pipefail; <producer> | restic backup --stdin ...'`
/// pour le dump Postgres (streamé, jamais d'artefact local). `pipefail` fait
/// échouer le run si `pg_dumpall` échoue en amont (sinon restic snapshoterait un
/// dump tronqué).
pub async fn backup_stdin_pipeline(
    env: &ResticEnv,
    bash_script: &str,
    cancel: &mut oneshot::Receiver<()>,
    on_status: impl FnMut(u64, Option<u64>),
) -> RunOutcome {
    let mut cmd = Command::new("bash");
    cmd.args(["-c", bash_script]);
    apply_env(&mut cmd, env);
    drive(cmd, cancel, on_status).await
}

/// Boucle commune : spawn, lecture du stdout JSON ligne-à-ligne (status →
/// callback, summary → résultat), course annulation/EOF, kill de groupe.
async fn drive(
    mut cmd: Command,
    cancel: &mut oneshot::Receiver<()>,
    mut on_status: impl FnMut(u64, Option<u64>),
) -> RunOutcome {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return RunOutcome::Failed(format!("spawn failed: {e}")),
    };
    let child_pid = child.id();

    // Draine stderr en tâche de fond (sinon le pipe peut bloquer l'enfant).
    let stderr_task = child.stderr.take().map(|err| {
        tokio::spawn(async move {
            let mut buf = String::new();
            let _ = BufReader::new(err).read_to_string(&mut buf).await;
            buf
        })
    });

    let mut summary = SnapshotResult::default();
    let mut cancelled = false;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        loop {
            tokio::select! {
                biased;
                _ = &mut *cancel => { cancelled = true; break; }
                next = lines.next_line() => match next {
                    Ok(Some(l)) => parse_line(&l, &mut summary, &mut on_status),
                    Ok(None) => break,
                    Err(e) => { warn!(?e, "restic stdout read error"); break; }
                },
            }
        }
    }

    if cancelled {
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

    if cancelled {
        return RunOutcome::Cancelled;
    }
    let ok = status.map(|s| s.success()).unwrap_or(false);
    if ok {
        debug!(bytes_added = summary.bytes_added, "restic backup done");
        RunOutcome::Ok(summary)
    } else {
        RunOutcome::Failed(stderr.trim().chars().take(800).collect::<String>())
    }
}

/// Parse une ligne JSON de `restic backup --json`.
fn parse_line(line: &str, summary: &mut SnapshotResult, on_status: &mut impl FnMut(u64, Option<u64>)) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    match v.get("message_type").and_then(|m| m.as_str()) {
        Some("status") => {
            let bytes_done = v.get("bytes_done").and_then(|n| n.as_u64()).unwrap_or(0);
            let total = v.get("total_bytes").and_then(|n| n.as_u64()).filter(|&n| n > 0);
            on_status(bytes_done, total);
        }
        Some("summary") => {
            summary.snapshot_id = v
                .get("snapshot_id")
                .and_then(|s| s.as_str())
                .map(|s| s.chars().take(12).collect());
            summary.bytes_added = v.get("data_added").and_then(|n| n.as_i64()).unwrap_or(0);
            summary.bytes_processed = v
                .get("total_bytes_processed")
                .and_then(|n| n.as_i64())
                .unwrap_or(0);
            summary.files = v
                .get("total_files_processed")
                .and_then(|n| n.as_i64())
                .unwrap_or(0);
        }
        _ => {}
    }
}
