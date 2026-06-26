use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{Semaphore, broadcast, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{info, warn};
use uuid::Uuid;

use crate::claude::{ClaudeRunner, ClaudeScanConfig};
use crate::findings::FindingsStore;
use crate::git_watcher::GitWatcher;
use crate::gitutil;
use crate::memory::MemoryStore;
use crate::migration::{self, DEFAULT_DB_NAME};
use crate::runs::RunsStore;
use crate::scandef::{
    AppScanStore, BIZ_KIND, CODE_REVIEW_KIND, Gate, SECURITY_KIND, ScanDef, is_valid_kind, sha_key,
    watermark_key,
};
use crate::sqlx::{Pool, Postgres};
use crate::sweep_scheduler::SweepScheduleStore;
use crate::{MAX_OPEN_FINDINGS, SurveillanceEvent, TranscriptLine};

/// Order in which the sweep launches an app's scans (all three run together).
const SWEEP_KINDS: [&str; 3] = [SECURITY_KIND, CODE_REVIEW_KIND, BIZ_KIND];

/// Minimal per-app metadata the surveillance service needs (prompt stack hint +
/// git_watcher slug list + display name for the sweep UI).
#[derive(Debug, Clone)]
pub struct AppMeta {
    pub slug: String,
    pub name: String,
    pub stack: String,
}

/// Terminal outcome of one scan run, mapped to a sweep cell. Returned by
/// `execute`/`execute_inner` so the sweep can show per-scan progress.
#[derive(Debug, Clone, Copy)]
pub enum RunOutcome {
    Success,
    Empty,
    Skipped,
    Failed,
    Cancelled,
}

impl RunOutcome {
    fn to_cell(self) -> ScanCell {
        match self {
            RunOutcome::Success => ScanCell::Done,
            RunOutcome::Empty => ScanCell::Empty,
            RunOutcome::Skipped => ScanCell::Skipped,
            RunOutcome::Failed => ScanCell::Failed,
            RunOutcome::Cancelled => ScanCell::Cancelled,
        }
    }
}

/// Overall state of the automatic sweep (single-flight). `Idle` = never started
/// (or reset); terminal states (`Done`/`Cancelled`/`Failed`) are retained so a
/// page load mid/after a sweep can hydrate the live view.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SweepStatus {
    Idle,
    Running,
    Cancelling,
    Done,
    Cancelled,
    Failed,
}

/// Per-scan cell state within a sweep app row.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanCell {
    Pending,
    Running,
    Done,
    Empty,
    Skipped,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct SweepScanState {
    pub status: ScanCell,
    /// The run id, set once the scan is launched — the frontend subscribes to
    /// this run's `surveillance:transcript` stream to show the live console.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
}

impl SweepScanState {
    fn pending() -> Self {
        Self { status: ScanCell::Pending, run_id: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SweepAppRow {
    pub slug: String,
    pub name: String,
    pub security: SweepScanState,
    pub code_review: SweepScanState,
    pub business: SweepScanState,
}

impl SweepAppRow {
    fn cell_mut(&mut self, kind: &str) -> &mut SweepScanState {
        match kind {
            SECURITY_KIND => &mut self.security,
            CODE_REVIEW_KIND => &mut self.code_review,
            _ => &mut self.business,
        }
    }
}

/// Full sweep state, broadcast over `surveillance:sweep` on every transition and
/// returned by `GET /api/surveillance/sweep` for page-load hydration.
#[derive(Debug, Clone, Serialize)]
pub struct SweepSnapshot {
    pub status: SweepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// Index into `apps` of the app currently being scanned.
    pub current_index: usize,
    pub total: usize,
    /// Apps fully settled so far.
    pub done: usize,
    pub apps: Vec<SweepAppRow>,
}

impl SweepSnapshot {
    fn idle() -> Self {
        Self {
            status: SweepStatus::Idle,
            started_at: None,
            finished_at: None,
            current_index: 0,
            total: 0,
            done: 0,
            apps: Vec::new(),
        }
    }
}

/// In-memory sweep state behind a mutex (separate from `running` so locks don't
/// contend). `abort` is the master cancel flag the loop polls; `active_runs` are
/// the in-flight run ids of the current app, killed on cancel.
struct SweepInner {
    snapshot: SweepSnapshot,
    abort: bool,
    active_runs: Vec<Uuid>,
}

#[derive(Clone)]
pub struct SurveillanceService {
    inner: Arc<Inner>,
}

struct Inner {
    findings: Option<FindingsStore>,
    runs: Option<RunsStore>,
    memory: Option<MemoryStore>,
    /// Per-app scan definitions (`app_scan` table). `None` in noop mode.
    app_scan: Option<AppScanStore>,
    /// Sweep schedule config store (`sweep_schedule` singleton). `None` in noop.
    sweep_schedule: Option<SweepScheduleStore>,
    runner: ClaudeRunner,
    apps_src_root: PathBuf,
    stacks: HashMap<String, String>,
    /// Ordered app list (slug + name) for the automatic sweep.
    apps: Vec<AppMeta>,
    /// Single-flight automatic sweep state (`None` = never started).
    sweep: Mutex<Option<SweepInner>>,
    /// Broadcast of the full sweep snapshot on every transition (WebSocket fan-out).
    sweep_tx: broadcast::Sender<SweepSnapshot>,
    sem: Arc<Semaphore>,
    /// Live event bus for WebSocket fan-out (findings/runs changes).
    tx: broadcast::Sender<SurveillanceEvent>,
    /// Live stream of scan-agent stdout lines for the in-progress-run console.
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
    /// AI engine for scans — the Claude Agent SDK (OAuth subscription, run as
    /// `hr-studio` via `scan.js`; never an API key).
    pub driver: ClaudeScanConfig,
    /// Max concurrent scan subprocesses (ratelimit guard).
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
        let (findings, runs, memory, app_scan, sweep_schedule) = match pool.as_ref() {
            Some(p) => (
                Some(FindingsStore::new(p.clone())),
                Some(RunsStore::new(p.clone())),
                Some(MemoryStore::new(p.clone())),
                Some(AppScanStore::new(p.clone())),
                Some(SweepScheduleStore::new(p.clone())),
            ),
            None => (None, None, None, None, None),
        };

        let stacks: HashMap<String, String> = cfg
            .seed_apps
            .iter()
            .map(|a| (a.slug.clone(), a.stack.clone()))
            .collect();

        let (tx, _rx) = broadcast::channel::<SurveillanceEvent>(256);
        let (transcript_tx, _trx) = broadcast::channel::<TranscriptLine>(1024);
        let (sweep_tx, _srx) = broadcast::channel::<SweepSnapshot>(64);

        let svc = Self {
            inner: Arc::new(Inner {
                findings,
                runs,
                memory,
                app_scan,
                sweep_schedule,
                runner: ClaudeRunner::new(cfg.driver.clone()),
                apps_src_root: cfg.apps_src_root.clone(),
                stacks,
                apps: cfg.seed_apps.clone(),
                sweep: Mutex::new(None),
                sweep_tx,
                sem: Arc::new(Semaphore::new(cfg.max_concurrent.max(1))),
                tx,
                transcript_tx,
                transcripts: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                enabled,
            }),
        };

        if enabled {
            // Boot reconciliation: rows left 'running' by a previous process
            // (restart mid-scan kills the detached tokio task) would otherwise
            // pin the dashboard's "running" counter forever.
            if let Some(r) = svc.inner.runs.as_ref() {
                match r.reconcile_interrupted().await {
                    Ok(0) => {}
                    Ok(n) => warn!(count = n, "surveillance: marked interrupted runs as failed"),
                    Err(e) => warn!(?e, "surveillance: stale-run reconciliation failed"),
                }
            }
            // Automatic sweep scheduler (boucle Tokio, off par défaut — activable
            // via PUT /api/surveillance/sweep/schedule). git_watcher auto-résout
            // les findings depuis les commits `fix(surveillance:N)`.
            if let Some(store) = svc.inner.sweep_schedule.clone() {
                tokio::spawn(crate::sweep_scheduler::run_loop(svc.clone(), store));
            }
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
            // Ensure every known app has a (blank) scan row — provisioning safety
            // net on top of AppCreate + the migration backfill.
            if let Some(store) = svc.inner.app_scan.clone() {
                let slugs: Vec<String> = cfg.seed_apps.iter().map(|a| a.slug.clone()).collect();
                tokio::spawn(async move {
                    for slug in slugs {
                        if let Err(e) = store.ensure(&slug).await {
                            warn!(slug = %slug, ?e, "ensure app_scan blank row failed");
                        }
                    }
                });
            }
            info!("atelier-watcher: started (stores + git_watcher + sweep scheduler)");
        }

        svc
    }

    /// Subscribe to live surveillance events (used by the WebSocket route).
    pub fn subscribe(&self) -> broadcast::Receiver<SurveillanceEvent> {
        self.inner.tx.subscribe()
    }

    /// Subscribe to the live scan-agent stdout stream (in-progress-run console).
    pub fn subscribe_transcript(&self) -> broadcast::Receiver<TranscriptLine> {
        self.inner.transcript_tx.subscribe()
    }

    /// Subscribe to live sweep-state snapshots (the `surveillance:sweep` channel).
    pub fn subscribe_sweep(&self) -> broadcast::Receiver<SweepSnapshot> {
        self.inner.sweep_tx.subscribe()
    }

    /// Current sweep state for page-load hydration. `Idle` when no sweep has run
    /// (or after a restart — the in-memory snapshot is not persisted).
    pub fn sweep_snapshot(&self) -> SweepSnapshot {
        self.inner
            .sweep
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.snapshot.clone())
            .unwrap_or_else(SweepSnapshot::idle)
    }

    fn broadcast_sweep(&self) {
        let snap = self
            .inner
            .sweep
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.snapshot.clone());
        if let Some(snap) = snap {
            let _ = self.inner.sweep_tx.send(snap);
        }
    }

    /// Mutate the live sweep state under lock. No-op (returns false) if no sweep.
    fn with_sweep<F: FnOnce(&mut SweepInner)>(&self, f: F) -> bool {
        let mut guard = self.inner.sweep.lock().unwrap();
        match guard.as_mut() {
            Some(s) => {
                f(s);
                true
            }
            None => false,
        }
    }

    fn sweep_aborted(&self) -> bool {
        self.inner
            .sweep
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.abort)
            .unwrap_or(false)
    }

    /// Start the automatic sweep (manual or `cron`). Single-flight: returns
    /// `Err("sweep already running")` if one is active. Builds the app queue,
    /// flips state to `Running`, spawns the loop, and returns the initial
    /// snapshot. The loop reuses the exact Claude scan path (run row + `execute`
    /// + `scan.js` as hr-studio), **forcing** every scan past the freshness/cap
    /// gates so the triage/purge runs on every app.
    pub fn start_sweep(&self, trigger: &str) -> Result<SweepSnapshot, String> {
        if !self.inner.enabled {
            return Err("surveillance disabled (postgres unreachable)".into());
        }
        let mut guard = self.inner.sweep.lock().unwrap();
        if let Some(s) = guard.as_ref() {
            if matches!(s.snapshot.status, SweepStatus::Running | SweepStatus::Cancelling) {
                return Err("sweep already running".into());
            }
        }
        let apps: Vec<SweepAppRow> = self
            .inner
            .apps
            .iter()
            .map(|a| SweepAppRow {
                slug: a.slug.clone(),
                name: a.name.clone(),
                security: SweepScanState::pending(),
                code_review: SweepScanState::pending(),
                business: SweepScanState::pending(),
            })
            .collect();
        if apps.is_empty() {
            return Err("no apps to sweep".into());
        }
        let snapshot = SweepSnapshot {
            status: SweepStatus::Running,
            started_at: Some(Utc::now()),
            finished_at: None,
            current_index: 0,
            total: apps.len(),
            done: 0,
            apps,
        };
        *guard = Some(SweepInner {
            snapshot: snapshot.clone(),
            abort: false,
            active_runs: Vec::new(),
        });
        drop(guard);
        self.broadcast_sweep();
        let svc = self.clone();
        let trigger = trigger.to_string();
        tokio::spawn(async move { svc.run_sweep(trigger).await });
        info!("surveillance sweep started");
        Ok(snapshot)
    }

    /// Cancel the active sweep: set the abort flag, kill the in-flight runs of
    /// the current app, and flip to `Cancelling` (the loop settles it to
    /// `Cancelled`). Returns false if no sweep is active.
    pub fn cancel_sweep(&self) -> bool {
        let active: Vec<Uuid> = {
            let mut guard = self.inner.sweep.lock().unwrap();
            let Some(s) = guard.as_mut() else {
                return false;
            };
            if !matches!(s.snapshot.status, SweepStatus::Running | SweepStatus::Cancelling) {
                return false;
            }
            s.abort = true;
            s.snapshot.status = SweepStatus::Cancelling;
            s.active_runs.clone()
        };
        for id in &active {
            self.cancel_run(*id);
        }
        self.broadcast_sweep();
        true
    }

    /// The sweep loop: app by app, launch the 3 scans simultaneously (forced),
    /// await them, advance. Broadcasts a fresh snapshot on every transition.
    async fn run_sweep(&self, trigger: String) {
        let apps = self.inner.apps.clone();
        let mut aborted = false;
        for (i, app) in apps.iter().enumerate() {
            if self.sweep_aborted() {
                aborted = true;
                break;
            }
            self.with_sweep(|s| s.snapshot.current_index = i);

            // Launch the app's 3 scans simultaneously (forced past the gates).
            let mut set: JoinSet<(Uuid, RunOutcome)> = JoinSet::new();
            for kind in SWEEP_KINDS {
                match self.launch_run(app.slug.clone(), kind, &trigger, true, None).await {
                    Ok((run_id, handle)) => {
                        self.with_sweep(|s| {
                            if let Some(row) = s.snapshot.apps.get_mut(i) {
                                let cell = row.cell_mut(kind);
                                cell.status = ScanCell::Running;
                                cell.run_id = Some(run_id);
                            }
                            s.active_runs.push(run_id);
                        });
                        set.spawn(async move {
                            (run_id, handle.await.unwrap_or(RunOutcome::Failed))
                        });
                    }
                    Err(e) => {
                        warn!(slug = %app.slug, kind, error = %e, "sweep: scan launch failed");
                        self.with_sweep(|s| {
                            if let Some(row) = s.snapshot.apps.get_mut(i) {
                                row.cell_mut(kind).status = ScanCell::Failed;
                            }
                        });
                    }
                }
            }
            self.broadcast_sweep();

            // Await this app's runs; settle each cell as it completes.
            while let Some(joined) = set.join_next().await {
                if let Ok((run_id, outcome)) = joined {
                    self.with_sweep(|s| {
                        if let Some(row) = s.snapshot.apps.get_mut(i) {
                            for cell in [
                                &mut row.security,
                                &mut row.code_review,
                                &mut row.business,
                            ] {
                                if cell.run_id == Some(run_id) {
                                    cell.status = outcome.to_cell();
                                }
                            }
                        }
                    });
                    self.broadcast_sweep();
                }
            }

            self.with_sweep(|s| {
                s.active_runs.clear();
                s.snapshot.done = i + 1;
            });
            self.broadcast_sweep();

            if self.sweep_aborted() {
                aborted = true;
                break;
            }
        }

        self.with_sweep(|s| {
            s.snapshot.status = if aborted {
                SweepStatus::Cancelled
            } else {
                SweepStatus::Done
            };
            s.snapshot.finished_at = Some(Utc::now());
        });
        self.broadcast_sweep();
        if let Some(store) = self.inner.sweep_schedule.as_ref() {
            if let Err(e) = store.mark_ran().await {
                warn!(?e, "sweep: failed to record last_run_at");
            }
        }
        info!(aborted, "surveillance sweep finished");
    }

    /// Read the sweep schedule config (singleton). `None` in noop mode.
    pub async fn sweep_schedule_get(
        &self,
    ) -> Option<anyhow::Result<crate::sweep_scheduler::SweepSchedule>> {
        let store = self.inner.sweep_schedule.as_ref()?;
        Some(store.get().await)
    }

    /// Update the sweep schedule config (enabled / hour / cadence).
    pub async fn sweep_schedule_set(
        &self,
        enabled: bool,
        hour: i32,
        cadence: &str,
    ) -> Result<crate::sweep_scheduler::SweepSchedule, String> {
        let Some(store) = self.inner.sweep_schedule.as_ref() else {
            return Err("surveillance disabled (postgres unreachable)".into());
        };
        store
            .set(enabled, hour, cadence)
            .await
            .map_err(|e| e.to_string())
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
    /// found running (its scan subprocess is then killed and the run recorded
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

    /// Read an app's BUSINESS scan definition (the agent-owned `app_scan` row).
    /// `None` if the app has no row yet (or in noop mode). The `security` and
    /// `code_review` scans are not stored here — see `ScanDef::fixed`.
    pub async fn scan_get(&self, slug: &str) -> Option<ScanDef> {
        self.inner.app_scan.as_ref()?.get(slug).await.ok().flatten()
    }

    /// Create the blank scan row for an app if absent (idempotent provisioning
    /// — called at AppCreate so every app starts with an empty scan).
    pub async fn ensure_app_scan(&self, slug: &str) -> Result<(), String> {
        let Some(store) = self.inner.app_scan.as_ref() else {
            return Err("surveillance disabled (postgres unreachable)".into());
        };
        store.ensure(slug).await.map_err(|e| e.to_string())
    }

    /// Create/replace an app's scan definition (agent-owned, NO approval). The
    /// project's agent calls this via the `scan_set` MCP tool. Validates the
    /// gate + (when data-gated) the SELECT-only `gate_sql`.
    #[allow(clippy::too_many_arguments)]
    pub async fn scan_set(
        &self,
        slug: &str,
        label: &str,
        prompt: &str,
        cadence: &str,
        gate: &str,
        gate_sql: Option<&str>,
        categories: &[String],
        updated_by: &str,
    ) -> Result<(), String> {
        let Some(store) = self.inner.app_scan.as_ref() else {
            return Err("surveillance disabled (postgres unreachable)".into());
        };
        let gate = Gate::parse(gate);
        if gate == Gate::Data {
            let sql = gate_sql.unwrap_or("").trim();
            if sql.is_empty() {
                return Err("gate='data' requires a gate_sql (a read-only SELECT watermark)".into());
            }
            let head = sql.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
            if head != "SELECT" && head != "WITH" {
                return Err("gate_sql must be a read-only SELECT (start with SELECT or WITH)".into());
            }
        }
        store
            .upsert(slug, label, prompt, cadence, gate, gate_sql, categories, updated_by)
            .await
            .map_err(|e| e.to_string())
    }

    /// Start a scan run for one of the app's three kinds (`security` /
    /// `code_review` / `business`). Creates the `surveillance_runs` row, spawns a
    /// detached task that runs the gates + the scan-agent, and returns the run id
    /// immediately (the work is async). `trigger` is "manual" or "cron".
    ///
    /// `data_watermark` is the freshness signal for a data-gated scan (only the
    /// `business` scan can be data-gated): the caller (which has dataverse access)
    /// runs the scan's `gate_sql` and passes the resulting watermark, so
    /// `atelier-watcher` stays decoupled from the dataverse. `None` otherwise.
    pub async fn run_now(
        &self,
        slug: String,
        kind: &str,
        trigger: &str,
        data_watermark: Option<String>,
    ) -> Result<Uuid, String> {
        // Manual runs respect the gates (force=false) — same behavior as before.
        let (run_id, _handle) = self.launch_run(slug, kind, trigger, false, data_watermark).await?;
        Ok(run_id)
    }

    /// Create the run row and spawn `execute`, returning the run id immediately
    /// AND a JoinHandle that resolves to the terminal `RunOutcome`. `run_now`
    /// drops the handle (fire-and-forget); the sweep awaits it to barrier on an
    /// app's three scans. `force=true` bypasses the freshness + cap gates.
    async fn launch_run(
        &self,
        slug: String,
        kind: &str,
        trigger: &str,
        force: bool,
        data_watermark: Option<String>,
    ) -> Result<(Uuid, JoinHandle<RunOutcome>), String> {
        if !is_valid_kind(kind) {
            return Err(format!("invalid scan kind: {kind}"));
        }
        let Some(runs) = self.inner.runs.as_ref() else {
            return Err("surveillance disabled (postgres unreachable)".into());
        };
        let run_id = runs
            .start(&slug, kind, trigger, None)
            .await
            .map_err(|e| format!("failed to create run: {e}"))?;
        let svc = self.clone();
        let kind = kind.to_string();
        let handle = tokio::spawn(async move {
            svc.execute(run_id, slug, kind, force, data_watermark).await
        });
        Ok((run_id, handle))
    }

    async fn execute(
        &self,
        run_id: Uuid,
        slug: String,
        kind: String,
        force: bool,
        data_watermark: Option<String>,
    ) -> RunOutcome {
        // Register a cancel channel for the whole run lifetime so a stop request
        // can kill the scan subprocess. The Sender is held in the registry
        // until the run ends, so the Receiver only fires on an explicit cancel.
        let (cancel_tx, cancel_rx) = oneshot::channel();
        self.inner
            .running
            .lock()
            .unwrap()
            .insert(run_id, cancel_tx);

        self.emit("run", &slug, "started");
        let outcome = self
            .execute_inner(run_id, &slug, &kind, force, data_watermark, cancel_rx)
            .await;
        self.inner.running.lock().unwrap().remove(&run_id);
        // Drop the buffered transcript — the run has settled (panel disappears).
        self.inner.transcripts.lock().unwrap().remove(&run_id);
        // A run almost always touches findings; emit a final event so any open
        // Surveillance view refreshes once the run settles.
        self.emit("run", &slug, "finished");
        outcome
    }

    async fn execute_inner(
        &self,
        run_id: Uuid,
        slug: &str,
        kind: &str,
        force: bool,
        data_watermark: Option<String>,
        cancel_rx: oneshot::Receiver<()>,
    ) -> RunOutcome {
        let slug = slug.to_string();
        let (Some(findings), Some(runs), Some(memory), Some(app_scan)) = (
            self.inner.findings.as_ref(),
            self.inner.runs.as_ref(),
            self.inner.memory.as_ref(),
            self.inner.app_scan.as_ref(),
        ) else {
            return RunOutcome::Failed;
        };

        // Resolve the scan definition by kind. `security`/`code_review` are fixed
        // platform scans (constructors, never blank, run for every app). `business`
        // is the agent-owned `app_scan` row; a blank one (no prompt) is "en veille".
        let scan = match ScanDef::fixed(kind, &slug) {
            Some(s) => s,
            None => match app_scan.get(&slug).await {
                Ok(Some(s)) if !s.is_blank() => s,
                Ok(_) => {
                    let _ = runs.finish_skipped(run_id, "blank (scan non défini)").await;
                    info!(slug = %slug, kind, "run skipped (blank scan)");
                    return RunOutcome::Skipped;
                }
                Err(e) => {
                    let _ = runs.finish_failed(run_id, &format!("scan load failed: {e}")).await;
                    warn!(slug = %slug, kind, ?e, "scan load failed");
                    return RunOutcome::Failed;
                }
            },
        };

        // Gate 1 — cap: skip when this (app,kind) already has MAX_OPEN_FINDINGS
        // open findings (the UI disables that kind's launch button at the same
        // threshold; this is the server-side backstop). `open_now` is reused below
        // to budget the prompt so the scan-agent reports only the most important
        // issues. A forced sweep run bypasses the cap (so the triage/purge can run
        // even at the ceiling) — the {{REMAINING}}=0 budget still blocks NEW findings.
        let open_now = match findings.count_open(&slug, kind).await {
            Ok(n) => n,
            Err(e) => {
                warn!(slug = %slug, ?e, "open findings count failed — proceeding");
                0
            }
        };
        if !force && open_now >= MAX_OPEN_FINDINGS {
            let reason = format!("cap: {open_now} findings open (max {MAX_OPEN_FINDINGS})");
            let _ = runs.finish_skipped(run_id, &reason).await;
            info!(slug = %slug, "run skipped (cap)");
            return RunOutcome::Skipped;
        }

        // Gate 2 — freshness, per the scan's gate. `code` → git-diff (skip when
        // HEAD unchanged); `data` → watermark from the scan's gate_sql (skip when
        // unchanged); `manual` → always run. A forced sweep run never skips: it
        // runs a full review (or the diff since the last reviewed SHA, if any) so
        // stale findings get re-examined even on an unchanged app.
        let src = self.inner.apps_src_root.join(&slug).join("src");
        let head = gitutil::head_sha(&src).await;
        let diff: Option<String> = match scan.gate {
            Gate::Code => {
                let last_sha = memory
                    .get(&slug, Some("last_run"), Some(&sha_key(kind)))
                    .await
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .and_then(|m| m.value.as_str().map(String::from));
                if force {
                    // Diff since the last reviewed SHA if it differs; else a full
                    // review (None) — never skip.
                    match (&last_sha, &head) {
                        (Some(last), Some(h)) if last != h => gitutil::diff_since(&src, last).await,
                        _ => None,
                    }
                } else {
                    match (&last_sha, &head) {
                        (Some(last), Some(h)) if last == h => {
                            let _ = runs.finish_skipped(run_id, "no_diff (HEAD unchanged)").await;
                            info!(slug = %slug, "run skipped (no_diff)");
                            return RunOutcome::Skipped;
                        }
                        (Some(last), Some(_)) => {
                            let d = gitutil::diff_since(&src, last).await;
                            if d.is_none() {
                                let _ = runs.finish_skipped(run_id, "no_diff (empty range)").await;
                                info!(slug = %slug, "run skipped (no_diff empty)");
                                return RunOutcome::Skipped;
                            }
                            d
                        }
                        // First run (no recorded SHA) → full review, no diff.
                        _ => None,
                    }
                }
            }
            Gate::Data => {
                // The watermark is the latest "material" the scan would analyse.
                // Empty ⇒ nothing to analyse; unchanged vs last run ⇒ no new material.
                if force {
                    None
                } else {
                    match &data_watermark {
                        None => None, // caller couldn't compute it → run unconditionally
                        Some(w) if w.is_empty() => {
                            let _ = runs.finish_skipped(run_id, "no_new_data").await;
                            info!(slug = %slug, "run skipped (no_new_data)");
                            return RunOutcome::Skipped;
                        }
                        Some(w) => {
                            let last = memory
                                .get(&slug, Some("last_run"), Some(&watermark_key(kind)))
                                .await
                                .ok()
                                .and_then(|v| v.into_iter().next())
                                .and_then(|m| m.value.as_str().map(String::from));
                            if last.as_deref() == Some(w.as_str()) {
                                let _ = runs.finish_skipped(run_id, "no_new_data").await;
                                info!(slug = %slug, "run skipped (no_new_data)");
                                return RunOutcome::Skipped;
                            }
                            None
                        }
                    }
                }
            }
            Gate::Manual => None,
        };

        // Build prompt.
        let stack = self
            .inner
            .stacks
            .get(&slug)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let mem_entries = memory.get(&slug, None, None).await.unwrap_or_default();
        let prompt =
            crate::runner::build_prompt(&scan, &stack, diff.as_deref(), &mem_entries, open_now);

        // Acquire concurrency permit + run the scan.
        let _permit = self.inner.sem.acquire().await.ok();
        let measure_from = Utc::now();
        // Stream each stdout line to the live console (ephemeral; not persisted)
        // and append to the per-run buffer for mid-run tab re-opens.
        let inner = self.inner.clone();
        let run_kind = kind.to_string();
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
                    ts: Utc::now().timestamp_millis(),
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
            info!(slug = %slug, "scan run cancelled by user");
            return RunOutcome::Cancelled;
        }
        if let Some(err) = exec.spawn_error {
            let _ = runs.finish_failed(run_id, &err).await;
            self.note_failure(&slug, kind, memory).await;
            warn!(slug = %slug, %err, "scan spawn failed");
            return RunOutcome::Failed;
        }
        if !exec.exit_ok {
            let msg = if exec.stderr.is_empty() {
                "scan agent exited non-zero".to_string()
            } else {
                exec.stderr.clone()
            };
            let _ = runs.finish_failed(run_id, &msg).await;
            self.note_failure(&slug, kind, memory).await;
            warn!(slug = %slug, "scan run failed");
            return RunOutcome::Failed;
        }

        // Success — measure how many findings the scan touched during the run.
        let delta = findings
            .count_touched_since(&slug, kind, measure_from)
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

        // Record the freshness watermark for the next run's gate: the reviewed
        // git SHA for code-gated scans, the data watermark for data-gated scans.
        match scan.gate {
            Gate::Code => {
                if let Some(h) = &head {
                    let _ = memory
                        .upsert(
                            &slug,
                            "last_run",
                            &sha_key(kind),
                            &serde_json::Value::String(h.clone()),
                            None,
                        )
                        .await;
                }
            }
            Gate::Data => {
                if let Some(w) = &data_watermark {
                    if !w.is_empty() {
                        let _ = memory
                            .upsert(
                                &slug,
                                "last_run",
                                &watermark_key(kind),
                                &serde_json::Value::String(w.clone()),
                                None,
                            )
                            .await;
                    }
                }
            }
            Gate::Manual => {}
        }
        // Reset consecutive-failure counter on success.
        let _ = memory
            .delete(&slug, "last_run", &format!("{kind}:consecutive_failures"))
            .await;

        info!(slug = %slug, findings = delta, empty, "scan run success");
        if empty {
            RunOutcome::Empty
        } else {
            RunOutcome::Success
        }
    }

    /// Track consecutive failures. After 3 in a row we just log loudly — a
    /// proper meta-finding / Hub ping needs schema + Hub wiring (deferred).
    async fn note_failure(&self, slug: &str, kind: &str, memory: &MemoryStore) {
        let key = format!("{kind}:consecutive_failures");
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
                count = next,
                "surveillance: 3+ consecutive failures — check scan agent auth/install"
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
