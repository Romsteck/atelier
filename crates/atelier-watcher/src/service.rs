use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::{Semaphore, broadcast, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{MAX_OPEN_FINDINGS, RunKind, SurveillanceEvent, TranscriptLine};
use crate::codex::{CodexConfig, CodexRunner};
use crate::findings::FindingsStore;
use crate::git_watcher::GitWatcher;
use crate::gitutil;
use crate::memory::MemoryStore;
use crate::migration::{self, DEFAULT_DB_NAME};
use crate::runs::RunsStore;
use crate::sqlx::{Pool, Postgres};

/// Minimal per-app metadata the surveillance service needs (prompt stack hint +
/// git_watcher slug list).
#[derive(Debug, Clone)]
pub struct AppMeta {
    pub slug: String,
    pub stack: String,
}

#[derive(Clone)]
pub struct SurveillanceService {
    inner: Arc<Inner>,
}

struct Inner {
    findings: Option<FindingsStore>,
    runs: Option<RunsStore>,
    memory: Option<MemoryStore>,
    runner: CodexRunner,
    apps_src_root: PathBuf,
    stacks: HashMap<String, String>,
    sem: Arc<Semaphore>,
    /// Live event bus for WebSocket fan-out (findings/runs changes).
    tx: broadcast::Sender<SurveillanceEvent>,
    /// Live stream of Codex stdout lines for the in-progress-run console.
    transcript_tx: broadcast::Sender<TranscriptLine>,
    /// Rolling buffer of transcript lines per in-flight run, so a client that
    /// (re)opens the tab mid-run can replay the conversation so far instead of
    /// only seeing new lines. Dropped when the run ends (ephemeral).
    transcripts: Mutex<HashMap<Uuid, Vec<TranscriptLine>>>,
    /// In-flight runs, keyed by run id, with a oneshot to cancel each. Present
    /// only while a run executes (inserted by `execute`, removed when it ends).
    running: Mutex<HashMap<Uuid, oneshot::Sender<()>>>,
    enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SurveillanceConfig {
    pub admin_dsn: Option<String>,
    pub db_name: Option<String>,
    /// Apps known to the service — stack hints for prompts + git_watcher slugs.
    pub seed_apps: Vec<AppMeta>,
    /// Root of app sources: `<root>/<slug>/src/`.
    pub apps_src_root: PathBuf,
    /// Codex CLI invocation config.
    pub codex: CodexConfig,
    /// Max concurrent Codex subprocesses (ratelimit guard).
    pub max_concurrent: usize,
}

impl SurveillanceService {
    pub async fn start(cfg: SurveillanceConfig) -> Self {
        let pool = match bootstrap(&cfg).await {
            Ok(p) => Some(p),
            Err(err) => {
                warn!(?err, "atelier-watcher: bootstrap failed — running in noop mode");
                None
            }
        };
        let enabled = pool.is_some();
        let (findings, runs, memory) = match pool.as_ref() {
            Some(p) => (
                Some(FindingsStore::new(p.clone())),
                Some(RunsStore::new(p.clone())),
                Some(MemoryStore::new(p.clone())),
            ),
            None => (None, None, None),
        };

        let stacks: HashMap<String, String> = cfg
            .seed_apps
            .iter()
            .map(|a| (a.slug.clone(), a.stack.clone()))
            .collect();

        let (tx, _rx) = broadcast::channel::<SurveillanceEvent>(256);
        let (transcript_tx, _trx) = broadcast::channel::<TranscriptLine>(1024);

        let svc = Self {
            inner: Arc::new(Inner {
                findings,
                runs,
                memory,
                runner: CodexRunner::new(cfg.codex.clone()),
                apps_src_root: cfg.apps_src_root.clone(),
                stacks,
                sem: Arc::new(Semaphore::new(cfg.max_concurrent.max(1))),
                tx,
                transcript_tx,
                transcripts: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                enabled,
            }),
        };

        if enabled {
            // No internal scheduler — runs are manual only (a cron would burn
            // too much GPT+ subscription). git_watcher still auto-resolves
            // findings from `fix(surveillance:N)` commits.
            if let (Some(f), Some(m)) = (svc.inner.findings.clone(), svc.inner.memory.clone()) {
                let slugs: Vec<String> = cfg.seed_apps.iter().map(|a| a.slug.clone()).collect();
                let gw = GitWatcher::new(
                    cfg.apps_src_root.clone(),
                    slugs,
                    f,
                    m,
                    svc.inner.tx.clone(),
                );
                tokio::spawn(gw.run_loop());
            }
            info!("atelier-watcher: started (stores + git_watcher, manual runs only)");
        }

        svc
    }

    /// Subscribe to live surveillance events (used by the WebSocket route).
    pub fn subscribe(&self) -> broadcast::Receiver<SurveillanceEvent> {
        self.inner.tx.subscribe()
    }

    /// Subscribe to the live Codex stdout stream (in-progress-run console).
    pub fn subscribe_transcript(&self) -> broadcast::Receiver<TranscriptLine> {
        self.inner.transcript_tx.subscribe()
    }

    /// Snapshot of the buffered transcript for a run (for replay when a client
    /// opens the tab mid-run). Empty once the run has ended.
    pub fn transcript(&self, run_id: Uuid) -> Vec<TranscriptLine> {
        self.inner
            .transcripts
            .lock()
            .unwrap()
            .get(&run_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Request cancellation of an in-flight run. Returns true if the run was
    /// found running (its Codex subprocess is then killed and the run recorded
    /// as `cancelled`); false if it already finished or never existed. Removing
    /// the sender + sending fires the oneshot the run's `exec` is awaiting.
    pub fn cancel_run(&self, run_id: Uuid) -> bool {
        let sender = self.inner.running.lock().unwrap().remove(&run_id);
        match sender {
            Some(tx) => {
                let _ = tx.send(());
                true
            }
            None => false,
        }
    }

    /// Broadcast a live event. No-op if there are no subscribers.
    pub fn emit(&self, kind: &str, slug: &str, action: &str) {
        let _ = self.inner.tx.send(SurveillanceEvent {
            kind: kind.to_string(),
            slug: slug.to_string(),
            action: action.to_string(),
        });
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    pub fn findings(&self) -> Option<&FindingsStore> {
        self.inner.findings.as_ref()
    }
    pub fn runs(&self) -> Option<&RunsStore> {
        self.inner.runs.as_ref()
    }
    pub fn memory(&self) -> Option<&MemoryStore> {
        self.inner.memory.as_ref()
    }

    /// Start a surveillance run. Creates the `surveillance_runs` row, spawns a
    /// detached task that runs the gates + Codex, and returns the run id
    /// immediately (the work is async — findings appear via findings_list once
    /// Codex finishes). `trigger` is always "manual" today (no scheduler).
    pub async fn run_now(
        &self,
        slug: String,
        kind: RunKind,
        trigger: &str,
    ) -> Result<Uuid, String> {
        let Some(runs) = self.inner.runs.as_ref() else {
            return Err("surveillance disabled (postgres unreachable)".into());
        };
        let run_id = runs
            .start(&slug, kind.run_kind(), trigger, None)
            .await
            .map_err(|e| format!("failed to create run: {e}"))?;
        let svc = self.clone();
        tokio::spawn(async move {
            svc.execute(run_id, slug, kind).await;
        });
        Ok(run_id)
    }

    async fn execute(&self, run_id: Uuid, slug: String, kind: RunKind) {
        // Register a cancel channel for the whole run lifetime so a stop request
        // can kill the Codex subprocess. The Sender is held in the registry
        // until the run ends, so the Receiver only fires on an explicit cancel.
        let (cancel_tx, cancel_rx) = oneshot::channel();
        self.inner
            .running
            .lock()
            .unwrap()
            .insert(run_id, cancel_tx);

        self.emit("run", &slug, "started");
        self.execute_inner(run_id, &slug, kind, cancel_rx).await;
        self.inner.running.lock().unwrap().remove(&run_id);
        // Drop the buffered transcript — the run has settled (panel disappears).
        self.inner.transcripts.lock().unwrap().remove(&run_id);
        // A run almost always touches findings; emit a final event so any open
        // Surveillance view refreshes once the run settles.
        self.emit("run", &slug, "finished");
    }

    async fn execute_inner(
        &self,
        run_id: Uuid,
        slug: &str,
        kind: RunKind,
        cancel_rx: oneshot::Receiver<()>,
    ) {
        let slug = slug.to_string();
        let (Some(findings), Some(runs), Some(memory)) = (
            self.inner.findings.as_ref(),
            self.inner.runs.as_ref(),
            self.inner.memory.as_ref(),
        ) else {
            return;
        };

        // Gate 1 — cap: skip when this kind already has MAX_OPEN_FINDINGS open
        // findings (the UI disables the launch button at the same threshold;
        // this is the server-side backstop). `open_now` is reused below to
        // budget the prompt so Codex reports only the most important issues.
        let open_now = match findings.count_open(&slug, kind.finding_kind()).await {
            Ok(n) => n,
            Err(e) => {
                warn!(slug = %slug, ?e, "open findings count failed — proceeding");
                0
            }
        };
        if open_now >= MAX_OPEN_FINDINGS {
            let reason = format!("cap: {open_now} findings open (max {MAX_OPEN_FINDINGS})");
            let _ = runs.finish_skipped(run_id, &reason).await;
            info!(slug = %slug, kind = kind.run_kind(), "run skipped (cap)");
            return;
        }

        // Gate 2 — diff-aware (code_review + suggestions both track their SHA).
        let src = self.inner.apps_src_root.join(&slug).join("src");
        let head = gitutil::head_sha(&src).await;
        let last_sha = memory
            .get(&slug, Some("last_run"), Some(kind.sha_memory_key()))
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
            .and_then(|m| m.value.as_str().map(String::from));

        let diff: Option<String> = match (&last_sha, &head) {
            (Some(last), Some(h)) if last == h => {
                let _ = runs.finish_skipped(run_id, "no_diff (HEAD unchanged)").await;
                info!(slug = %slug, kind = kind.run_kind(), "run skipped (no_diff)");
                return;
            }
            (Some(last), Some(_)) => {
                let d = gitutil::diff_since(&src, last).await;
                if d.is_none() {
                    let _ = runs.finish_skipped(run_id, "no_diff (empty range)").await;
                    info!(slug = %slug, kind = kind.run_kind(), "run skipped (no_diff empty)");
                    return;
                }
                d
            }
            // First run (no recorded SHA) → full review, no diff.
            _ => None,
        };

        // Build prompt.
        let stack = self
            .inner
            .stacks
            .get(&slug)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let mem_entries = memory.get(&slug, None, None).await.unwrap_or_default();
        let prompt = self
            .inner
            .runner
            .build_prompt(&slug, &stack, kind, diff.as_deref(), &mem_entries, open_now);

        // Acquire concurrency permit + run Codex.
        let _permit = self.inner.sem.acquire().await.ok();
        let measure_from = Utc::now();
        // Stream each stdout line to the live console (ephemeral; not persisted)
        // and append to the per-run buffer for mid-run tab re-opens.
        let inner = self.inner.clone();
        let run_kind = kind.run_kind().to_string();
        let slug_line = slug.clone();
        let mut seq: u64 = 0;
        let exec = self
            .inner
            .runner
            .exec(&src, &prompt, cancel_rx, |line| {
                seq += 1;
                let tl = TranscriptLine {
                    run_id,
                    slug: slug_line.clone(),
                    kind: run_kind.clone(),
                    seq,
                    line: line.to_string(),
                };
                {
                    let mut map = inner.transcripts.lock().unwrap();
                    let buf = map.entry(run_id).or_default();
                    buf.push(tl.clone());
                    // Cap memory: keep the last ~2000 lines per run.
                    if buf.len() > 2500 {
                        let cut = buf.len() - 2000;
                        buf.drain(0..cut);
                    }
                }
                let _ = inner.transcript_tx.send(tl);
            })
            .await;

        if exec.cancelled {
            let _ = runs.finish_cancelled(run_id).await;
            info!(slug = %slug, kind = kind.run_kind(), "codex run cancelled by user");
            return;
        }
        if let Some(err) = exec.spawn_error {
            let _ = runs.finish_failed(run_id, &err).await;
            self.note_failure(&slug, kind, memory).await;
            warn!(slug = %slug, kind = kind.run_kind(), %err, "codex spawn failed");
            return;
        }
        if !exec.exit_ok {
            let msg = if exec.stderr.is_empty() {
                "codex exited non-zero".to_string()
            } else {
                exec.stderr.clone()
            };
            let _ = runs.finish_failed(run_id, &msg).await;
            self.note_failure(&slug, kind, memory).await;
            warn!(slug = %slug, kind = kind.run_kind(), "codex run failed");
            return;
        }

        // Success — measure how many findings Codex touched during the run.
        let delta = findings
            .count_touched_since(&slug, kind.finding_kind(), measure_from)
            .await
            .unwrap_or(0);
        let empty = delta == 0;
        let _ = runs
            .finish_success(
                run_id,
                delta as i32,
                exec.tokens_in,
                exec.tokens_out,
                head.as_deref(),
                empty,
            )
            .await;

        // Record reviewed SHA for diff-aware next time.
        if let Some(h) = &head {
            let _ = memory
                .upsert(
                    &slug,
                    "last_run",
                    kind.sha_memory_key(),
                    &serde_json::Value::String(h.clone()),
                    None,
                )
                .await;
        }
        // Reset consecutive-failure counter on success.
        let _ = memory
            .delete(&slug, "last_run", &format!("{}:consecutive_failures", kind.run_kind()))
            .await;

        info!(
            slug = %slug,
            kind = kind.run_kind(),
            findings = delta,
            empty,
            "codex run success"
        );
    }

    /// Track consecutive failures. After 3 in a row we just log loudly — a
    /// proper meta-finding / Hub ping needs schema + Hub wiring (deferred).
    async fn note_failure(&self, slug: &str, kind: RunKind, memory: &MemoryStore) {
        let key = format!("{}:consecutive_failures", kind.run_kind());
        let prev = memory
            .get(slug, Some("last_run"), Some(&key))
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
            .and_then(|m| m.value.as_i64())
            .unwrap_or(0);
        let next = prev + 1;
        let _ = memory
            .upsert(slug, "last_run", &key, &serde_json::json!(next), None)
            .await;
        if next >= 3 {
            warn!(
                slug = %slug,
                kind = kind.run_kind(),
                count = next,
                "surveillance: 3+ consecutive failures — check codex auth/install"
            );
        }
    }
}

async fn bootstrap(cfg: &SurveillanceConfig) -> anyhow::Result<Pool<Postgres>> {
    let admin_dsn = cfg
        .admin_dsn
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("admin_dsn missing"))?;
    let db_name = cfg.db_name.as_deref().unwrap_or(DEFAULT_DB_NAME);

    let admin_pool = migration::open_admin_pool(admin_dsn).await?;
    migration::ensure_database(&admin_pool, db_name).await?;

    let target_dsn = migration::swap_db(admin_dsn, db_name);
    let pool = migration::open_pool(&target_dsn).await?;
    migration::run_migrations(&pool).await?;

    Ok(pool)
}
