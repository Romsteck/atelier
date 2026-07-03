use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::migration::{self, DEFAULT_DB_NAME};
use crate::models::{BackupEvent, EventStatus, Phase, PhaseDetail, RepoStats, SnapshotResult, ToolStatus};
use crate::rclone;
use crate::restic::{self, RunOutcome};
use crate::runs::{BackupRun, RunsStore};
use crate::sources::SourcePaths;
use crate::sqlx::{Pool, Postgres};
use crate::target::{BackupTarget, NewTarget, TargetStore};

/// Statut agrégé pour l'UI (`GET /api/backup/status`).
#[derive(Debug, Clone, Serialize)]
pub struct BackupStatus {
    pub running: bool,
    pub current_run_id: Option<Uuid>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_run: Option<BackupRun>,
    pub target_configured: bool,
    pub tools: ToolStatus,
    pub schedule_enabled: bool,
    pub repo_stats: Option<RepoStats>,
}

pub struct BackupServiceConfig {
    pub admin_dsn: Option<String>,
    pub db_name: Option<String>,
    pub sources: SourcePaths,
    pub restic_bin: String,
    pub rclone_bin: String,
    pub pg_dumpall_bin: String,
    pub pg_run_user: String,
}

#[derive(Clone)]
pub struct BackupService {
    inner: Arc<Inner>,
}

struct Inner {
    target: Option<TargetStore>,
    runs: Option<RunsStore>,
    tx: broadcast::Sender<BackupEvent>,
    /// Run unique en cours (single-flight) : id + canal d'annulation.
    running: Mutex<Option<(Uuid, oneshot::Sender<()>)>>,
    sources: SourcePaths,
    restic_bin: String,
    rclone_bin: String,
    pg_dumpall_bin: String,
    pg_run_user: String,
    /// Cache des stats du dépôt (appel restic/SMB potentiellement lent).
    repo_stats: Mutex<Option<RepoStats>>,
    enabled: bool,
}

// Pondération de progression par phase (start, end).
const R_REPO: (u8, u8) = (0, 2);
const R_GIT: (u8, u8) = (2, 35);
const R_POSTGRES: (u8, u8) = (35, 70);
const R_CONFIG: (u8, u8) = (70, 80);
const R_PRUNE: (u8, u8) = (80, 98);

fn map_progress(range: (u8, u8), bytes_done: u64, total: Option<u64>) -> u8 {
    let (a, b) = range;
    match total {
        Some(t) if t > 0 => {
            let frac = (bytes_done as f64 / t as f64).clamp(0.0, 1.0);
            a + ((b - a) as f64 * frac) as u8
        }
        _ => a,
    }
}

fn gen_password() -> String {
    let a = Uuid::new_v4().simple().to_string();
    let b = Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

impl BackupService {
    pub async fn start(cfg: BackupServiceConfig) -> Self {
        let pool = match bootstrap(&cfg).await {
            Ok(p) => Some(p),
            Err(err) => {
                warn!(?err, "atelier-backup: bootstrap failed — running in noop mode");
                None
            }
        };
        let enabled = pool.is_some();
        let (target, runs) = match pool.as_ref() {
            Some(p) => (Some(TargetStore::new(p.clone())), Some(RunsStore::new(p.clone()))),
            None => (None, None),
        };
        let (tx, _rx) = broadcast::channel::<BackupEvent>(256);

        let svc = Self {
            inner: Arc::new(Inner {
                target,
                runs,
                tx,
                running: Mutex::new(None),
                sources: cfg.sources,
                restic_bin: cfg.restic_bin,
                rclone_bin: cfg.rclone_bin,
                pg_dumpall_bin: cfg.pg_dumpall_bin,
                pg_run_user: cfg.pg_run_user,
                repo_stats: Mutex::new(None),
                enabled,
            }),
        };

        if enabled {
            if let Some(runs) = svc.inner.runs.as_ref() {
                match runs.sweep_running().await {
                    Ok(n) if n > 0 => warn!(swept = n, "atelier-backup: runs 'running' orphelins → failed"),
                    _ => {}
                }
            }
            // Scheduler présent mais inactif tant que schedule_enabled=false.
            tokio::spawn(crate::scheduler::run_loop(svc.clone()));
            info!("atelier-backup: started (restic+rclone, manual runs; scheduler idle)");
        }
        svc
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BackupEvent> {
        self.inner.tx.subscribe()
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    pub fn is_running(&self) -> bool {
        self.inner.running.lock().unwrap().is_some()
    }

    fn current_run_id(&self) -> Option<Uuid> {
        self.inner.running.lock().unwrap().as_ref().map(|(id, _)| *id)
    }

    pub async fn tools(&self) -> ToolStatus {
        let (restic, rclone) = tokio::join!(
            restic::binary_present(&self.inner.restic_bin, "version"),
            restic::binary_present(&self.inner.rclone_bin, "version"),
        );
        ToolStatus { restic, rclone }
    }

    // ---- cible ----

    pub async fn target(&self) -> Result<Option<BackupTarget>, String> {
        let store = self.inner.target.as_ref().ok_or_else(noop)?;
        store.get_redacted().await.map_err(|e| e.to_string())
    }

    pub async fn set_target(&self, t: &NewTarget) -> Result<(), String> {
        t.validate()?;
        let store = self.inner.target.as_ref().ok_or_else(noop)?;
        store.upsert(t).await.map_err(|e| e.to_string())
    }

    pub async fn reveal_restic_password(&self) -> Result<Option<String>, String> {
        let store = self.inner.target.as_ref().ok_or_else(noop)?;
        store.reveal_restic_password().await.map_err(|e| e.to_string())
    }

    /// Découverte : liste les partages exposés par un serveur SMB à partir
    /// d'identifiants fournis (rien n'est persisté). Si `password` est vide et
    /// qu'un mot de passe est déjà stocké pour cet hôte, on réutilise le stocké.
    pub async fn discover_shares(
        &self,
        host: &str,
        username: &str,
        password: &str,
        domain: &str,
    ) -> Result<Vec<String>, String> {
        if host.trim().is_empty() {
            return Err("hôte requis".into());
        }
        // Repli sur le mot de passe stocké si le champ est laissé vide (édition
        // d'une cible déjà configurée sur le même hôte).
        let pw = if !password.is_empty() {
            password.to_string()
        } else if let Some(store) = self.inner.target.as_ref() {
            match store.get_full().await {
                Ok(Some(t)) if t.host.trim() == host.trim() => t.password.unwrap_or_default(),
                _ => String::new(),
            }
        } else {
            String::new()
        };
        let vars = rclone::smb_vars(&self.inner.rclone_bin, host, username, &pw, domain).await?;
        rclone::list_shares(&self.inner.rclone_bin, &vars).await
    }

    /// Teste la connectivité SMB + l'état du dépôt (sans rien écrire).
    pub async fn test_target(&self) -> Result<TestReport, String> {
        let store = self.inner.target.as_ref().ok_or_else(noop)?;
        let mut t = store.get_full().await.map_err(|e| e.to_string())?.ok_or("cible non configurée")?;
        if !t.is_configured() {
            return Err("cible non configurée (host/share/username requis)".into());
        }
        // Un test n'a pas besoin du dépôt initialisé : on injecte un mdp dépôt
        // factice si absent, juste pour bâtir l'env rclone et lister le partage.
        if t.restic_password.is_none() {
            t.restic_password = Some("test-only".into());
        }
        let env = rclone::build_env(&self.inner.rclone_bin, &t).await?;
        rclone::test_share(&self.inner.rclone_bin, &env, &t).await?;
        let repo_ok = restic::repo_exists(&self.inner.restic_bin, &env)
            .await
            .unwrap_or(false);
        Ok(TestReport { shares_ok: true, repo_ok })
    }

    // ---- runs ----

    pub async fn list_runs(&self, limit: i64, offset: i64) -> Result<(Vec<BackupRun>, i64), String> {
        let runs = self.inner.runs.as_ref().ok_or_else(noop)?;
        let items = runs.list(limit, offset).await.map_err(|e| e.to_string())?;
        let total = runs.count().await.map_err(|e| e.to_string())?;
        Ok((items, total))
    }

    pub async fn get_run(&self, id: Uuid) -> Result<Option<BackupRun>, String> {
        let runs = self.inner.runs.as_ref().ok_or_else(noop)?;
        runs.get(id).await.map_err(|e| e.to_string())
    }

    pub async fn last_success_at(&self) -> Result<Option<DateTime<Utc>>, String> {
        let runs = self.inner.runs.as_ref().ok_or_else(noop)?;
        runs.last_success_at().await.map_err(|e| e.to_string())
    }

    pub async fn status(&self) -> Result<BackupStatus, String> {
        let runs = self.inner.runs.as_ref().ok_or_else(noop)?;
        let target = self.inner.target.as_ref().ok_or_else(noop)?;
        let last_success_at = runs.last_success_at().await.map_err(|e| e.to_string())?;
        let last_run = runs.list(1, 0).await.map_err(|e| e.to_string())?.into_iter().next();
        let full = target.get_full().await.map_err(|e| e.to_string())?;
        let target_configured = full.as_ref().map(|t| t.is_configured()).unwrap_or(false);
        let schedule_enabled = full.as_ref().map(|t| t.schedule_enabled).unwrap_or(false);
        let repo_stats = self.inner.repo_stats.lock().unwrap().clone();
        Ok(BackupStatus {
            running: self.is_running(),
            current_run_id: self.current_run_id(),
            last_success_at,
            last_run,
            target_configured,
            tools: self.tools().await,
            schedule_enabled,
            repo_stats,
        })
    }

    /// Lance un backup (fire-and-forget). 409 si déjà en cours, 400 si cible/outil
    /// manquant.
    pub async fn run_now(&self, trigger: &str) -> Result<Uuid, String> {
        let store = self.inner.target.as_ref().ok_or_else(noop)?;
        let runs = self.inner.runs.as_ref().ok_or_else(noop)?;
        let full = store.get_full().await.map_err(|e| e.to_string())?;
        match &full {
            Some(t) if t.is_configured() => {}
            _ => return Err("cible de sauvegarde non configurée".into()),
        }
        let tools = self.tools().await;
        if !tools.restic || !tools.rclone {
            return Err("binaire manquant (restic/rclone) — installer les paquets".into());
        }

        // Réservation atomique du créneau single-flight.
        let run_id = Uuid::new_v4();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        {
            let mut g = self.inner.running.lock().unwrap();
            if g.is_some() {
                return Err("une sauvegarde est déjà en cours".into());
            }
            *g = Some((run_id, cancel_tx));
        }
        if let Err(e) = runs.start(run_id, trigger).await {
            self.inner.running.lock().unwrap().take();
            return Err(format!("création du run échouée: {e}"));
        }
        let svc = self.clone();
        tokio::spawn(async move {
            svc.execute(run_id, cancel_rx).await;
        });
        Ok(run_id)
    }

    /// Annule le run in-flight. true si trouvé.
    pub fn cancel_run(&self, run_id: Uuid) -> bool {
        let mut g = self.inner.running.lock().unwrap();
        match g.as_ref() {
            Some((id, _)) if *id == run_id => {
                if let Some((_, tx)) = g.take() {
                    let _ = tx.send(());
                }
                true
            }
            _ => false,
        }
    }

    fn emit(
        &self,
        run_id: Uuid,
        phase: Phase,
        status: EventStatus,
        message: &str,
        progress: u8,
        detail: Option<PhaseDetail>,
    ) {
        let _ = self.inner.tx.send(BackupEvent {
            run_id,
            phase,
            status,
            message: message.to_string(),
            progress,
            detail,
            at: Utc::now(),
        });
    }

    /// Mirror disque du mot de passe restic, à côté des secrets dataverse
    /// (root-only). Volontairement HORS des sources sauvegardées : le chiffrer
    /// avec lui-même n'aurait aucun intérêt — c'est une copie de survie locale.
    fn password_mirror_path(&self) -> Option<std::path::PathBuf> {
        self.inner
            .sources
            .dv_secrets
            .parent()
            .map(|p| p.join("restic-repo-password"))
    }

    async fn read_password_mirror(&self) -> Option<String> {
        let path = self.password_mirror_path()?;
        let content = tokio::fs::read_to_string(&path).await.ok()?;
        let pw = content.trim().to_string();
        if pw.is_empty() {
            return None;
        }
        warn!(path = %path.display(), "restic password absent de Postgres — repris depuis le mirror disque");
        Some(pw)
    }

    async fn write_password_mirror(&self, pw: &str) {
        if pw.is_empty() {
            return;
        }
        let Some(path) = self.password_mirror_path() else {
            return;
        };
        if matches!(tokio::fs::read_to_string(&path).await, Ok(existing) if existing.trim() == pw) {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        match tokio::fs::write(&path, format!("{pw}\n")).await {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
                info!(path = %path.display(), "mirror disque du mot de passe restic écrit");
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "échec écriture du mirror du mot de passe restic");
            }
        }
    }

    async fn execute(&self, run_id: Uuid, mut cancel_rx: oneshot::Receiver<()>) {
        self.execute_inner(run_id, &mut cancel_rx).await;
        self.inner.running.lock().unwrap().take();
        // Rafraîchit le cache des stats du dépôt après chaque run.
        self.refresh_repo_stats().await;
    }

    async fn execute_inner(&self, run_id: Uuid, cancel_rx: &mut oneshot::Receiver<()>) {
        let store = self.inner.target.as_ref().unwrap();
        let runs = self.inner.runs.as_ref().unwrap();

        // Cible + mot de passe du dépôt (généré au 1ᵉʳ run).
        let mut t = match store.get_full().await {
            Ok(Some(t)) if t.is_configured() => t,
            _ => return self.fail(run_id, "cible non configurée").await,
        };
        if t.restic_password.as_deref().unwrap_or("").is_empty() {
            // Ligne PG vide mais mirror disque présent = base atelier_meta
            // perdue/recréée : reprendre le mot de passe existant, sinon un
            // nouveau rendrait tous les snapshots existants illisibles.
            let pw = match self.read_password_mirror().await {
                Some(pw) => pw,
                None => gen_password(),
            };
            if let Err(e) = store.set_restic_password(&pw).await {
                return self.fail(run_id, &format!("init mot de passe dépôt: {e}")).await;
            }
            t.restic_password = Some(pw);
        }
        // Mirror disque (0600) : la base qu'on sauvegarde est précisément ce que
        // ce mot de passe protège — sans copie hors-Postgres, perdre atelier_meta
        // rendait le dépôt restic définitivement illisible (dépendance circulaire).
        self.write_password_mirror(t.restic_password.as_deref().unwrap_or("")).await;
        let env = match rclone::build_env(&self.inner.rclone_bin, &t).await {
            Ok(e) => e,
            Err(e) => return self.fail(run_id, &e).await,
        };

        // Phase repo : init si absent.
        self.emit(run_id, Phase::Repo, EventStatus::Running, "Vérification du dépôt…", R_REPO.0, None);
        match restic::repo_exists(&self.inner.restic_bin, &env).await {
            Ok(true) => {}
            Ok(false) => {
                self.emit(run_id, Phase::Repo, EventStatus::Running, "Initialisation du dépôt…", R_REPO.1, None);
                if let Err(e) = restic::init(&self.inner.restic_bin, &env).await {
                    return self.fail(run_id, &format!("init dépôt: {e}")).await;
                }
            }
            Err(e) => return self.fail(run_id, &format!("accès dépôt: {e}")).await,
        }

        let mut total_processed = 0i64;

        // --- GIT ---
        let _ = runs.set_phase(run_id, "git").await;
        self.emit(run_id, Phase::Git, EventStatus::Running, "Archivage du dépôt git…", R_GIT.0, None);
        let git_paths = vec![self.inner.sources.git_dir.clone()];
        let mut git_throttle = None;
        let outcome = restic::backup_paths(
            &self.inner.restic_bin,
            &env,
            "git",
            &git_paths,
            cancel_rx,
            |bd, total| {
                if throttle(&mut git_throttle) {
                    return;
                }
                self.emit(
                    run_id,
                    Phase::Git,
                    EventStatus::Running,
                    "Archivage du dépôt git…",
                    map_progress(R_GIT, bd, total),
                    Some(PhaseDetail { bytes_done: Some(bd), bytes_total: total, tag: Some("git".into()) }),
                );
            },
        )
        .await;
        let git_added = match self.finish_phase(run_id, "git", outcome, R_GIT.1).await {
            PhaseFlow::Continue(res) => {
                total_processed += res.bytes_processed;
                res.bytes_added
            }
            PhaseFlow::Stop => return,
        };

        // --- POSTGRES (streamé) ---
        let _ = runs.set_phase(run_id, "postgres").await;
        self.emit(run_id, Phase::Postgres, EventStatus::Running, "Dump PostgreSQL (pg_dumpall)…", R_POSTGRES.0, None);
        let script = crate::pgdump::pipeline_script(
            &self.inner.pg_dumpall_bin,
            &self.inner.restic_bin,
            &self.inner.pg_run_user,
        );
        let mut pg_throttle = None;
        let outcome = restic::backup_stdin_pipeline(&env, &script, cancel_rx, |bd, total| {
            if throttle(&mut pg_throttle) {
                return;
            }
            self.emit(
                run_id,
                Phase::Postgres,
                EventStatus::Running,
                "Dump PostgreSQL (pg_dumpall)…",
                map_progress(R_POSTGRES, bd, total),
                Some(PhaseDetail { bytes_done: Some(bd), bytes_total: total, tag: Some("postgres".into()) }),
            );
        })
        .await;
        let postgres_added = match self.finish_phase(run_id, "postgres", outcome, R_POSTGRES.1).await {
            PhaseFlow::Continue(res) => {
                total_processed += res.bytes_processed;
                res.bytes_added
            }
            PhaseFlow::Stop => return,
        };

        // --- CONFIG ---
        let _ = runs.set_phase(run_id, "config").await;
        self.emit(run_id, Phase::Config, EventStatus::Running, "Archivage de la config…", R_CONFIG.0, None);
        let cfg_paths = self.inner.sources.config_paths();
        if cfg_paths.is_empty() {
            warn!("atelier-backup: aucun chemin de config trouvé");
        }
        let mut cfg_throttle = None;
        let outcome = restic::backup_paths(
            &self.inner.restic_bin,
            &env,
            "config",
            &cfg_paths,
            cancel_rx,
            |bd, total| {
                if throttle(&mut cfg_throttle) {
                    return;
                }
                self.emit(
                    run_id,
                    Phase::Config,
                    EventStatus::Running,
                    "Archivage de la config…",
                    map_progress(R_CONFIG, bd, total),
                    Some(PhaseDetail { bytes_done: Some(bd), bytes_total: total, tag: Some("config".into()) }),
                );
            },
        )
        .await;
        let config_added = match self.finish_phase(run_id, "config", outcome, R_CONFIG.1).await {
            PhaseFlow::Continue(res) => {
                total_processed += res.bytes_processed;
                res.bytes_added
            }
            PhaseFlow::Stop => return,
        };

        // --- RÉTENTION (non fatale : les snapshots sont déjà stockés) ---
        let _ = runs.set_phase(run_id, "prune").await;
        self.emit(run_id, Phase::Prune, EventStatus::Running, "Purge des anciennes sauvegardes…", R_PRUNE.0, None);
        if let Err(e) = restic::forget_prune(&self.inner.restic_bin, &env, t.retention_keep).await {
            warn!(error = %e, "atelier-backup: prune échoué (run conservé en succès)");
        }

        // --- SUCCÈS ---
        if let Err(e) = runs
            .finish_success(run_id, git_added, postgres_added, config_added, total_processed)
            .await
        {
            warn!(?e, "finish_success failed");
        }
        self.emit(run_id, Phase::Done, EventStatus::Success, "Sauvegarde terminée", 100, None);
        info!(run_id = %run_id, git_added, postgres_added, config_added, "atelier-backup: run success");
    }

    /// Gère l'issue d'une phase : persiste le snapshot, émet l'event, et indique
    /// si on continue ou si on arrête (annulation/échec).
    async fn finish_phase(
        &self,
        run_id: Uuid,
        tag: &str,
        outcome: RunOutcome,
        done_progress: u8,
    ) -> PhaseFlow {
        let runs = self.inner.runs.as_ref().unwrap();
        match outcome {
            RunOutcome::Ok(res) => {
                let _ = runs.upsert_snapshot(run_id, tag, "success", &res, None).await;
                self.emit(
                    run_id,
                    phase_of(tag),
                    EventStatus::Success,
                    &format!("{tag} : terminé"),
                    done_progress,
                    Some(PhaseDetail {
                        bytes_done: Some(res.bytes_added.max(0) as u64),
                        bytes_total: None,
                        tag: Some(tag.into()),
                    }),
                );
                PhaseFlow::Continue(res)
            }
            RunOutcome::Cancelled => {
                let _ = runs
                    .upsert_snapshot(run_id, tag, "failed", &SnapshotResult::default(), Some("annulé"))
                    .await;
                let _ = runs.finish_cancelled(run_id).await;
                self.emit(
                    run_id,
                    Phase::Cancelled,
                    EventStatus::Cancelled,
                    "Sauvegarde annulée",
                    done_progress,
                    Some(PhaseDetail { tag: Some(tag.into()), ..Default::default() }),
                );
                info!(run_id = %run_id, tag, "atelier-backup: run cancelled");
                PhaseFlow::Stop
            }
            RunOutcome::Failed(err) => {
                let _ = runs
                    .upsert_snapshot(run_id, tag, "failed", &SnapshotResult::default(), Some(&err))
                    .await;
                let msg = format!("{tag} : {err}");
                let _ = runs.finish_failed(run_id, &msg).await;
                self.emit(
                    run_id,
                    Phase::Failed,
                    EventStatus::Failed,
                    &msg,
                    done_progress,
                    Some(PhaseDetail { tag: Some(tag.into()), ..Default::default() }),
                );
                warn!(run_id = %run_id, tag, error = %err, "atelier-backup: run failed");
                PhaseFlow::Stop
            }
        }
    }

    async fn fail(&self, run_id: Uuid, msg: &str) {
        if let Some(runs) = self.inner.runs.as_ref() {
            let _ = runs.finish_failed(run_id, msg).await;
        }
        self.emit(run_id, Phase::Failed, EventStatus::Failed, msg, 0, None);
        warn!(run_id = %run_id, error = %msg, "atelier-backup: run failed (setup)");
    }

    async fn refresh_repo_stats(&self) {
        let Some(store) = self.inner.target.as_ref() else { return };
        let Ok(Some(t)) = store.get_full().await else { return };
        if !t.is_configured() || t.restic_password.as_deref().unwrap_or("").is_empty() {
            return;
        }
        let Ok(env) = rclone::build_env(&self.inner.rclone_bin, &t).await else { return };
        if let Ok(stats) = restic::stats(&self.inner.restic_bin, &env).await {
            *self.inner.repo_stats.lock().unwrap() = Some(stats);
        }
    }
}

enum PhaseFlow {
    Continue(SnapshotResult),
    Stop,
}

/// Throttle l'émission d'events de progression (≤ 1 / 200 ms) pour ne pas
/// inonder le WebSocket ni faire vibrer l'UI. true = sauter cet event. La fin de
/// phase est de toute façon couverte par l'event `success` de `finish_phase`.
fn throttle(last: &mut Option<std::time::Instant>) -> bool {
    let now = std::time::Instant::now();
    if let Some(t) = last {
        if now.duration_since(*t) < std::time::Duration::from_millis(200) {
            return true;
        }
    }
    *last = Some(now);
    false
}

fn phase_of(tag: &str) -> Phase {
    match tag {
        "git" => Phase::Git,
        "postgres" => Phase::Postgres,
        "config" => Phase::Config,
        _ => Phase::Config,
    }
}

fn noop() -> String {
    "backup disabled (postgres unreachable)".to_string()
}

/// Rapport du test de connexion.
#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub shares_ok: bool,
    pub repo_ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_maps_within_range() {
        assert_eq!(map_progress(R_GIT, 0, Some(100)), 2);
        assert_eq!(map_progress(R_GIT, 100, Some(100)), 35);
        assert_eq!(map_progress(R_GIT, 50, Some(100)), 18);
        // total inconnu (stdin) → début de plage.
        assert_eq!(map_progress(R_POSTGRES, 1234, None), 35);
    }

    #[test]
    fn generated_password_is_256_bits_hex() {
        let p = gen_password();
        assert_eq!(p.len(), 64);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(gen_password(), gen_password());
    }
}

async fn bootstrap(cfg: &BackupServiceConfig) -> anyhow::Result<Pool<Postgres>> {
    let admin_dsn = cfg
        .admin_dsn
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("no admin dsn"))?;
    let db_name = cfg.db_name.as_deref().unwrap_or(DEFAULT_DB_NAME);
    let admin_pool = migration::open_admin_pool(admin_dsn).await?;
    migration::ensure_database(&admin_pool, db_name).await?;
    let meta_dsn = migration::swap_db(admin_dsn, db_name);
    let pool = migration::open_pool(&meta_dsn).await?;
    migration::run_migrations(&pool).await?;
    Ok(pool)
}
