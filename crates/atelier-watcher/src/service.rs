use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{Semaphore, broadcast, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{error, info, warn};
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

/// Erreur typée du single-flight par (app, kind) — comparée telle quelle par le
/// handler HTTP (`routes/surveillance.rs`) pour mapper en 409, même mécanique
/// que le conflit sweep ("sweep already running").
pub const ERR_SCAN_ALREADY_RUNNING: &str = "scan already running for this app/kind";

/// Écriture best-effort (watermark, mémoire) : l'échec ne doit pas faire échouer
/// le scan, mais il doit se VOIR. Les statuts TERMINAUX de run passent par
/// `finish_with_retry` ci-dessous, pas par ce helper.
fn warn_if_err<T, E: std::fmt::Display>(op: &'static str, res: Result<T, E>) {
    if let Err(e) = res {
        warn!(op, error = %e, "best-effort write failed");
    }
}

/// Écriture TERMINALE de run (`finish_*`) avec retry borné (2s/5s/10s). WHY :
/// contrairement aux écritures de progression (best-effort via `warn_if_err`),
/// une écriture terminale perdue laisse la row `running` à jamais (dashboard
/// bloqué jusqu'au prochain boot) — typiquement Postgres indisponible à la FIN
/// d'un long scan. Après épuisement des retries on abandonne en `error!` : le
/// reaper périodique (`stale_run_reaper`) rattrapera la row.
async fn finish_with_retry<F, Fut>(op: &'static str, mut write: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    const BACKOFF_SECS: [u64; 3] = [2, 5, 10];
    let mut attempt = 0;
    loop {
        match write().await {
            Ok(()) => return,
            Err(e) if attempt < BACKOFF_SECS.len() => {
                warn!(op, attempt = attempt + 1, error = %e, "terminal run write failed — retrying");
                tokio::time::sleep(Duration::from_secs(BACKOFF_SECS[attempt])).await;
                attempt += 1;
            }
            Err(e) => {
                error!(op, error = %e, "terminal run write failed after retries — row left 'running' until reaped");
                return;
            }
        }
    }
}

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

/// One in-flight scan tracked in `Inner::running`, keyed by (slug, kind).
/// `run_id` is `None` only during the short window between the single-flight
/// reservation and the DB row creation (`launch_run`). `cancel` is `take()`n on
/// cancel, but the entry itself stays until `execute` settles — so a new run of
/// the same pair cannot start while the killed subprocess winds down.
struct RunningScan {
    run_id: Option<Uuid>,
    cancel: Option<oneshot::Sender<()>>,
}

/// Provider ASYNC du token d'auth SDK à injecter dans le stdin de CHAQUE run de
/// scan. Relu à chaque run (pas mis en cache) → une ré-auth depuis Paramètres
/// s'applique sans redémarrer le service. Construit par `main.rs` (qui a accès au
/// store `agent_auth`) pour éviter une dépendance du watcher vers `atelier-common`.
pub type TokenProvider = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>
        + Send
        + Sync,
>;

/// Sink appelé quand un scan détecte `authentication_failed` : la closure (dans
/// `main.rs`) déduplique (claim `agent_auth`) et pousse UNE notification plateforme.
/// Le watcher ne connaît ni le store ni le NotificationStore — juste ce callback.
pub type AuthFailureSink = Arc<dyn Fn(String) + Send + Sync>;

/// Sink appelé quand le git_watcher clôt un item backlog depuis un commit
/// `fix(backlog:<id>)`. `main.rs` y branche `BacklogStore::republish` (event
/// `pilot:backlog`) — même patron d'inversion que les deux types ci-dessus :
/// le watcher ne dépend jamais d'atelier-pilot.
pub type BacklogSettledSink = Arc<dyn Fn(i64) + Send + Sync>;

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
    /// Reciprocal platform gate (Pilote/backup). Installed by the control plane
    /// after all services exist, so the watcher crate stays dependency-free.
    external_busy: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
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
    /// In-flight scans keyed by (slug, kind) — la clé EST le single-flight par
    /// couple : check + insertion atomiques sous ce lock dans `launch_run`
    /// (AVANT la création de la row, sinon fenêtre TOCTOU), retrait par
    /// `execute` quand le run se termine.
    running: Mutex<HashMap<(String, String), RunningScan>>,
    enabled: bool,
    /// Token d'auth SDK frais par run (injecté dans le stdin de scan.js). `None` =
    /// pas de token configuré → scan.js retombe sur le `.credentials.json` local.
    token_provider: Option<TokenProvider>,
    /// Remontée d'un `authentication_failed` détecté par un scan (dédup + notif).
    on_auth_failure: Option<AuthFailureSink>,
}

#[derive(Debug, Clone, Default)]
pub struct SurveillanceConfig {
    pub admin_dsn: Option<String>,
    pub db_name: Option<String>,
    /// Apps known to the service — stack hints for prompts + git_watcher slugs.
    pub seed_apps: Vec<AppMeta>,
    /// Root of app sources: `<root>/<slug>/src/`.
    pub apps_src_root: PathBuf,
    /// Racine du dépôt source d'Atelier — scannée par le git_watcher pour les
    /// commits `fix(backlog:<id>)` du scope 'atelier' uniquement (Atelier n'a
    /// pas de findings de surveillance). `None` = pas de couverture Atelier.
    pub atelier_src_root: Option<PathBuf>,
    /// AI engine for scans — the Claude Agent SDK (OAuth subscription, run as
    /// `hr-studio` via `scan.js`; never an API key).
    pub driver: ClaudeScanConfig,
    /// Max concurrent scan subprocesses (ratelimit guard).
    pub max_concurrent: usize,
}

impl SurveillanceService {
    /// `token_provider` / `on_auth_failure` / `on_backlog_settled` : câblages runtime
    /// (cf. types). Passés en params plutôt que dans `SurveillanceConfig` (qui dérive
    /// Debug/Clone/Default — incompatible avec des `Arc<dyn Fn…>`). `None` = auth SDK
    /// non gérée par Atelier (le scan retombe sur le `.credentials.json` local, pas de
    /// notif) / pas de republication live des items backlog clos par commit.
    pub async fn start(
        cfg: SurveillanceConfig,
        token_provider: Option<TokenProvider>,
        on_auth_failure: Option<AuthFailureSink>,
        on_backlog_settled: Option<BacklogSettledSink>,
    ) -> Self {
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
                external_busy: Mutex::new(None),
                sweep: Mutex::new(None),
                sweep_tx,
                sem: Arc::new(Semaphore::new(cfg.max_concurrent.max(1))),
                tx,
                transcript_tx,
                transcripts: Mutex::new(HashMap::new()),
                running: Mutex::new(HashMap::new()),
                enabled,
                token_provider,
                on_auth_failure,
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
            // Réconciliation périodique en ligne : un run dont l'écriture terminale
            // a été perdue (Postgres down à la fin du scan, malgré les retries de
            // `finish_with_retry`) resterait 'running' jusqu'au prochain boot — le
            // reaper le rattrape toutes les 10 min.
            tokio::spawn(svc.clone().stale_run_reaper(cfg.driver.timeout * 2));
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
                    cfg.atelier_src_root.clone(),
                    f,
                    m,
                    svc.inner.tx.clone(),
                    pool.as_ref().expect("enabled watcher pool").clone(),
                    on_backlog_settled,
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

    /// True while an individual scan or a full sweep owns the surveillance
    /// execution lane. Used by Pilote's reciprocal scheduler gate.
    pub fn is_busy(&self) -> bool {
        if !self.inner.running.lock().unwrap().is_empty() {
            return true;
        }
        self.inner.sweep.lock().unwrap().as_ref().map(|s| {
            matches!(s.snapshot.status, SweepStatus::Running | SweepStatus::Cancelling)
        }).unwrap_or(false)
    }

    /// Install the reciprocal scheduler gate. Manual and scheduled sweeps share
    /// `start_sweep`, therefore neither can race an autonomous Pilote run.
    pub fn set_external_busy(&self, gate: Arc<dyn Fn() -> bool + Send + Sync>) {
        *self.inner.external_busy.lock().unwrap() = Some(gate);
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
        if self
            .inner
            .external_busy
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|gate| gate())
        {
            return Err("another platform job is in progress".into());
        }
        // Exclusion mutuelle avec les scans individuels (Studio/manuels) : refuser le
        // sweep tant qu'un scan est en vol — sa relecture FORCÉE du même app+kind
        // collisionnerait sur les findings (double run concurrent, triage incohérent).
        // Ici aucun sweep n'est encore actif (single-flight juste en dessous), donc tout
        // run présent dans `running` est forcément un scan unitaire. Couvre le bouton ET
        // le scheduler (`start_sweep` est l'unique point d'entrée des deux).
        if !self.inner.running.lock().unwrap().is_empty() {
            return Err("scan in progress".into());
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
    /// as `cancelled`); false if it already finished, is already cancelling, or
    /// never existed. Le sender est `take()`n mais l'entrée (slug, kind) reste
    /// dans `running` jusqu'à ce que `execute` se termine — le single-flight
    /// tient donc pendant toute la descente du process annulé.
    pub fn cancel_run(&self, run_id: Uuid) -> bool {
        let mut running = self.inner.running.lock().unwrap();
        for scan in running.values_mut() {
            if scan.run_id == Some(run_id) {
                if let Some(tx) = scan.cancel.take() {
                    let _ = tx.send(());
                    return true;
                }
                return false;
            }
        }
        false
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
        // Le single-flight par (slug, kind) est appliqué dans `launch_run`
        // (Err(ERR_SCAN_ALREADY_RUNNING) → 409 côté HTTP).
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
        // Single-flight par (app, kind) : check + réservation ATOMIQUES sous le
        // même lock (un double-clic « Scanner » lançait deux scans concurrents du
        // même couple → findings dupliqués, triage incohérent). Réservé AVANT la
        // création de la row : réserver après aurait laissé une fenêtre TOCTOU où
        // deux rows 'running' naissent. Couvre aussi une collision manuel/sweep.
        let key = (slug.clone(), kind.to_string());
        let (cancel_tx, cancel_rx) = oneshot::channel();
        {
            let mut running = self.inner.running.lock().unwrap();
            if running.contains_key(&key) {
                return Err(ERR_SCAN_ALREADY_RUNNING.into());
            }
            running.insert(
                key.clone(),
                RunningScan { run_id: None, cancel: Some(cancel_tx) },
            );
        }
        let run_id = match runs.start(&slug, kind, trigger, None).await {
            Ok(id) => id,
            Err(e) => {
                self.inner.running.lock().unwrap().remove(&key);
                return Err(format!("failed to create run: {e}"));
            }
        };
        if let Some(scan) = self.inner.running.lock().unwrap().get_mut(&key) {
            scan.run_id = Some(run_id);
        }
        let svc = self.clone();
        let kind = kind.to_string();
        let handle = tokio::spawn(async move {
            svc.execute(run_id, slug, kind, force, data_watermark, cancel_rx)
                .await
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
        cancel_rx: oneshot::Receiver<()>,
    ) -> RunOutcome {
        // Le slot single-flight (slug, kind) + le canal cancel ont été réservés
        // par `launch_run` (atomiquement, avant la row) ; cette fonction possède
        // leur libération une fois le run terminé.
        self.emit("run", &slug, "started");
        let outcome = self
            .execute_inner(run_id, &slug, &kind, force, data_watermark, cancel_rx)
            .await;
        self.inner
            .running
            .lock()
            .unwrap()
            .remove(&(slug.clone(), kind.clone()));
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
                    finish_with_retry("finish_skipped", || {
                        runs.finish_skipped(run_id, "blank (scan non défini)")
                    })
                    .await;
                    info!(slug = %slug, kind, "run skipped (blank scan)");
                    return RunOutcome::Skipped;
                }
                Err(e) => {
                    let msg = format!("scan load failed: {e}");
                    finish_with_retry("finish_failed", || runs.finish_failed(run_id, &msg)).await;
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
            finish_with_retry("finish_skipped", || runs.finish_skipped(run_id, &reason)).await;
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
                            finish_with_retry("finish_skipped", || {
                                runs.finish_skipped(run_id, "no_diff (HEAD unchanged)")
                            })
                            .await;
                            info!(slug = %slug, "run skipped (no_diff)");
                            return RunOutcome::Skipped;
                        }
                        (Some(last), Some(_)) => {
                            let d = gitutil::diff_since(&src, last).await;
                            if d.is_none() {
                                finish_with_retry("finish_skipped", || {
                                    runs.finish_skipped(run_id, "no_diff (empty range)")
                                })
                                .await;
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
                            finish_with_retry("finish_skipped", || {
                                runs.finish_skipped(run_id, "no_new_data")
                            })
                            .await;
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
                                finish_with_retry("finish_skipped", || {
                                    runs.finish_skipped(run_id, "no_new_data")
                                })
                                .await;
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
        // Token d'auth SDK FRAIS (relu par run → ré-auth sans restart). None → scan.js
        // retombe sur le .credentials.json local.
        let oauth_token = match self.inner.token_provider.as_ref() {
            Some(tp) => tp().await,
            None => None,
        };
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
            .exec(&src, &prompt, oauth_token, cancel_rx, |line| {
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
            finish_with_retry("finish_cancelled", || runs.finish_cancelled(run_id)).await;
            info!(slug = %slug, "scan run cancelled by user");
            return RunOutcome::Cancelled;
        }
        if let Some(err) = exec.spawn_error {
            finish_with_retry("finish_failed", || runs.finish_failed(run_id, &err)).await;
            self.note_failure(&slug, kind, memory).await;
            warn!(slug = %slug, %err, "scan spawn failed");
            return RunOutcome::Failed;
        }
        // Échec MCP fatal signalé par scan.js (auth/connexion) : l'agent écrit ses
        // findings via les tools MCP — ce run n'a rien pu enregistrer, le marquer
        // success_empty serait un faux « tout est clean ». FAILED explicite, même
        // quand le process sort en exit 0. Testé AVANT exit_ok : le message typé
        // prime sur le stderr brut quand scan.js a avorté (exit 2) sur ce cas.
        if let Some(mcp_err) = exec.mcp_error.as_deref() {
            finish_with_retry("finish_failed", || runs.finish_failed(run_id, mcp_err)).await;
            self.note_failure(&slug, kind, memory).await;
            error!(slug = %slug, kind, error = %mcp_err, "scan MCP failure — findings not recorded");
            return RunOutcome::Failed;
        }
        // Échec d'AUTH SDK (`authentication_failed` : token OAuth abonnement mort/
        // révoqué) signalé par scan.js. On remonte au sink (dédup + notification
        // plateforme) et on marque FAILED. Un token mort touche tous les scans du
        // sweep → le sink déduplique en une seule notif via le claim `agent_auth`.
        if let Some(auth_err) = exec.auth_error.as_deref() {
            if let Some(sink) = self.inner.on_auth_failure.as_ref() {
                sink(auth_err.to_string());
            }
            finish_with_retry("finish_failed", || runs.finish_failed(run_id, auth_err)).await;
            self.note_failure(&slug, kind, memory).await;
            error!(slug = %slug, kind, error = %auth_err, "scan SDK auth failure — token OAuth expiré/révoqué");
            return RunOutcome::Failed;
        }
        if !exec.exit_ok {
            let msg = if exec.stderr.is_empty() {
                "scan agent exited non-zero".to_string()
            } else {
                exec.stderr.clone()
            };
            finish_with_retry("finish_failed", || runs.finish_failed(run_id, &msg)).await;
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
        finish_with_retry("finish_success", || {
            runs.finish_success(
                run_id,
                delta as i32,
                exec.tokens_in,
                exec.tokens_out,
                head.as_deref(),
                empty,
            )
        })
        .await;

        // Record the freshness watermark for the next run's gate: the reviewed
        // git SHA for code-gated scans, the data watermark for data-gated scans.
        match scan.gate {
            Gate::Code => {
                if let Some(h) = &head {
                    warn_if_err(
                        "watermark_sha",
                        memory
                            .upsert(
                                &slug,
                                "last_run",
                                &sha_key(kind),
                                &serde_json::Value::String(h.clone()),
                                None,
                            )
                            .await,
                    );
                }
            }
            Gate::Data => {
                if let Some(w) = &data_watermark {
                    if !w.is_empty() {
                        warn_if_err(
                            "watermark_data",
                            memory
                                .upsert(
                                    &slug,
                                    "last_run",
                                    &watermark_key(kind),
                                    &serde_json::Value::String(w.clone()),
                                    None,
                                )
                                .await,
                        );
                    }
                }
            }
            Gate::Manual => {}
        }
        // Reset consecutive-failure counter on success.
        warn_if_err(
            "reset_consecutive_failures",
            memory
                .delete(&slug, "last_run", &format!("{kind}:consecutive_failures"))
                .await,
        );

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

    /// Boucle de réconciliation périodique (10 min) : marque `failed` les rows
    /// encore `running` plus vieilles que `stale_after` (2× le timeout de scan)
    /// ET absentes de la map `running` en mémoire — forcément des fantômes
    /// (écriture terminale perdue pendant une indispo Postgres, malgré les
    /// retries de `finish_with_retry`). Pendant du `reconcile_interrupted()` de
    /// boot, mais en ligne : plus besoin d'attendre un restart.
    async fn stale_run_reaper(self, stale_after: Duration) {
        let stale = chrono::Duration::from_std(stale_after)
            .unwrap_or_else(|_| chrono::Duration::seconds(1200));
        let mut tick = tokio::time::interval(Duration::from_secs(600));
        loop {
            tick.tick().await;
            let Some(runs) = self.inner.runs.as_ref() else {
                return; // noop mode — jamais spawné dans ce cas, pure ceinture.
            };
            let in_flight: Vec<Uuid> = self
                .inner
                .running
                .lock()
                .unwrap()
                .values()
                .filter_map(|s| s.run_id)
                .collect();
            match runs.fail_stale_running(Utc::now() - stale, &in_flight).await {
                Ok(0) => {}
                Ok(n) => warn!(count = n, "surveillance: stale 'running' runs reaped (marked failed)"),
                Err(e) => warn!(?e, "surveillance: stale-run reaper tick failed"),
            }
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
