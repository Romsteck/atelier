use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;
use tokio::sync::{Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::backlog::{BacklogItem, BacklogStore, NewBacklogItem, Question};
use crate::engine::{ClaudeWorkerEngine, CodexWorkerEngine, EnginePolicy, WorkerExec};
use crate::gitops;
use crate::runs::RunsStore;
use crate::schedule::{NightSnapshot, PilotSchedule, ScheduleStore, due};
use crate::sqlx::PgPool;

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub type TokenProvider = Arc<dyn Fn() -> BoxFuture<Option<String>> + Send + Sync>;
pub type ActionHook = Arc<dyn Fn(String) -> BoxFuture<Result<(), String>> + Send + Sync>;
pub type HealthHook = Arc<dyn Fn(String) -> BoxFuture<Result<(), String>> + Send + Sync>;
pub type NotifyHook =
    Arc<dyn Fn(Option<String>, String, String, String) -> BoxFuture<()> + Send + Sync>;
pub type FindingsHook = Arc<dyn Fn() -> BoxFuture<Vec<PilotFinding>> + Send + Sync>;
pub type ResolveFindingHook =
    Arc<dyn Fn(i64, Option<String>) -> BoxFuture<Result<(), String>> + Send + Sync>;
pub type AppSlugsHook = Arc<dyn Fn() -> BoxFuture<Vec<String>> + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PilotFinding {
    pub id: i64,
    pub slug: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub plan: String,
}

#[derive(Clone)]
pub struct PilotHooks {
    pub token: TokenProvider,
    pub build: ActionHook,
    pub ship: ActionHook,
    pub health: HealthHook,
    pub notify: NotifyHook,
    pub live_sessions: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub platform_busy: Arc<dyn Fn() -> bool + Send + Sync>,
    pub findings: FindingsHook,
    pub resolve_finding: ResolveFindingHook,
    pub app_slugs: AppSlugsHook,
    // Auth SDK morte (sdk_auth_failed) : main.rs câble agent_auth.record_failure
    // + la notification plateforme dédupliquée. Appelé UNE fois par événement.
    pub on_auth_failure: Arc<dyn Fn(String) + Send + Sync>,
}

impl Default for PilotHooks {
    fn default() -> Self {
        Self {
            token: Arc::new(|| Box::pin(async { None })),
            build: Arc::new(|_| Box::pin(async { Err("hooks Pilote non configurés".into()) })),
            ship: Arc::new(|_| Box::pin(async { Err("hooks Pilote non configurés".into()) })),
            health: Arc::new(|_| Box::pin(async { Err("hooks Pilote non configurés".into()) })),
            notify: Arc::new(|_, _, _, _| Box::pin(async {})),
            live_sessions: Arc::new(|_| false),
            platform_busy: Arc::new(|| false),
            findings: Arc::new(|| Box::pin(async { Vec::new() })),
            resolve_finding: Arc::new(|_, _| Box::pin(async { Ok(()) })),
            app_slugs: Arc::new(|| Box::pin(async { Vec::new() })),
            on_auth_failure: Arc::new(|_| {}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PilotConfig {
    pub apps_src_root: PathBuf,
    pub atelier_root: PathBuf,
    pub app_user: String,
    pub atelier_user: String,
    pub model: String,
    pub effort: String,
    pub timeout: Duration,
    pub engine: ClaudeWorkerEngine,
    pub codex_engine: Option<CodexWorkerEngine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PilotEvent {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<BacklogItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptLine {
    pub run_id: Uuid,
    pub scope: String,
    pub seq: u64,
    pub ts: i64,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtelierWorkerReport {
    pub run_id: Uuid,
    pub secret: String,
    pub item_id: i64,
    pub status: String,
    pub commit_sha: Option<String>,
    pub report: Option<String>,
    pub error: Option<String>,
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub checkpoint_sha: Option<String>,
    #[serde(default)]
    pub git_sha_before: Option<String>,
}

struct RunningWork {
    run_id: Uuid,
    cancel: CancellationToken,
    trigger: String,
}

/// Rings de transcript bornés à N runs : sans purge, la map mémoire grossissait
/// indéfiniment (un ring de 500 lignes par run, jamais libéré).
#[derive(Default)]
struct TranscriptBuf {
    rings: HashMap<Uuid, VecDeque<TranscriptLine>>,
    order: VecDeque<Uuid>,
}

const TRANSCRIPT_KEEP_RUNS: usize = 8;

/// Contexte d'échec embarqué dans le prompt de la tentative suivante :
/// phase échouée + erreur + fin de transcript (retry enrichi).
struct RetryContext {
    phase: String,
    error: String,
    transcript_tail: String,
}

#[derive(Debug, Default, Serialize)]
struct EngineNightStats {
    runs: u64,
    success: u64,
    failed: u64,
    tokens_in: i64,
    tokens_out: i64,
}

struct Inner {
    enabled: bool,
    backlog: Option<BacklogStore>,
    runs: Option<RunsStore>,
    schedule: Option<ScheduleStore>,
    config: PilotConfig,
    hooks: RwLock<PilotHooks>,
    running: Mutex<HashMap<String, RunningWork>>,
    transcript: Mutex<TranscriptBuf>,
    backlog_tx: broadcast::Sender<PilotEvent>,
    transcript_tx: broadcast::Sender<TranscriptLine>,
    night_tx: broadcast::Sender<NightSnapshot>,
    night_cancel: Mutex<Option<CancellationToken>>,
    detached_atelier: AtomicBool,
    activated: AtomicBool,
}

#[derive(Clone)]
pub struct PilotService {
    inner: Arc<Inner>,
}

impl PilotService {
    pub async fn start(pool: Option<PgPool>, config: PilotConfig) -> Self {
        let (backlog_tx, _) = broadcast::channel(256);
        let (transcript_tx, _) = broadcast::channel(1024);
        let (night_tx, _) = broadcast::channel(64);
        let mut stores = None;
        if let Some(pool) = pool {
            match crate::run_migrations(&pool).await {
                Ok(()) => {
                    stores = Some((
                        BacklogStore::new(pool.clone()).with_events(backlog_tx.clone()),
                        RunsStore::new(pool.clone()),
                        ScheduleStore::new(pool),
                    ))
                }
                Err(e) => error!(error=%e, "pilot migrations failed — service disabled"),
            }
        }
        let (backlog, runs, schedule) = match stores {
            Some((b, r, s)) => (Some(b), Some(r), Some(s)),
            None => (None, None, None),
        };
        let svc = Self {
            inner: Arc::new(Inner {
                enabled: backlog.is_some(),
                backlog,
                runs,
                schedule,
                config,
                hooks: RwLock::new(PilotHooks::default()),
                running: Mutex::new(HashMap::new()),
                transcript: Mutex::new(TranscriptBuf::default()),
                backlog_tx,
                transcript_tx,
                night_tx,
                night_cancel: Mutex::new(None),
                detached_atelier: AtomicBool::new(false),
                activated: AtomicBool::new(false),
            }),
        };
        svc
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }
    pub fn codex_worker_enabled(&self) -> bool {
        self.inner.config.codex_engine.is_some()
    }
    pub fn backlog(&self) -> Option<BacklogStore> {
        self.inner.backlog.clone()
    }
    pub fn runs(&self) -> Option<RunsStore> {
        self.inner.runs.clone()
    }
    pub fn schedules(&self) -> Option<ScheduleStore> {
        self.inner.schedule.clone()
    }
    pub fn configure_hooks(&self, hooks: PilotHooks) {
        *self.inner.hooks.write().expect("pilot hooks") = hooks;
        if self.inner.enabled && !self.inner.activated.swap(true, Ordering::SeqCst) {
            let svc = self.clone();
            tokio::spawn(async move {
                // The detached Atelier worker survives a deploy restart. Consume
                // its durable result only after real notification/finding hooks
                // are present, then reconcile all other interrupted work.
                svc.reconcile_atelier_worker().await;
                if let (Some(b), Some(r)) = (&svc.inner.backlog, &svc.inner.runs) {
                    // Les arbres des runs app orphelins doivent être restaurés
                    // AVANT que reconcile_interrupted ne les marque failed
                    // (sinon le diff d'un run interrompu reste dans le worktree).
                    svc.restore_orphan_trees().await;
                    if let Err(e) = b.reconcile_boot().await {
                        warn!(error=%e, "pilot backlog reconciliation failed");
                    }
                    if let Err(e) = r.reconcile_interrupted().await {
                        warn!(error=%e, "pilot runs reconciliation failed");
                    }
                    let _ = r.prune().await;
                }
                svc.scheduler_loop().await;
            });
        }
    }
    fn hooks(&self) -> PilotHooks {
        self.inner.hooks.read().expect("pilot hooks").clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<PilotEvent> {
        self.inner.backlog_tx.subscribe()
    }
    pub fn subscribe_transcript(&self) -> broadcast::Receiver<TranscriptLine> {
        self.inner.transcript_tx.subscribe()
    }
    pub fn subscribe_night(&self) -> broadcast::Receiver<NightSnapshot> {
        self.inner.night_tx.subscribe()
    }
    pub fn is_busy(&self) -> bool {
        !self.inner.running.lock().expect("pilot running").is_empty()
            || self
                .inner
                .night_cancel
                .lock()
                .expect("pilot night")
                .is_some()
            || self.inner.detached_atelier.load(Ordering::Relaxed)
    }

    pub fn publish(&self, action: &str, item: Option<BacklogItem>, id: Option<i64>) {
        let _ = self.inner.backlog_tx.send(PilotEvent {
            action: action.into(),
            item,
            id,
        });
    }

    pub fn transcript(&self, run_id: Uuid) -> Vec<TranscriptLine> {
        self.inner
            .transcript
            .lock()
            .expect("pilot transcript")
            .rings
            .get(&run_id)
            .map(|v| v.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[instrument(skip(self))]
    pub async fn run_item(&self, id: i64, trigger: &str) -> Result<Uuid, String> {
        let backlog = self.inner.backlog.as_ref().ok_or("Pilote indisponible")?;
        let item = backlog
            .get(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("item introuvable")?;
        if item.needs_user {
            return Err("item bloqué dans l’attente de réponses".into());
        }
        if !matches!(item.lane.as_str(), "inbox" | "ready" | "attention") {
            return Err("item non exécutable dans cette colonne".into());
        }
        if (self.hooks().live_sessions)(&item.scope) {
            return Err("conversation interactive active sur ce scope".into());
        }
        // Anti-race post-restart : la réservation mémoire (map running +
        // detached_atelier) ne survit pas au redémarrage, alors que l'unité
        // systemd du worker atelier si — la DB (pilot_night.atelier_unit) +
        // systemd font foi avant de relancer quoi que ce soit sur ce scope.
        if item.scope == "atelier"
            && let Some(schedules) = self.inner.schedule.as_ref()
            && let Ok(night) = schedules.night().await
            && let Some(unit) = night.atelier_unit
        {
            let active = Command::new("systemctl")
                .args(["is-active", "--quiet", &unit])
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if active {
                return Err("worker atelier déjà en vol".into());
            }
        }
        let run_id = Uuid::new_v4();
        let trigger = if trigger == "night" {
            "night"
        } else {
            "manual"
        }
        .to_string();
        {
            let mut running = self.inner.running.lock().expect("pilot running");
            if running.contains_key(&item.scope) {
                return Err("un run Pilote est déjà actif sur ce scope".into());
            }
            running.insert(
                item.scope.clone(),
                RunningWork {
                    run_id,
                    cancel: CancellationToken::new(),
                    trigger: trigger.clone(),
                },
            );
        }
        let Some(queued) = backlog
            .mark_queued(id, run_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            self.inner
                .running
                .lock()
                .expect("pilot running")
                .remove(&item.scope);
            return Err("item déjà en cours ou non exécutable".into());
        };
        info!(item_id = id, run_id = %run_id, scope = %item.scope, trigger = %trigger, "pilot run start");
        self.publish("exec", Some(queued), Some(id));
        let svc = self.clone();
        tokio::spawn(async move {
            svc.execute_with_retries(item, run_id, &trigger).await;
        });
        Ok(run_id)
    }

    pub fn cancel_run(&self, run_id: Uuid) -> bool {
        let running = self.inner.running.lock().expect("pilot running");
        if let Some(w) = running.values().find(|w| w.run_id == run_id) {
            w.cancel.cancel();
            true
        } else {
            false
        }
    }

    async fn execute_with_retries(&self, original: BacklogItem, first_run: Uuid, trigger: &str) {
        let scope = original.scope.clone();
        let cancel = self
            .inner
            .running
            .lock()
            .expect("pilot running")
            .get(&scope)
            .map(|w| w.cancel.clone())
            .unwrap_or_else(CancellationToken::new);
        let mut last_error = String::new();
        let mut last_failure = String::new();
        let mut last_retry: Option<RetryContext> = None;
        let mut last_engine: Option<String> = None;
        let mut attempts_done = 0;
        let mut final_item = None;
        for attempt in 1..=3 {
            if cancel.is_cancelled() {
                // Cancel constaté entre deux tentatives : outcome `cancelled`,
                // pas un échec — l'item repart ready sans consommer de
                // tentative, jamais blocked ni notif error.
                info!(item_id = original.id, scope = %scope, "pilot run cancelled between attempts");
                final_item = self
                    .inner
                    .backlog
                    .as_ref()
                    .unwrap()
                    .defer_ready(original.id)
                    .await
                    .ok();
                break;
            }
            let run_id = if attempt == 1 {
                first_run
            } else {
                Uuid::new_v4()
            };
            if let Some(w) = self
                .inner
                .running
                .lock()
                .expect("pilot running")
                .get_mut(&scope)
            {
                w.run_id = run_id;
            }
            let engine_name = self.select_engine(&original, attempt, &last_failure).await;
            last_engine = Some(engine_name.to_string());
            let model = if engine_name == "codex" {
                self.inner
                    .config
                    .codex_engine
                    .as_ref()
                    .map(|e| e.model.as_str())
            } else {
                Some(self.inner.config.model.as_str())
            };
            let runs = match self.inner.runs.as_ref() {
                Some(r) => r,
                None => break,
            };
            let run_kind = if original.kind == "finding_fix" && original.finding_id.is_some() {
                "findings"
            } else {
                "item"
            };
            if let Err(e) = runs
                .start(
                    run_id,
                    Some(original.id),
                    &scope,
                    run_kind,
                    trigger,
                    attempt,
                    engine_name,
                    model,
                )
                .await
            {
                last_error = e.to_string();
                break;
            }
            let item = match self
                .inner
                .backlog
                .as_ref()
                .unwrap()
                .mark_running(original.id, attempt)
                .await
            {
                Ok(i) => i,
                Err(e) => {
                    last_error = e.to_string();
                    break;
                }
            };
            attempts_done = attempt;
            info!(item_id = item.id, run_id = %run_id, scope = %scope, attempt, engine = engine_name, "pilot attempt start");
            self.publish("exec", Some(item.clone()), Some(item.id));
            let result = self
                .execute_once(
                    &item,
                    run_id,
                    attempt,
                    last_retry.as_ref(),
                    engine_name,
                    trigger,
                    cancel.clone(),
                )
                .await;
            match result {
                AttemptOutcome::Detached => {
                    // The systemd unit owns the rest of this attempt. Its report
                    // (possibly after Atelier restarted) performs the terminal DB
                    // transition and clears the in-memory scope reservation.
                    return;
                }
                AttemptOutcome::Success { commit, exec } => {
                    let _ = runs
                        .finish_success(
                            run_id,
                            commit.as_deref(),
                            exec.final_report.as_deref(),
                            exec.tokens_in,
                            exec.tokens_out,
                        )
                        .await;
                    match self
                        .inner
                        .backlog
                        .as_ref()
                        .unwrap()
                        .settle_done(item.id, commit.as_deref(), Some(engine_name))
                        .await
                    {
                        Ok(done) => {
                            info!(item_id = done.id, scope = %scope, commit = ?commit, engine = engine_name, "pilot item done");
                            // Garde-fou : ne résoudre le finding QUE si un commit
                            // existe — sans commit, rien ne prouve la correction
                            // (un Done-sans-changement ne résout jamais à tort).
                            if commit.is_some()
                                && let Some(fid) = done.finding_id
                            {
                                let _ = (self.hooks().resolve_finding)(fid, commit.clone()).await;
                            }
                            (self.hooks().notify)(
                                Some(scope.clone()),
                                "info".into(),
                                format!("Livré : {}", done.title),
                                commit.clone().unwrap_or_else(|| {
                                    "Aucun changement de code nécessaire".into()
                                }),
                            )
                            .await;
                            final_item = Some(done);
                        }
                        Err(e) => last_error = e.to_string(),
                    }
                    break;
                }
                AttemptOutcome::NeedsUser {
                    reason,
                    questions,
                    exec,
                } => {
                    let _ = runs
                        .finish_failure(
                            run_id,
                            "attention",
                            "needs_user",
                            &reason,
                            exec.final_report.as_deref(),
                            Some(&exec.lines.join("\n")),
                            exec.tokens_in,
                            exec.tokens_out,
                        )
                        .await;
                    if let Ok(blocked) = self
                        .inner
                        .backlog
                        .as_ref()
                        .unwrap()
                        .settle_needs_user(item.id, &reason, &questions, Some(engine_name))
                        .await
                    {
                        info!(item_id = blocked.id, scope = %scope, "pilot item needs user");
                        (self.hooks().notify)(
                            Some(scope.clone()),
                            "warn".into(),
                            format!("L’agent a des questions : {}", blocked.title),
                            reason,
                        )
                        .await;
                        final_item = Some(blocked);
                    }
                    break;
                }
                AttemptOutcome::Deferred(reason) => {
                    let failure_reason = if reason.contains("conversation interactive") {
                        "deferred_live_session"
                    } else if reason.contains("annulé") {
                        "cancelled"
                    } else {
                        "deferred_busy"
                    };
                    let _ = runs
                        .finish_failure(
                            run_id,
                            "cancelled",
                            failure_reason,
                            &reason,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await;
                    final_item = self
                        .inner
                        .backlog
                        .as_ref()
                        .unwrap()
                        .defer_ready(item.id)
                        .await
                        .ok();
                    break;
                }
                AttemptOutcome::Failed {
                    reason,
                    error: err,
                    exec,
                    grave,
                } => {
                    last_error = err.clone();
                    let prev_failure = std::mem::replace(&mut last_failure, reason.clone());
                    // Retry enrichi : la tentative suivante voit la phase
                    // échouée, l'erreur et la fin de transcript du run raté.
                    last_retry = Some(RetryContext {
                        phase: reason.clone(),
                        error: err.clone(),
                        transcript_tail: exec_tail(&exec.lines),
                    });
                    let _ = runs
                        .finish_failure(
                            run_id,
                            "failed",
                            &reason,
                            &err,
                            exec.final_report.as_deref(),
                            Some(&exec.lines.join("\n")),
                            exec.tokens_in,
                            exec.tokens_out,
                        )
                        .await;
                    warn!(item_id = item.id, run_id = %run_id, scope = %scope, attempt, reason = %reason, "pilot attempt failed");
                    if reason == "sdk_auth_failed" {
                        // Panne systémique d'auth : AUCUN retry, mais l'item
                        // n'a rien fait de mal — il repart ready sans blocked
                        // ni notif individuelle et reviendra quand le token
                        // sera réparé. Le hook porte la remontée dédupliquée.
                        (self.hooks().on_auth_failure)(err.clone());
                        final_item = self
                            .inner
                            .backlog
                            .as_ref()
                            .unwrap()
                            .defer_ready(item.id)
                            .await
                            .ok();
                        break;
                    }
                    if grave {
                        break;
                    }
                    // Deux erreurs MCP consécutives = panne systémique : la
                    // 3e tentative échouerait pareil, on n'y va pas.
                    if reason == "mcp_error" && prev_failure == "mcp_error" {
                        break;
                    }
                }
            }
        }
        if final_item.is_none() {
            let reason = if attempts_done == 0 && last_failure.is_empty() && last_error.is_empty() {
                "Run annulé".to_string()
            } else {
                blocked_reason(attempts_done, &last_failure, &last_error)
            };
            if let Ok(blocked) = self
                .inner
                .backlog
                .as_ref()
                .unwrap()
                .settle_attention(original.id, true, &reason, last_engine.as_deref())
                .await
            {
                info!(item_id = blocked.id, scope = %scope, attempts = attempts_done, reason = %reason, "pilot item blocked");
                (self.hooks().notify)(
                    Some(scope.clone()),
                    "error".into(),
                    format!("Item bloqué : {}", blocked.title),
                    reason,
                )
                .await;
                final_item = Some(blocked);
            }
        }
        self.inner
            .running
            .lock()
            .expect("pilot running")
            .remove(&scope);
        if let Some(item) = final_item {
            self.publish("exec", Some(item.clone()), Some(item.id));
        }
    }

    async fn select_engine(
        &self,
        item: &BacklogItem,
        attempt: i32,
        previous_reason: &str,
    ) -> &'static str {
        let codex_enabled = self.inner.config.codex_engine.is_some();
        let auto_enabled = if let Some(schedule) = self.inner.schedule.as_ref() {
            schedule
                .get()
                .await
                .map(|s| s.engine_policy == "auto")
                .unwrap_or(false)
        } else {
            false
        };
        let policy = EnginePolicy {
            codex_enabled,
            auto_enabled,
        };
        let selected = policy.select(&item.engine, &item.effort);
        if attempt > 1
            && item.engine == "auto"
            && codex_enabled
            && matches!(previous_reason, "agent_error" | "build_failed")
        {
            if selected == "claude" {
                "codex"
            } else {
                "claude"
            }
        } else {
            selected
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_once(
        &self,
        item: &BacklogItem,
        run_id: Uuid,
        attempt: i32,
        retry: Option<&RetryContext>,
        engine_name: &str,
        trigger: &str,
        cancel: CancellationToken,
    ) -> AttemptOutcome {
        if (self.hooks().platform_busy)() {
            return AttemptOutcome::Deferred("backup ou surveillance en cours".into());
        }
        if (self.hooks().live_sessions)(&item.scope) {
            return AttemptOutcome::Deferred("conversation interactive active".into());
        }
        if item.scope == "atelier" {
            return match self
                .spawn_atelier_worker(item, run_id, attempt, retry, trigger)
                .await
            {
                Ok(()) => AttemptOutcome::Detached,
                Err(e) => AttemptOutcome::Failed {
                    reason: "spawn_error".into(),
                    error: e,
                    exec: WorkerExec::default(),
                    grave: false,
                },
            };
        }
        let cwd = self
            .inner
            .config
            .apps_src_root
            .join(&item.scope)
            .join("src");
        let user = &self.inner.config.app_user;
        let runs = self.inner.runs.as_ref().unwrap();
        let _ = runs.set_phase(run_id, "checkpoint").await;
        let checkpoint = match gitops::checkpoint(user, &cwd, &item.scope).await {
            Ok(v) => v,
            Err(e) => return failed("commit_failed", e.to_string(), true),
        };
        let sha_before = match gitops::head_sha(user, &cwd).await {
            Ok(s) => s,
            Err(e) => return failed("commit_failed", e.to_string(), true),
        };
        let _ = runs
            .set_git_state(run_id, checkpoint.as_deref(), Some(&sha_before))
            .await;
        let prompt = build_item_prompt(item, attempt, retry);
        let other_trees = self.snapshot_other_trees(&item.scope).await;
        let _ = runs.set_phase(run_id, "agent").await;
        let svc = self.clone();
        let scope = item.scope.clone();
        let exec = if engine_name == "codex" {
            let Some(worker) = self.inner.config.codex_engine.as_ref() else {
                return failed("spawn_error", "worker Codex indisponible".into(), false);
            };
            worker
                .exec(&cwd, &prompt, cancel, move |v| {
                    svc.push_transcript(run_id, &scope, v.to_string());
                })
                .await
        } else {
            let token = (self.hooks().token)().await;
            let mut worker_engine = self.inner.config.engine.clone();
            if let Some(base) = worker_engine.mcp_endpoint.clone() {
                worker_engine.mcp_endpoint =
                    Some(format!("{base}?scope=pilot-worker&project={}", item.scope));
            }
            worker_engine
                .exec(&cwd, &prompt, token.as_deref(), cancel, move |v| {
                    svc.push_transcript(run_id, &scope, v.to_string());
                })
                .await
        };
        if let Err(error) = self
            .enforce_cross_app_guard(&item.scope, &other_trees)
            .await
        {
            let rb = gitops::rollback(user, &cwd, &sha_before).await;
            return AttemptOutcome::Failed {
                reason: if rb.is_err() {
                    "revert_failed".into()
                } else {
                    "cross_app_write".into()
                },
                error,
                exec,
                grave: true,
            };
        }
        if exec.cancelled {
            // Un rollback raté laisse un arbre corrompu : c'est grave même sur
            // un simple cancel — jamais d'échec avalé ici (miroir du chemin
            // needs_user/report ci-dessous).
            if let Err(e) = gitops::rollback(user, &cwd, &sha_before).await {
                return AttemptOutcome::Failed {
                    reason: "revert_failed".into(),
                    error: e.to_string(),
                    exec,
                    grave: true,
                };
            }
            return AttemptOutcome::Deferred("run annulé".into());
        }
        if let Some(reason) = exec.failure_reason.clone() {
            let rollback = gitops::rollback(user, &cwd, &sha_before).await;
            if let Err(e) = rollback {
                return AttemptOutcome::Failed {
                    reason: "revert_failed".into(),
                    error: e.to_string(),
                    exec,
                    grave: true,
                };
            }
            return AttemptOutcome::Failed {
                reason,
                error: exec
                    .error
                    .clone()
                    .unwrap_or_else(|| "agent en échec".into()),
                exec,
                grave: false,
            };
        }
        let current = self
            .inner
            .backlog
            .as_ref()
            .unwrap()
            .get(item.id)
            .await
            .ok()
            .flatten();
        if let Some(updated) = current.filter(|i| i.needs_user) {
            if let Err(e) = gitops::rollback(user, &cwd, &sha_before).await {
                return AttemptOutcome::Failed {
                    reason: "revert_failed".into(),
                    error: e.to_string(),
                    exec,
                    grave: true,
                };
            }
            return AttemptOutcome::NeedsUser {
                reason: updated
                    .needs_user_reason
                    .unwrap_or_else(|| "Décision requise".into()),
                questions: updated.questions,
                exec,
            };
        }
        if let Some((reason, questions)) = worker_needs_user(exec.final_report.as_deref()) {
            let rb = gitops::rollback(user, &cwd, &sha_before).await;
            if let Err(e) = rb {
                return AttemptOutcome::Failed {
                    reason: "revert_failed".into(),
                    error: e.to_string(),
                    exec,
                    grave: true,
                };
            }
            return AttemptOutcome::NeedsUser {
                reason,
                questions,
                exec,
            };
        }
        let head = gitops::head_sha(user, &cwd).await.unwrap_or_default();
        if head != sha_before {
            let rb = gitops::rollback(user, &cwd, &sha_before).await;
            return AttemptOutcome::Failed {
                reason: if rb.is_err() {
                    "revert_failed"
                } else {
                    "head_moved"
                }
                .into(),
                error: "L’agent a créé un commit alors que seul l’orchestrateur y est autorisé"
                    .into(),
                exec,
                grave: rb.is_err(),
            };
        }
        let diff = gitops::status_porcelain(user, &cwd)
            .await
            .unwrap_or_default();
        if diff.trim().is_empty() {
            // Porcelain vide ≠ succès inconditionnel : le rapport tranche (le
            // cas needs_user du rapport est déjà traité par worker_needs_user
            // ci-dessus) — reste le Done-sans-commit. Exception : un
            // finding_fix sans commit ne doit JAMAIS mener à un resolve, on
            // rend la main à Romain.
            if item.kind == "finding_fix" {
                return AttemptOutcome::NeedsUser {
                    reason: "aucun changement appliqué — finding non résolu automatiquement".into(),
                    questions: Vec::new(),
                    exec,
                };
            }
            return AttemptOutcome::Success { commit: None, exec };
        }
        let hooks = self.hooks();
        let _ = runs.set_phase(run_id, "build").await;
        if let Err(e) = retry_busy(|| (hooks.build)(item.scope.clone())).await {
            let rb = gitops::rollback(user, &cwd, &sha_before).await;
            return AttemptOutcome::Failed {
                reason: if rb.is_err() {
                    "revert_failed"
                } else {
                    "build_failed"
                }
                .into(),
                error: e,
                exec,
                grave: rb.is_err(),
            };
        }
        let _ = runs.set_phase(run_id, "ship").await;
        if let Err(e) = retry_busy(|| (hooks.ship)(item.scope.clone())).await {
            let rb = gitops::rollback(user, &cwd, &sha_before).await;
            // Ne JAMAIS re-build/re-ship un arbre dont la restauration a
            // échoué (m7) : on livrerait un état corrompu par-dessus la prod.
            if rb.is_ok() {
                let _ = (hooks.build)(item.scope.clone()).await;
                let _ = (hooks.ship)(item.scope.clone()).await;
            }
            return AttemptOutcome::Failed {
                reason: if rb.is_err() {
                    "revert_failed"
                } else {
                    "ship_failed"
                }
                .into(),
                error: e,
                exec,
                grave: rb.is_err(),
            };
        }
        let _ = runs.set_phase(run_id, "healthcheck").await;
        if let Err(e) = (hooks.health)(item.scope.clone()).await {
            let rb = gitops::rollback(user, &cwd, &sha_before).await;
            if rb.is_ok() {
                let _ = (hooks.build)(item.scope.clone()).await;
                let _ = (hooks.ship)(item.scope.clone()).await;
            }
            return AttemptOutcome::Failed {
                reason: if rb.is_err() {
                    "revert_failed"
                } else {
                    "healthcheck_failed"
                }
                .into(),
                error: e,
                exec,
                grave: rb.is_err(),
            };
        }
        let _ = runs.set_phase(run_id, "commit").await;
        let message = format!("auto({}): {} (backlog:{})", item.scope, item.title, item.id);
        match gitops::commit(user, &cwd, &message).await {
            Ok(sha) => {
                info!(item_id = item.id, scope = %item.scope, commit = %sha, "pilot commit");
                AttemptOutcome::Success {
                    commit: Some(sha),
                    exec,
                }
            }
            Err(e) => {
                let rb = gitops::rollback(user, &cwd, &sha_before).await;
                AttemptOutcome::Failed {
                    reason: if rb.is_err() {
                        "revert_failed"
                    } else {
                        "commit_failed"
                    }
                    .into(),
                    error: e.to_string(),
                    exec,
                    grave: rb.is_err(),
                }
            }
        }
    }

    async fn spawn_atelier_worker(
        &self,
        item: &BacklogItem,
        run_id: Uuid,
        attempt: i32,
        retry: Option<&RetryContext>,
        trigger: &str,
    ) -> Result<(), String> {
        let script = std::env::var("ATELIER_PILOT_ATELIER_WORKER")
            .unwrap_or_else(|_| "/opt/atelier/bin/pilot-atelier-worker.sh".into());
        if !std::path::Path::new(&script).is_file() {
            return Err(format!("worker Atelier absent : {script}"));
        }
        let runtime = std::env::var("ATELIER_PILOT_RUNTIME_DIR")
            .unwrap_or_else(|_| "/run/atelier-pilot".into());
        let runtime_path = PathBuf::from(&runtime);
        let owner = &self.inner.config.atelier_user;
        let install = Command::new("install")
            .args(["-d", "-m", "0700", "-o", owner, "-g", owner])
            .arg(&runtime_path)
            .output()
            .await
            .map_err(|e| format!("création runtime Pilote: {e}"))?;
        if !install.status.success() {
            return Err(format!(
                "création runtime Pilote: {}",
                String::from_utf8_lossy(&install.stderr).trim()
            ));
        }

        let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let unit = format!("atelier-pilot-{}", run_id.simple());
        let payload_path = runtime_path.join(format!("{run_id}.json"));
        let prompt = build_item_prompt(item, attempt, retry);
        let payload = json!({
            "run_id": run_id,
            "item_id": item.id,
            "title": item.title,
            "prompt": prompt,
            "secret": secret,
            "oauth_token": (self.hooks().token)().await,
            "api": "http://127.0.0.1:4100/api/pilot/atelier-report",
            "root": self.inner.config.atelier_root,
            "node": std::env::var("ATELIER_AGENT_NODE_BIN").unwrap_or_else(|_| "/usr/bin/node".into()),
            "worker": std::env::var("ATELIER_PILOT_RUNNER").unwrap_or_else(|_| "/opt/atelier/runner/src/worker.js".into()),
            "config_dir": std::env::var("ATELIER_PILOT_CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "/home/romain/.atelier-pilot-claude".into()),
            "model": self.inner.config.model,
            "effort": self.inner.config.effort,
        });
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&payload_path)
            .map_err(|e| format!("payload Atelier: {e}"))?;
        file.write_all(payload.to_string().as_bytes())
            .map_err(|e| format!("payload Atelier: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("payload Atelier sync: {e}"))?;
        let chown = Command::new("chown")
            .arg(format!("{owner}:{owner}"))
            .arg(&payload_path)
            .output()
            .await
            .map_err(|e| format!("chown payload Atelier: {e}"))?;
        if !chown.status.success() {
            let _ = std::fs::remove_file(&payload_path);
            return Err(format!(
                "chown payload Atelier: {}",
                String::from_utf8_lossy(&chown.stderr).trim()
            ));
        }

        let runs = self.inner.runs.as_ref().ok_or("store runs indisponible")?;
        runs.set_phase(run_id, "report")
            .await
            .map_err(|e| e.to_string())?;
        if let Some(schedules) = self.inner.schedule.as_ref() {
            schedules
                .set_secret(Some(&secret), Some(&unit))
                .await
                .map_err(|e| e.to_string())?;
            // m2 : seul un run de NUIT pilote le statut pilot_night ; un run
            // manuel garde le tracking unit+secret (réconciliation post-restart)
            // mais laisse l'état de la nuit intact.
            if trigger == "night"
                && let Ok(snapshot) = schedules.set_waiting_atelier(&json!({"atelier_item_id":item.id,"atelier_run_id":run_id,"atelier_attempt":attempt})).await
            {
                let _ = self.inner.night_tx.send(snapshot);
            }
        }

        let output = Command::new("systemd-run")
            .arg(format!("--unit={unit}"))
            .arg("--collect")
            .arg(format!("--uid={owner}"))
            .arg(format!("--gid={owner}"))
            .arg(format!(
                "--property=WorkingDirectory={}",
                self.inner.config.atelier_root.display()
            ))
            // Garde-fou dur : systemd tue l'unité à 9000 s (2 h 30) — même
            // horizon que la réconciliation, qui settle alors report_lost.
            .arg("--property=RuntimeMaxSec=9000")
            .arg("--")
            .arg(&script)
            .arg(&payload_path)
            .output()
            .await
            .map_err(|e| format!("systemd-run Atelier: {e}"))?;
        if !output.status.success() {
            let _ = std::fs::remove_file(&payload_path);
            if let Some(schedules) = self.inner.schedule.as_ref() {
                let _ = schedules.set_secret(None, None).await;
            }
            return Err(format!(
                "systemd-run Atelier: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        self.inner.detached_atelier.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn snapshot_other_trees(&self, current: &str) -> HashMap<String, (String, String)> {
        let mut states = HashMap::new();
        for slug in (self.hooks().app_slugs)().await {
            if slug == current {
                continue;
            }
            let cwd = self.inner.config.apps_src_root.join(&slug).join("src");
            let Ok(head) = gitops::head_sha(&self.inner.config.app_user, &cwd).await else {
                continue;
            };
            let Ok(status) = gitops::status_porcelain(&self.inner.config.app_user, &cwd).await
            else {
                continue;
            };
            states.insert(slug, (head, status));
        }
        states
    }

    async fn enforce_cross_app_guard(
        &self,
        current: &str,
        before: &HashMap<String, (String, String)>,
    ) -> Result<(), String> {
        let mut changed = Vec::new();
        for (slug, (head, status)) in before {
            let cwd = self.inner.config.apps_src_root.join(slug).join("src");
            let now_head = gitops::head_sha(&self.inner.config.app_user, &cwd)
                .await
                .unwrap_or_default();
            let now_status = gitops::status_porcelain(&self.inner.config.app_user, &cwd)
                .await
                .unwrap_or_default();
            if &now_head == head && &now_status == status {
                continue;
            }
            // A clean, idle tree can be restored deterministically. Never erase
            // concurrent human edits or a pre-existing dirty tree.
            if status.trim().is_empty() && !(self.hooks().live_sessions)(slug) {
                let _ = gitops::rollback(&self.inner.config.app_user, &cwd, head).await;
            }
            changed.push(slug.clone());
        }
        if changed.is_empty() {
            return Ok(());
        }
        let detail = format!(
            "Le worker {current} a touché d’autres workspaces : {}",
            changed.join(", ")
        );
        (self.hooks().notify)(
            Some(current.into()),
            "error".into(),
            "Écriture cross-app bloquée".into(),
            detail.clone(),
        )
        .await;
        Err(detail)
    }

    pub async fn accept_atelier_report(
        &self,
        report: AtelierWorkerReport,
    ) -> Result<BacklogItem, String> {
        let runs = self.inner.runs.as_ref().ok_or("store runs indisponible")?;
        let backlog = self
            .inner
            .backlog
            .as_ref()
            .ok_or("store backlog indisponible")?;
        let run = runs
            .get(report.run_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("run Atelier inconnu")?;
        if run.scope != "atelier" || run.item_id != Some(report.item_id) || run.status != "running"
        {
            return Err("report Atelier ne correspond pas à un run actif".into());
        }
        info!(run_id = %report.run_id, item_id = report.item_id, status = %report.status, trigger = %run.trigger, "pilot atelier report");
        if report.checkpoint_sha.is_some() || report.git_sha_before.is_some() {
            runs.set_git_state(
                report.run_id,
                report.checkpoint_sha.as_deref(),
                report.git_sha_before.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        let needs_user = if report.status == "needs_user" {
            worker_needs_user(report.report.as_deref()).or_else(|| {
                Some((
                    report
                        .error
                        .clone()
                        .unwrap_or_else(|| "Décision utilisateur requise".into()),
                    Vec::new(),
                ))
            })
        } else {
            None
        };
        let success = report.status == "success";
        let item = if let Some((reason, questions)) = needs_user {
            runs.finish_failure(
                report.run_id,
                "attention",
                "needs_user",
                &reason,
                report.report.as_deref(),
                None,
                None,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
            let blocked = backlog
                .settle_needs_user(report.item_id, &reason, &questions, Some(&run.engine))
                .await
                .map_err(|e| e.to_string())?;
            (self.hooks().notify)(
                Some("atelier".into()),
                "warn".into(),
                format!("L’agent a des questions : {}", blocked.title),
                reason,
            )
            .await;
            blocked
        } else if success {
            runs.finish_success(
                report.run_id,
                report.commit_sha.as_deref(),
                report.report.as_deref(),
                None,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
            let done = backlog
                .settle_done(
                    report.item_id,
                    report.commit_sha.as_deref(),
                    Some(&run.engine),
                )
                .await
                .map_err(|e| e.to_string())?;
            (self.hooks().notify)(
                Some("atelier".into()),
                "info".into(),
                format!("Livré : {}", done.title),
                report
                    .commit_sha
                    .clone()
                    .unwrap_or_else(|| "Aucun changement de code nécessaire".into()),
            )
            .await;
            done
        } else {
            let reason = report.failure_reason.as_deref().unwrap_or("deploy_failed");
            let error = report.error.as_deref().unwrap_or("worker Atelier en échec");
            runs.finish_failure(
                report.run_id,
                "failed",
                reason,
                error,
                report.report.as_deref(),
                None,
                None,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
            let blocked = backlog
                .settle_attention(report.item_id, true, error, Some(&run.engine))
                .await
                .map_err(|e| e.to_string())?;
            (self.hooks().notify)(
                Some("atelier".into()),
                "error".into(),
                format!("Item Atelier bloqué : {}", blocked.title),
                error.into(),
            )
            .await;
            blocked
        };
        self.publish("exec", Some(item.clone()), Some(item.id));
        self.inner
            .running
            .lock()
            .expect("pilot running")
            .remove("atelier");
        self.inner.detached_atelier.store(false, Ordering::Relaxed);
        if let Some(schedules) = self.inner.schedule.as_ref() {
            // m2 : seul un run de NUIT clôt pilot_night (status/mark_ran/notif
            // du matin) ; un run manuel ne touche pas à l'état de la nuit.
            if run.trigger == "night" {
                let mut stats = schedules
                    .night()
                    .await
                    .map(|s| s.stats)
                    .unwrap_or_else(|_| json!({}));
                if !stats.is_object() {
                    stats = json!({});
                }
                stats["atelier_item"] = json!(report.item_id);
                stats["atelier_status"] = json!(if success {
                    "done"
                } else if report.status == "needs_user" {
                    "attention"
                } else {
                    "failed"
                });
                if let Ok(snapshot) = schedules
                    .set_night(if success { "done" } else { "failed" }, &stats)
                    .await
                {
                    let _ = self.inner.night_tx.send(snapshot);
                }
                let _ = schedules.mark_ran().await;
                (self.hooks().notify)(
                    None,
                    if success { "info" } else { "error" }.into(),
                    "Rapport Pilote du matin".into(),
                    stats.to_string(),
                )
                .await;
            }
            // Le tracking unit+secret est toujours libéré, même en manuel.
            let _ = schedules.set_secret(None, None).await;
        }
        let runtime = PathBuf::from(
            std::env::var("ATELIER_PILOT_RUNTIME_DIR")
                .unwrap_or_else(|_| "/run/atelier-pilot".into()),
        );
        let _ = std::fs::remove_file(runtime.join(format!("{}.report.json", report.run_id)));
        let _ = std::fs::remove_file(runtime.join(format!("{}.json", report.run_id)));
        Ok(item)
    }

    async fn reconcile_atelier_worker(&self) {
        let (Some(runs), Some(schedules)) = (&self.inner.runs, &self.inner.schedule) else {
            return;
        };
        let Ok(Some(run)) = runs.waiting_atelier().await else {
            return;
        };
        let runtime = std::env::var("ATELIER_PILOT_RUNTIME_DIR")
            .unwrap_or_else(|_| "/run/atelier-pilot".into());
        let marker = PathBuf::from(&runtime).join(format!("{}.report.json", run.id));
        if let Ok(raw) = std::fs::read_to_string(&marker)
            && let Ok(report) = serde_json::from_str::<AtelierWorkerReport>(&raw)
        {
            if let Err(e) = self.accept_atelier_report(report).await {
                warn!(run_id=%run.id,error=%e,"pilot Atelier marker reconciliation failed");
            }
            return;
        }
        let snapshot = schedules
            .night()
            .await
            .unwrap_or_else(|_| NightSnapshot::idle());
        let Some(unit) = snapshot.atelier_unit else {
            return;
        };
        let active = Command::new("systemctl")
            .args(["is-active", "--quiet", &unit])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if active {
            // Au-delà de 2 h 30 le worker est considéré perdu (RuntimeMaxSec le
            // tue à 9000 s de toute façon) : stop, puis on retombe sur le
            // chemin report_lost ci-dessous (rollback + settle blocked).
            let overdue = Utc::now().signed_duration_since(run.started_at)
                > chrono::Duration::minutes(150);
            if !overdue {
                self.inner.detached_atelier.store(true, Ordering::Relaxed);
                return;
            }
            warn!(run_id = %run.id, unit = %unit, "pilot Atelier worker overdue — stopping unit");
            let _ = Command::new("systemctl").args(["stop", &unit]).output().await;
        }
        let item_id = run.item_id.unwrap_or_default();
        if let Ok(Some(commit_sha)) = gitops::find_backlog_commit(
            &self.inner.config.atelier_user,
            &self.inner.config.atelier_root,
            item_id,
        )
        .await
        {
            let report = AtelierWorkerReport {
                run_id: run.id,
                secret: String::new(),
                item_id,
                status: "success".into(),
                commit_sha: Some(commit_sha),
                report: Some(
                    "Commit Atelier retrouvé pendant la réconciliation de démarrage".into(),
                ),
                error: None,
                failure_reason: None,
                checkpoint_sha: run.checkpoint_sha.clone(),
                git_sha_before: run.git_sha_before.clone(),
            };
            if let Err(e) = self.accept_atelier_report(report).await {
                warn!(run_id=%run.id,error=%e,"pilot Atelier recovered-commit reconciliation failed");
            }
            return;
        }
        let payload_path = PathBuf::from(&runtime).join(format!("{}.json", run.id));
        let payload = std::fs::read_to_string(&payload_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
        let checkpoint_sha = payload
            .as_ref()
            .and_then(|v| v.get("checkpoint_sha"))
            .and_then(serde_json::Value::as_str)
            .map(String::from);
        let git_sha_before = payload
            .as_ref()
            .and_then(|v| v.get("git_sha_before"))
            .and_then(serde_json::Value::as_str)
            .map(String::from);
        let (failure_reason, error) = if let Some(before) = git_sha_before.as_deref() {
            match gitops::rollback(
                &self.inner.config.atelier_user,
                &self.inner.config.atelier_root,
                before,
            )
            .await
            {
                Ok(()) => (
                    "report_lost",
                    "L’unité Atelier s’est terminée sans rapport; la source a été restaurée",
                ),
                Err(_) => (
                    "rollback_failed",
                    "L’unité Atelier a perdu son rapport et la source n’a pas pu être restaurée",
                ),
            }
        } else {
            (
                "report_lost",
                "L’unité Atelier s’est terminée sans rapport vérifiable",
            )
        };
        let report = AtelierWorkerReport {
            run_id: run.id,
            secret: String::new(),
            item_id,
            status: "failed".into(),
            commit_sha: None,
            report: None,
            error: Some(error.into()),
            failure_reason: Some(failure_reason.into()),
            checkpoint_sha,
            git_sha_before,
        };
        if let Err(e) = self.accept_atelier_report(report).await {
            warn!(run_id=%run.id,error=%e,"pilot Atelier lost-report reconciliation failed");
        }
    }

    /// Restaure l'arbre des runs app restés `running` après un restart (le
    /// diff d'un run interrompu ne doit pas survivre au boot). Best-effort,
    /// AVANT le fail_stale ; le scope atelier a son chemin dédié
    /// (`reconcile_atelier_worker`).
    async fn restore_orphan_trees(&self) {
        let Some(runs) = self.inner.runs.as_ref() else {
            return;
        };
        let orphans = match runs.running_orphans().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "pilot orphan runs listing failed");
                return;
            }
        };
        for run in orphans {
            if run.scope == "atelier" {
                continue;
            }
            let Some(before) = run.git_sha_before.as_deref() else {
                continue;
            };
            let cwd = self.inner.config.apps_src_root.join(&run.scope).join("src");
            match gitops::rollback(&self.inner.config.app_user, &cwd, before).await {
                Ok(()) => {
                    info!(run_id = %run.id, scope = %run.scope, sha = %before, "pilot boot tree restored")
                }
                Err(e) => {
                    warn!(run_id = %run.id, scope = %run.scope, error = %e, "pilot boot tree restore failed")
                }
            }
        }
    }

    fn push_transcript(&self, run_id: Uuid, scope: &str, line: String) {
        let mut buf = self.inner.transcript.lock().expect("pilot transcript");
        if !buf.rings.contains_key(&run_id) {
            buf.order.push_back(run_id);
            while buf.order.len() > TRANSCRIPT_KEEP_RUNS {
                if let Some(old) = buf.order.pop_front() {
                    buf.rings.remove(&old);
                }
            }
        }
        let ring = buf.rings.entry(run_id).or_default();
        let ev = TranscriptLine {
            run_id,
            scope: scope.into(),
            seq: ring.back().map(|l| l.seq + 1).unwrap_or(0),
            ts: Utc::now().timestamp_millis(),
            line,
        };
        ring.push_back(ev.clone());
        if ring.len() > 500 {
            ring.pop_front();
        }
        let _ = self.inner.transcript_tx.send(ev);
    }

    pub async fn schedule(&self) -> Result<PilotSchedule, String> {
        self.inner
            .schedule
            .as_ref()
            .ok_or("Pilote indisponible")?
            .get()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn night(&self) -> Result<NightSnapshot, String> {
        self.inner
            .schedule
            .as_ref()
            .ok_or("Pilote indisponible")?
            .night()
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn start_night(&self, trigger: &str) -> Result<NightSnapshot, String> {
        if self
            .inner
            .night_cancel
            .lock()
            .expect("pilot night")
            .is_some()
        {
            return Err("une nuit Pilote est déjà en cours".into());
        }
        if !self.inner.running.lock().expect("pilot running").is_empty()
            || self.inner.detached_atelier.load(Ordering::Relaxed)
        {
            return Err("un run Pilote est déjà actif".into());
        }
        if (self.hooks().platform_busy)() {
            return Err("backup ou surveillance en cours".into());
        }
        let cancel = CancellationToken::new();
        *self.inner.night_cancel.lock().expect("pilot night") = Some(cancel.clone());
        let snap = self
            .inner
            .schedule
            .as_ref()
            .ok_or("Pilote indisponible")?
            .set_night("running", &json!({"trigger":trigger,"done":0,"failed":0}))
            .await
            .map_err(|e| e.to_string())?;
        info!(trigger = %trigger, "pilot night start");
        let _ = self.inner.night_tx.send(snap.clone());
        let svc = self.clone();
        tokio::spawn(async move {
            svc.run_night(cancel).await;
        });
        Ok(snap)
    }

    pub fn cancel_night(&self) -> bool {
        let cancelled = if let Some(c) = self
            .inner
            .night_cancel
            .lock()
            .expect("pilot night")
            .as_ref()
        {
            c.cancel();
            true
        } else {
            false
        };
        if cancelled {
            for work in self.inner.running.lock().expect("pilot running").values() {
                if work.trigger == "night" {
                    work.cancel.cancel();
                }
            }
        }
        cancelled
    }

    #[instrument(skip(self, cancel))]
    async fn run_night(&self, cancel: CancellationToken) {
        let Some(schedule_store) = self.inner.schedule.as_ref() else {
            return;
        };
        let cfg = match schedule_store.get().await {
            Ok(v) => v,
            Err(e) => {
                error!(error=%e,"pilot night schedule");
                return;
            }
        };
        let hooks = self.hooks();
        if cfg.resolve_findings {
            for f in (hooks.findings)().await {
                let _=self.inner.backlog.as_ref().unwrap().insert(NewBacklogItem{scope:f.slug,title:format!("Résoudre finding #{} — {}",f.id,f.title),request:format!("Résous ce finding de surveillance sans approbation si le correctif est sûr.\n\nKind: {}\nRésumé: {}\n\nPlan:\n{}",f.kind,f.summary,f.plan),description:String::new(),plan:Some(f.plan),kind:"finding_fix".into(),priority:severity_priority(&f.severity).into(),severity:f.severity,effort:"m".into(),lane:"ready".into(),engine:"auto".into(),needs_user:false,needs_user_reason:None,questions:vec![],finding_id:Some(f.id),created_by:"scan".into()}).await;
            }
        }
        let items = self
            .inner
            .backlog
            .as_ref()
            .unwrap()
            .ready_items(cfg.include_atelier)
            .await
            .unwrap_or_default();
        let planned_ids = Arc::new(items.iter().map(|i| i.id).collect::<Vec<_>>());
        self.publish_night_progress(&planned_ids).await;
        let mut groups: HashMap<String, Vec<BacklogItem>> = HashMap::new();
        let mut atelier = None;
        for item in items {
            if item.scope == "atelier" {
                if atelier.is_none() {
                    atelier = Some(item)
                }
            } else {
                groups.entry(item.scope.clone()).or_default().push(item);
            }
        }
        let sem = Arc::new(Semaphore::new(cfg.max_concurrent as usize));
        // Auth systémique (sdk_auth_failed) : tout run échouerait pareil — la
        // nuit s'arrête et se clôt `failed` (≠ simple cancel utilisateur).
        let auth_abort = Arc::new(AtomicBool::new(false));
        // Scopes exclus après échec grave (revert_failed / cross-app) : leurs
        // items restants restent ready, mention au rapport du matin.
        let excluded_scopes: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut joins = Vec::new();
        for (scope, items) in groups {
            let svc = self.clone();
            let sem = sem.clone();
            let cancel = cancel.clone();
            let planned = planned_ids.clone();
            let auth_abort = auth_abort.clone();
            let excluded_scopes = excluded_scopes.clone();
            joins.push(tokio::spawn(async move {
                let _p = sem.acquire().await.ok();
                for item in items {
                    if cancel.is_cancelled() {
                        break;
                    }
                    while (svc.hooks().platform_busy)() {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                        }
                    }
                    if cancel.is_cancelled() {
                        break;
                    }
                    if svc.run_item(item.id, "night").await.is_ok() {
                        svc.wait_item(item.id, &cancel).await;
                        svc.publish_night_progress(&planned).await;
                        let failure = svc.item_last_failure(item.id).await;
                        if failure.as_deref() == Some("sdk_auth_failed") {
                            // Abort global : on annule aussi TOUS les runs en
                            // vol des autres scopes (ils échoueraient pareil).
                            auth_abort.store(true, Ordering::Relaxed);
                            cancel.cancel();
                            svc.cancel_all_running();
                            break;
                        }
                        if matches!(
                            failure.as_deref(),
                            Some("revert_failed") | Some("cross_app_write")
                        ) {
                            // Arbre potentiellement corrompu : on n'enchaîne
                            // pas les items restants de ce scope cette nuit.
                            warn!(scope = %scope, "pilot night: scope excluded after grave failure");
                            excluded_scopes
                                .lock()
                                .expect("pilot excluded scopes")
                                .push(scope.clone());
                            break;
                        }
                    }
                }
            }));
        }
        for j in joins {
            let _ = j.await;
        }
        if !cancel.is_cancelled() {
            if let Some(item) = atelier {
                let _ = self.run_item(item.id, "night").await;
                self.wait_item(item.id, &cancel).await;
            }
        }
        let all = self
            .inner
            .backlog
            .as_ref()
            .unwrap()
            .list(None, None)
            .await
            .unwrap_or_default();
        let selected = all
            .iter()
            .filter(|i| planned_ids.contains(&i.id))
            .collect::<Vec<_>>();
        let mut engines: HashMap<String, EngineNightStats> = HashMap::new();
        let mut attempts = 0_u64;
        for id in planned_ids.iter() {
            if let Ok(item_runs) = self.inner.runs.as_ref().unwrap().list_for_item(*id).await {
                for run in item_runs.into_iter().filter(|r| {
                    r.trigger == "night"
                        && r.started_at
                            >= cfg
                                .last_run_at
                                .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24))
                }) {
                    attempts += 1;
                    let entry = engines.entry(run.engine).or_default();
                    entry.runs += 1;
                    if run.status == "success" {
                        entry.success += 1;
                    } else {
                        entry.failed += 1;
                    }
                    entry.tokens_in += run.tokens_in.unwrap_or(0);
                    entry.tokens_out += run.tokens_out.unwrap_or(0);
                }
            }
        }
        let done = selected.iter().filter(|i| i.lane == "done").count();
        let attention = selected.iter().filter(|i| i.lane == "attention").count();
        let blocked = selected
            .iter()
            .filter(|i| i.exec_status == "blocked")
            .count();
        let findings = selected
            .iter()
            .filter(|i| i.kind == "finding_fix" && i.lane == "done")
            .count();
        let excluded = excluded_scopes
            .lock()
            .expect("pilot excluded scopes")
            .clone();
        let auth_aborted = auth_abort.load(Ordering::Relaxed);
        let mut stats = json!({"total":planned_ids.len(),"done":done,"attention":attention,"blocked":blocked,"findings":findings,"attempts":attempts,"engines":engines});
        if auth_aborted {
            stats["note"] = json!("auth expirée");
        }
        if !excluded.is_empty() {
            stats["excluded_scopes"] = json!(excluded);
        }
        let status = if auth_aborted {
            "failed"
        } else if cancel.is_cancelled() {
            "cancelled"
        } else {
            "done"
        };
        if let Ok(s) = schedule_store.set_night(status, &stats).await {
            let _ = self.inner.night_tx.send(s);
        }
        let _ = schedule_store.mark_ran().await;
        info!(status = %status, done, attention, blocked, findings, attempts, "pilot night end");
        let mut report = format!(
            "{done} livré(s) · {attention} en attention · {blocked} bloqué(s) · {findings} finding(s) résolu(s) · {attempts} tentative(s)"
        );
        if !excluded.is_empty() {
            report.push_str(&format!(
                " · scope(s) exclu(s) après échec grave : {}",
                excluded.join(", ")
            ));
        }
        if auth_aborted {
            report.push_str(" · nuit interrompue : authentification Claude expirée");
        }
        (hooks.notify)(
            None,
            if blocked > 0 {
                "error"
            } else if attention > 0 {
                "warn"
            } else {
                "info"
            }
            .into(),
            "Rapport Pilote du matin".into(),
            report,
        )
        .await;
        *self.inner.night_cancel.lock().expect("pilot night") = None;
    }

    async fn wait_item(&self, id: i64, cancel: &CancellationToken) {
        // Pire cas réel : 3 tentatives au timeout max chacune + marge
        // build/ship/health (m12) — un cap fixe sous-dimensionné larguait des
        // items encore en vol quand le timeout par run est long.
        let cap_secs = self.inner.config.timeout.as_secs().saturating_mul(3) + 30 * 60;
        for _ in 0..cap_secs {
            if cancel.is_cancelled() {
                break;
            }
            let active = self
                .inner
                .backlog
                .as_ref()
                .unwrap()
                .get(id)
                .await
                .ok()
                .flatten()
                .map(|i| matches!(i.exec_status.as_str(), "queued" | "running"))
                .unwrap_or(false);
            if !active {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn publish_night_progress(&self, ids: &[i64]) {
        let Some(schedule) = self.inner.schedule.as_ref() else {
            return;
        };
        let all = self
            .inner
            .backlog
            .as_ref()
            .unwrap()
            .list(None, None)
            .await
            .unwrap_or_default();
        let selected = all
            .iter()
            .filter(|i| ids.contains(&i.id))
            .collect::<Vec<_>>();
        let queue = ids
            .iter()
            .filter_map(|id| selected.iter().find(|item| item.id == *id))
            .map(|item| {
                json!({
                    "id": item.id,
                    "scope": item.scope,
                    "title": item.title,
                    "status": item.exec_status,
                    "attempt": item.attempts,
                })
            })
            .collect::<Vec<_>>();
        let current = selected
            .iter()
            .find(|i| matches!(i.exec_status.as_str(), "queued" | "running"))
            .map(|i| json!({"id":i.id,"scope":i.scope,"title":i.title,"attempt":i.attempts}));
        let stats = json!({
            "total": ids.len(),
            "done": selected.iter().filter(|i|i.lane=="done").count(),
            "running": selected.iter().filter(|i|matches!(i.exec_status.as_str(),"queued"|"running")).count(),
            "attention": selected.iter().filter(|i|i.lane=="attention").count(),
            "blocked": selected.iter().filter(|i|i.exec_status=="blocked").count(),
            "queue": queue,
            "current": current,
        });
        if let Ok(snapshot) = schedule.set_night("running", &stats).await {
            let _ = self.inner.night_tx.send(snapshot);
        }
    }

    /// `failure_reason` du run le plus récent de l'item (None si aucun run ou
    /// dernier run réussi) — sert au pilotage de la nuit (abort auth, scopes
    /// exclus après échec grave).
    async fn item_last_failure(&self, item_id: i64) -> Option<String> {
        let runs = self.inner.runs.as_ref()?;
        runs.list_for_item(item_id)
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
            .and_then(|r| r.failure_reason)
    }

    /// Annule tous les runs en vol, quel que soit leur trigger (abort de nuit
    /// pour cause systémique : ils échoueraient tous pareil).
    fn cancel_all_running(&self) {
        for work in self.inner.running.lock().expect("pilot running").values() {
            work.cancel.cancel();
        }
    }

    async fn scheduler_loop(&self) {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            self.reconcile_atelier_worker().await;
            let Some(s) = self.inner.schedule.as_ref() else {
                return;
            };
            match s.get().await {
                Ok(cfg) if due(&cfg) && !self.is_busy() && !(self.hooks().platform_busy)() => {
                    let _ = self.start_night("night").await;
                }
                Ok(_) => {}
                Err(e) => warn!(error=%e,"pilot scheduler tick"),
            }
        }
    }
}

enum AttemptOutcome {
    Detached,
    Success {
        commit: Option<String>,
        exec: WorkerExec,
    },
    NeedsUser {
        reason: String,
        questions: Vec<Question>,
        exec: WorkerExec,
    },
    Deferred(String),
    Failed {
        reason: String,
        error: String,
        exec: WorkerExec,
        grave: bool,
    },
}
fn failed(reason: &str, error: String, grave: bool) -> AttemptOutcome {
    AttemptOutcome::Failed {
        reason: reason.into(),
        error,
        exec: WorkerExec::default(),
        grave,
    }
}

async fn retry_busy<F, Fut>(mut f: F) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    for n in 0..3 {
        match f().await {
            Ok(()) => return Ok(()),
            Err(e) if e.contains("BUILD_BUSY") && n < 2 => {
                tokio::time::sleep(Duration::from_secs(60)).await
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Message du settle blocked : nombre RÉEL de tentatives + raison (m9) —
/// jamais « 3 tentatives » en dur.
fn blocked_reason(attempts: i32, failure: &str, error: &str) -> String {
    let detail = match (failure.is_empty(), error.is_empty()) {
        (false, false) => format!("{failure} : {error}"),
        (false, true) => failure.to_string(),
        (true, false) => error.to_string(),
        (true, true) => "échec inconnu".to_string(),
    };
    format!("Bloqué après {attempts} tentative(s) — {detail}")
}

/// Fin de transcript embarquée dans le prompt de retry : ~40 dernières lignes,
/// bornée à 4 Ko (on garde la FIN, l'échec y est).
fn exec_tail(lines: &[String]) -> String {
    let start = lines.len().saturating_sub(40);
    let mut tail = lines[start..].join("\n");
    if tail.len() > 4096 {
        let mut cut = tail.len() - 4096;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
    tail
}

fn build_item_prompt(item: &BacklogItem, attempt: i32, retry: Option<&RetryContext>) -> String {
    let answers = item
        .questions
        .iter()
        .filter_map(|q| q.answer.as_ref().map(|a| format!("- {} → {}", q.q, a)))
        .collect::<Vec<_>>()
        .join("\n");
    // Retry enrichi : le contexte d'échec (phase + erreur + fin de transcript)
    // est souvent ce qui débloque la tentative suivante.
    let previous = match retry {
        None => "(aucun)".to_string(),
        Some(r) => {
            let mut s = format!("phase `{}` — {}", r.phase, r.error);
            if !r.transcript_tail.trim().is_empty() {
                s.push_str(&format!(
                    "\nFin de transcript de la tentative précédente :\n{}",
                    r.transcript_tail
                ));
            }
            s
        }
    };
    format!(
        r#"[RUN AUTONOME — ATELIER PILOTE]
Tu exécutes un item borné du backlog de {scope}. Respecte CLAUDE.md, les rules et skills du projet.

ITEM #{id} — {title}
Demande verbatim :
{request}

Description :
{description}

Plan attaché :
{plan}

Tentative {attempt}/3. Échec précédent : {previous}
Questions/réponses précédentes :
{answers}

Contrat : investigue puis implémente strictement ce périmètre. Tu peux modifier uniquement ce workspace. Ne commite jamais, ne pousse jamais et ne redémarre aucun service : l'orchestrateur build/ship/healthcheck/commit après toi. N'utilise ni sudo ni systemctl. Les suppressions de données en masse sont interdites.
Si une décision produit, une ambiguïté, un plan caduc ou un risque empêche un changement sûr, appelle backlog_update avec id={id}, needs_user=true, reason et questions autosuffisantes, puis ARRÊTE sans modifier davantage. Sinon valide localement autant que possible.

Ta réponse finale doit se terminer par un bloc JSON valide de cette forme (sans texte après) :
```json
{{"pilot":{{"outcome":"done|needs_user","summary":"résumé","reason":"raison si besoin","questions":["question autosuffisante"]}}}}
```
Utilise outcome=needs_user pour tout doute; Atelier appliquera ce rapport même si ton moteur n'a pas accès au MCP."#,
        scope = item.scope,
        id = item.id,
        title = item.title,
        request = item.request,
        description = item.description,
        plan = item.plan.as_deref().unwrap_or("(aucun)"),
        attempt = attempt,
        previous = previous,
        answers = if answers.is_empty() {
            "(aucune)"
        } else {
            &answers
        }
    )
}

fn severity_priority(s: &str) -> &'static str {
    match s {
        "critical" => "critical",
        "high" => "high",
        "low" => "low",
        _ => "medium",
    }
}

fn worker_needs_user(report: Option<&str>) -> Option<(String, Vec<Question>)> {
    let text = report?;
    let json_text = text
        .rsplit_once("```json")
        .and_then(|(_, tail)| tail.split_once("```").map(|(body, _)| body.trim()))
        .or_else(|| {
            text.rsplit_once("```")
                .and_then(|(head, _)| head.rsplit_once("```json").map(|(_, body)| body.trim()))
        })?;
    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;
    let pilot = value.get("pilot")?;
    if pilot.get("outcome").and_then(serde_json::Value::as_str) != Some("needs_user") {
        return None;
    }
    let reason = pilot
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Décision requise")
        .to_string();
    let questions = pilot
        .get("questions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            q.as_str().map(|s| Question {
                q: s.to_string(),
                answer: None,
            })
        })
        .collect::<Vec<_>>();
    Some((reason, questions))
}

#[cfg(test)]
mod tests {
    use super::{blocked_reason, exec_tail, worker_needs_user};

    #[test]
    fn blocked_message_uses_real_attempts_and_reason() {
        assert_eq!(
            blocked_reason(1, "build_failed", "cargo: erreur E0308"),
            "Bloqué après 1 tentative(s) — build_failed : cargo: erreur E0308"
        );
        assert_eq!(
            blocked_reason(2, "", "boom"),
            "Bloqué après 2 tentative(s) — boom"
        );
        assert_eq!(
            blocked_reason(3, "timeout", ""),
            "Bloqué après 3 tentative(s) — timeout"
        );
        assert_eq!(
            blocked_reason(1, "", ""),
            "Bloqué après 1 tentative(s) — échec inconnu"
        );
    }

    #[test]
    fn exec_tail_keeps_the_end() {
        let lines: Vec<String> = (0..100).map(|i| format!("l{i}")).collect();
        let tail = exec_tail(&lines);
        assert!(tail.starts_with("l60"));
        assert!(tail.ends_with("l99"));
        let big = vec!["x".repeat(10_000)];
        assert_eq!(exec_tail(&big).len(), 4096);
        assert!(exec_tail(&[]).is_empty());
    }

    #[test]
    fn terminal_report_extracts_user_questions() {
        let report = r#"Résumé.
```json
{"pilot":{"outcome":"needs_user","reason":"Choix produit","questions":["Quelle option ?"]}}
```"#;
        let (reason, questions) = worker_needs_user(Some(report)).unwrap();
        assert_eq!(reason, "Choix produit");
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].q, "Quelle option ?");
        assert!(
            worker_needs_user(Some(
                r#"```json
{"pilot":{"outcome":"done","summary":"ok"}}
```"#
            ))
            .is_none()
        );
        assert!(worker_needs_user(Some("pas de rapport structuré")).is_none());
    }
}
