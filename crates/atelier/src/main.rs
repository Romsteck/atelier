use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use atelier_api::state::ApiState;
use atelier_logging::{LogIngestConfig, LogIngestService, LoggingLayer};
use atelier_backup::{BackupService, BackupServiceConfig, SourcePaths};
use atelier_watcher::{SurveillanceConfig, SurveillanceService};
use tokio::net::{TcpListener, UnixListener};
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const DEFAULT_HTTP_ADDR: &str = "0.0.0.0:4100";
const DEFAULT_IPC_SOCK: &str = "/run/atelier.sock";
const DEFAULT_DOCS_DIR: &str = "/var/lib/atelier/docs";
const DEFAULT_GIT_REPOS_DIR: &str = "/var/lib/atelier/git/repos";
const DEFAULT_APPS_STATE_DIR: &str = "/var/lib/atelier/state";
const DEFAULT_APPS_SRC_ROOT: &str = "/var/lib/atelier/apps";
const DEFAULT_APPS_RUNTIME_ROOT: &str = "/var/lib/atelier/apps";
const DEFAULT_WEB_DIST: &str = "/opt/atelier/web/dist";
/// Données canoniques d'Atelier post-cutover (Atelier owns these files).
const DEFAULT_APPS_DATA_DIR: &str = "/opt/atelier/data";
const DEFAULT_BASE_DOMAIN: &str = "mynetwk.biz";
/// Default MCP endpoint pour les agents inside-app. Direct IP:port pour
/// rester équivalent au legacy `http://10.0.0.254:4001/mcp` — hr-edge
/// exige une auth cookie sur `atelier.mynetwk.biz` qui collisionne avec le
/// Bearer-token MCP. Le Bearer (header `Authorization`) reste l'unique
/// gardien de la surface MCP.
const DEFAULT_MCP_ENDPOINT: &str = "http://10.0.0.254:4100/mcp";
/// Hôte des Postgres apps (Medion). Le secret synchronisé contient `127.0.0.1`
/// (point de vue de Medion) — Atelier le swap vers cet hôte au registre des DSN.
const DEFAULT_DV_HOST: &str = "10.0.0.254";

#[tokio::main]
async fn main() -> Result<()> {
    // Bootstrap the centralized logging service first, then install tracing
    // layers that pipe events into it. If Postgres is unreachable at boot,
    // LogIngestService runs in noop mode and Atelier still starts.
    let logs = init_logs_ingest().await;
    init_tracing(logs.clone());

    let http_addr = std::env::var("ATELIER_HTTP_ADDR").unwrap_or_else(|_| DEFAULT_HTTP_ADDR.to_string());
    let ipc_sock = PathBuf::from(std::env::var("ATELIER_IPC_SOCK").unwrap_or_else(|_| DEFAULT_IPC_SOCK.to_string()));
    let docs_dir = PathBuf::from(std::env::var("ATELIER_DOCS_DIR").unwrap_or_else(|_| DEFAULT_DOCS_DIR.to_string()));
    let git_repos_dir = PathBuf::from(std::env::var("ATELIER_GIT_REPOS_DIR").unwrap_or_else(|_| DEFAULT_GIT_REPOS_DIR.to_string()));
    let apps_state_dir = PathBuf::from(std::env::var("ATELIER_APPS_STATE_DIR").unwrap_or_else(|_| DEFAULT_APPS_STATE_DIR.to_string()));
    let apps_src_root = PathBuf::from(std::env::var("ATELIER_APPS_SRC_ROOT").unwrap_or_else(|_| DEFAULT_APPS_SRC_ROOT.to_string()));
    let apps_runtime_root = PathBuf::from(std::env::var("ATELIER_APPS_RUNTIME_ROOT").unwrap_or_else(|_| DEFAULT_APPS_RUNTIME_ROOT.to_string()));
    let apps_data_dir = PathBuf::from(std::env::var("ATELIER_APPS_DATA_DIR").unwrap_or_else(|_| DEFAULT_APPS_DATA_DIR.to_string()));
    let base_domain = std::env::var("ATELIER_BASE_DOMAIN").unwrap_or_else(|_| DEFAULT_BASE_DOMAIN.to_string());
    let mcp_endpoint = std::env::var("ATELIER_MCP_ENDPOINT").unwrap_or_else(|_| DEFAULT_MCP_ENDPOINT.to_string());
    let web_dist = PathBuf::from(std::env::var("ATELIER_WEB_DIST").unwrap_or_else(|_| DEFAULT_WEB_DIST.to_string()));

    info!(
        http_addr = %http_addr,
        ipc_sock = %ipc_sock.display(),
        docs_dir = %docs_dir.display(),
        git_repos_dir = %git_repos_dir.display(),
        apps_state_dir = %apps_state_dir.display(),
        apps_src_root = %apps_src_root.display(),
        apps_runtime_root = %apps_runtime_root.display(),
        web_dist = %web_dist.display(),
        "atelier starting"
    );

    // Chemins capturés par la sauvegarde (résolus avant que git_repos_dir ne soit
    // déplacé dans le GitService). git_dir = parent de .../git/repos.
    let backup_sources = SourcePaths {
        git_dir: git_repos_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| git_repos_dir.clone()),
        env_file: PathBuf::from(
            std::env::var("ATELIER_BACKUP_ENV_FILE").unwrap_or_else(|_| "/opt/atelier/.env".to_string()),
        ),
        data_dir: apps_data_dir.clone(),
        dv_secrets: apps_state_dir.join("dataverse-secrets.json"),
        apps_runtime_root: apps_runtime_root.clone(),
        docs_dir: docs_dir.clone(),
    };

    let git = Arc::new(atelier_git::GitService::with_repos_dir(git_repos_dir));
    let dv = init_dv(&apps_state_dir).await;
    // Shared control-plane Postgres pool (atelier_meta). Backs the task store +
    // docs index now; the registry/port stores migrate onto it in a later stage.
    let meta_pool = init_control_db().await;
    let task_store =
        Arc::new(atelier_common::task_store::TaskStore::new(meta_pool.clone()).await);
    let docs_index = open_docs_index(&meta_pool, &docs_dir).await;

    // Apps supervisor wiring. The registries (apps + ports) live in the shared
    // `atelier_meta` Postgres in a single `applications` table — app and port in
    // one transactional row, so the old apps.json/port-registry.json desync (and
    // its boot-time `reconcile` hack) can no longer happen. Postgres is therefore
    // a hard dependency for supervision: fail fast if the pool is unavailable
    // (mirrors the previous `.expect()` on the local-file registry load).
    let registry_pool = meta_pool
        .clone()
        .expect("control-plane Postgres (atelier_meta) required for the app registry");
    // One-shot backfill from the legacy JSON/SQLite files if the DB is empty.
    if let Err(err) = backfill_control_plane(&registry_pool, &apps_data_dir, &apps_state_dir).await {
        warn!(?err, "control-plane backfill skipped/failed");
    }
    let events = Arc::new(atelier_common::events::EventBus::new());
    let app_registry = atelier_apps::AppRegistry::new(registry_pool.clone())
        .await
        .expect("Failed to load app registry from Postgres");
    let port_registry =
        atelier_apps::PortRegistry::new(registry_pool.clone(), 3001)
            .await
            .expect("Failed to load port registry from Postgres");

    let supervisor = Arc::new(atelier_apps::AppSupervisor::new(
        app_registry.clone(),
        port_registry.clone(),
        events.app_state.clone(),
    ));
    let context_generator = Arc::new(atelier_apps::context::ContextGenerator::new(
        apps_src_root.clone(),
        base_domain.clone(),
        mcp_endpoint.clone(),
    ));
    info!(
        apps = app_registry.list().await.len(),
        "atelier-apps supervisor wired (Phase 9 prep)"
    );

    // Adopt existing transient units for apps marked Running. À chaque boot
    // d'Atelier, on raccroche les processus déjà supervisés par systemd
    // (sinon /api/apps/.../status renvoie null jusqu'au prochain control).
    if let Err(err) = supervisor.start_all_running().await {
        warn!(?err, "supervisor.start_all_running failed");
    }

    // Surveillance IA (Codex code-review + suggestions + sécurité). Migrate
    // schema, spawn git_watcher loop. Runs manuels uniquement (pas de
    // scheduler). Inert tant que le binaire `codex` n'est pas installé. Noop
    // si pas de DSN.
    let surveillance = init_surveillance(&app_registry, &apps_src_root).await;

    // Sauvegarde restic+rclone vers Samba. Noop si pas de DSN ; runs manuels
    // (scheduler présent mais désactivé tant que schedule_enabled=false).
    let backup = init_backup(backup_sources).await;

    let state = ApiState::new(
        docs_dir.clone(),
        docs_index,
        git,
        apps_state_dir.clone(),
        dv,
        task_store,
        apps_src_root,
        apps_runtime_root,
        events,
        app_registry,
        port_registry,
        supervisor,
        context_generator,
        logs,
        surveillance,
        backup,
    );
    info!(
        slugs = ?state.preserve_prefix_slugs,
        "apps_proxy: prefix-preserving (no-strip) slugs"
    );

    let web_dist_opt = if web_dist.is_dir() { Some(web_dist) } else { None };
    let app = atelier_api::router(state, web_dist_opt);

    let listener = TcpListener::bind(&http_addr)
        .await
        .with_context(|| format!("bind HTTP {http_addr}"))?;
    info!(addr = %http_addr, "http listener bound");

    let http_task = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            error!(?err, "http server exited");
        }
    });

    let ipc_task = tokio::spawn(serve_ipc(ipc_sock.clone()));

    shutdown_signal().await;
    info!("shutdown signal received");

    if ipc_sock.exists() {
        if let Err(err) = tokio::fs::remove_file(&ipc_sock).await {
            warn!(?err, path = %ipc_sock.display(), "failed to remove ipc socket");
        }
    }

    http_task.abort();
    ipc_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), http_task).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), ipc_task).await;

    info!("atelier stopped");
    Ok(())
}

/// Bootstrap the surveillance service. Reuses `ATELIER_DV_ADMIN_URL` (the
/// dataverse admin DSN) to CREATE DATABASE `atelier_meta` on first boot,
/// run its migrations, and spawn the git_watcher loop. `seed_apps` carries the
/// registry's slugs + stack hints (used for prompts + git_watcher). If the env
/// var is missing, the service starts in noop mode.
///
/// Codex CLI invocation is configured via env (all optional, sane defaults):
///   - `ATELIER_CODEX_BIN`         (default "codex")
///   - `ATELIER_CODEX_ARGS`        (default "exec --sandbox read-only", space-split)
///   - `ATELIER_CODEX_TIMEOUT_SECS`(default 600)
///   - `ATELIER_CODEX_MAX_CONCURRENT` (default 2)
///
/// The Atelier MCP server is registered once in `~/.codex/config.toml` via
/// `codex mcp add atelier --url http://127.0.0.1:4100/mcp --bearer-token-env-var MCP_TOKEN`.
async fn init_surveillance(
    registry: &atelier_apps::AppRegistry,
    apps_src_root: &PathBuf,
) -> SurveillanceService {
    let admin_dsn = std::env::var("ATELIER_DV_ADMIN_URL")
        .ok()
        .filter(|s| !s.is_empty());
    if admin_dsn.is_none() {
        warn!("ATELIER_DV_ADMIN_URL absent — surveillance in noop mode");
    }
    let seed_apps: Vec<atelier_watcher::AppMeta> = registry
        .list()
        .await
        .into_iter()
        .map(|a| atelier_watcher::AppMeta {
            slug: a.slug,
            stack: a.stack.display_name().to_string(),
        })
        .collect();

    let codex_bin = std::env::var("ATELIER_CODEX_BIN").unwrap_or_else(|_| "codex".to_string());
    let codex_args: Vec<String> = std::env::var("ATELIER_CODEX_ARGS")
        .unwrap_or_else(|_| "exec --sandbox read-only".to_string())
        .split_whitespace()
        .map(String::from)
        .collect();
    let timeout_secs = std::env::var("ATELIER_CODEX_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600u64);
    let max_concurrent = std::env::var("ATELIER_CODEX_MAX_CONCURRENT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2usize);

    SurveillanceService::start(SurveillanceConfig {
        admin_dsn,
        db_name: None,
        seed_apps,
        apps_src_root: apps_src_root.clone(),
        codex: atelier_watcher::CodexConfig {
            bin: codex_bin,
            args: codex_args,
            timeout: Duration::from_secs(timeout_secs),
        },
        max_concurrent,
    })
    .await
}

/// Bootstrap du service de sauvegarde. Réutilise `ATELIER_DV_ADMIN_URL` pour
/// CREATE DATABASE `atelier_meta` (si besoin) + migrations. Les binaires sont
/// configurables (defaults restic/rclone/pg_dumpall). Noop si DSN absent.
///
///   - `ATELIER_RESTIC_BIN`      (default "restic")
///   - `ATELIER_RCLONE_BIN`      (default "rclone")
///   - `ATELIER_PG_DUMPALL_BIN`  (default "pg_dumpall")
///   - `ATELIER_BACKUP_PG_USER`  (default "postgres")
async fn init_backup(sources: SourcePaths) -> BackupService {
    let admin_dsn = std::env::var("ATELIER_DV_ADMIN_URL")
        .ok()
        .filter(|s| !s.is_empty());
    if admin_dsn.is_none() {
        warn!("ATELIER_DV_ADMIN_URL absent — backup in noop mode");
    }
    BackupService::start(BackupServiceConfig {
        admin_dsn,
        db_name: None,
        sources,
        restic_bin: std::env::var("ATELIER_RESTIC_BIN").unwrap_or_else(|_| "restic".to_string()),
        rclone_bin: std::env::var("ATELIER_RCLONE_BIN").unwrap_or_else(|_| "rclone".to_string()),
        pg_dumpall_bin: std::env::var("ATELIER_PG_DUMPALL_BIN")
            .unwrap_or_else(|_| "pg_dumpall".to_string()),
        pg_run_user: std::env::var("ATELIER_BACKUP_PG_USER").unwrap_or_else(|_| "postgres".to_string()),
    })
    .await
}

fn init_tracing(logs: LogIngestService) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_target(true);
    let logging_layer = LoggingLayer::new(logs, "atelier");
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(logging_layer)
        .init();
}

/// Bootstrap the Postgres-backed log ingest service. Reads:
///
/// - `ATELIER_LOGS_DB_ADMIN_URL` — admin DSN (typically the dataverse admin)
///   used to CREATE DATABASE/CREATE ROLE on first boot. Optional if the DB
///   and writer role already exist.
/// - `ATELIER_LOGS_DB_URL` — writer DSN used for INSERT/SELECT on
///   `events_log`. If absent, falls back to the admin DSN swapped to
///   `atelier_logs`.
/// - `ATELIER_LOGS_WRITER_PASSWORD` — only consulted on the very first boot
///   when the writer role doesn't yet exist.
///
/// If neither admin nor writer DSN is set, the service starts in noop mode
/// (logs go to stdout only, no Postgres persistence). This keeps `atelier`
/// bootable even when Postgres is unreachable.
async fn init_logs_ingest() -> LogIngestService {
    let admin_dsn = std::env::var("ATELIER_LOGS_DB_ADMIN_URL")
        .or_else(|_| std::env::var("ATELIER_DV_ADMIN_URL"))
        .ok()
        .filter(|s| !s.is_empty());
    let writer_dsn = std::env::var("ATELIER_LOGS_DB_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let writer_password = std::env::var("ATELIER_LOGS_WRITER_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty());

    if admin_dsn.is_none() && writer_dsn.is_none() {
        warn!("ATELIER_LOGS_DB_URL / ATELIER_DV_ADMIN_URL absent — log ingest in noop mode");
    }

    let cfg = LogIngestConfig {
        admin_dsn,
        writer_dsn,
        writer_password,
        ..LogIngestConfig::default()
    };
    LogIngestService::start(cfg).await
}

/// Seed `/opt/atelier/data/{apps,port-registry}.json` from the read-only mirror
/// `/var/lib/atelier/state/` if Atelier's canonical writer copy is missing.
/// Idempotent : ne touche pas aux fichiers existants.
fn seed_apps_data(data_dir: &PathBuf, state_dir: &PathBuf) {
    if let Err(err) = std::fs::create_dir_all(data_dir) {
        warn!(?err, path = %data_dir.display(), "failed to create apps data dir");
        return;
    }
    for file in ["apps.json", "port-registry.json"] {
        let dst = data_dir.join(file);
        if dst.exists() {
            continue;
        }
        let src = state_dir.join(file);
        if !src.exists() {
            continue;
        }
        match std::fs::copy(&src, &dst) {
            Ok(n) => info!(bytes = n, src = %src.display(), dst = %dst.display(), "seeded apps data"),
            Err(err) => warn!(?err, src = %src.display(), "seed apps data failed"),
        }
    }
}

/// One-shot, idempotent import of the legacy file-based control plane into the
/// `applications` table. Runs only when the table is empty (fresh cutover);
/// once populated, Postgres is the source of truth and this is a no-op.
///
/// Merges `apps.json` (app metadata) with `port-registry.json` (slug→port) into
/// one row per app. The legacy files are left in place as a rollback safety net.
async fn backfill_control_plane(
    pool: &atelier_common::control_db::sqlx::PgPool,
    apps_data_dir: &PathBuf,
    state_dir: &PathBuf,
) -> anyhow::Result<()> {
    use atelier_common::control_db::sqlx::{Row, query};

    // Ensure the legacy files are present (seed from the read-only mirror) so a
    // first boot after the schema migration can import them.
    seed_apps_data(apps_data_dir, state_dir);

    let row = query("SELECT COUNT(*) AS n FROM applications")
        .fetch_one(pool)
        .await?;
    let existing: i64 = row.get("n");
    if existing > 0 {
        return Ok(());
    }

    let apps_path = apps_data_dir.join("apps.json");
    let ports_path = apps_data_dir.join("port-registry.json");
    if !apps_path.exists() {
        info!("backfill: no apps.json — starting with an empty registry");
        return Ok(());
    }

    let apps: Vec<atelier_apps::Application> = {
        let bytes = std::fs::read(&apps_path)?;
        if bytes.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&bytes)?
        }
    };
    let ports: std::collections::BTreeMap<String, u16> = if ports_path.exists() {
        let bytes = std::fs::read(&ports_path)?;
        if bytes.is_empty() {
            Default::default()
        } else {
            // {base_port, assignments:{slug:port}} — only assignments matter here.
            let v: serde_json::Value = serde_json::from_slice(&bytes)?;
            serde_json::from_value(v.get("assignments").cloned().unwrap_or_default())
                .unwrap_or_default()
        }
    } else {
        Default::default()
    };

    let mut imported = 0u32;
    for mut app in apps {
        // The port registry file is authoritative for the port (apps.json's copy
        // is the one that "drifts"); fall back to the app's own port otherwise.
        let port = ports.get(&app.slug).copied().unwrap_or(app.port);
        app.port = port;
        let data = serde_json::to_value(&app)?;
        let port_col: Option<i32> = if port == 0 { None } else { Some(port as i32) };
        query(
            "INSERT INTO applications (slug, port, state, data, updated_at) \
             VALUES ($1, $2, $3, $4, now()) ON CONFLICT (slug) DO NOTHING",
        )
        .bind(&app.slug)
        .bind(port_col)
        .bind(app.state.as_str())
        .bind(&data)
        .execute(pool)
        .await?;
        imported += 1;
    }
    info!(imported, "control-plane backfill complete (apps.json/port-registry.json → Postgres)");
    Ok(())
}

/// Open the shared control-plane Postgres pool (`atelier_meta`) via the
/// dataverse admin DSN and apply the control-plane DDL. Returns `None` when the
/// DSN is absent or Postgres is unreachable — the control-plane stores then run
/// in degraded mode (matching the soft-dependency behaviour of dv/logs/surveillance).
async fn init_control_db() -> Option<atelier_common::control_db::sqlx::PgPool> {
    let admin_dsn = match std::env::var("ATELIER_DV_ADMIN_URL").ok().filter(|s| !s.is_empty()) {
        Some(u) => u,
        None => {
            warn!("ATELIER_DV_ADMIN_URL absent — control-plane Postgres désactivé (mode dégradé)");
            return None;
        }
    };
    match atelier_common::control_db::bootstrap(&admin_dsn).await {
        Ok(pool) => {
            info!("control-plane Postgres ready (atelier_meta)");
            Some(pool)
        }
        Err(err) => {
            warn!(?err, "control-plane Postgres bootstrap failed — mode dégradé");
            None
        }
    }
}

async fn init_dv(state_dir: &PathBuf) -> Option<Arc<atelier_dataverse::manager::DataverseManager>> {
    let admin_dsn = match std::env::var("ATELIER_DV_ADMIN_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            warn!("ATELIER_DV_ADMIN_URL absent — /api/dv désactivé");
            return None;
        }
    };
    let secrets_path = state_dir.join("dataverse-secrets.json");
    if !secrets_path.exists() {
        warn!(path = %secrets_path.display(), "dataverse-secrets.json absent — /api/dv désactivé");
        return None;
    }

    let mgr = match atelier_dataverse::manager::DataverseManager::connect_admin(
        admin_dsn,
        atelier_dataverse::provisioning::ProvisioningConfig::default(),
        Some(secrets_path.clone()),
    )
    .await
    {
        Ok(m) => m,
        Err(err) => {
            warn!(?err, "DataverseManager init failed — /api/dv désactivé");
            return None;
        }
    };

    // Override des DSN per-slug pour rediriger 127.0.0.1 → 10.0.0.254.
    let dv_host = std::env::var("ATELIER_DV_HOST").unwrap_or_else(|_| DEFAULT_DV_HOST.to_string());
    if let Ok(bytes) = std::fs::read(&secrets_path) {
        if let Ok(secrets) =
            serde_json::from_slice::<atelier_dataverse::manager::SecretsFile>(&bytes)
        {
            let mut applied = 0;
            for (slug, sec) in secrets.apps.iter() {
                let swapped = sec
                    .dsn
                    .replace("127.0.0.1", &dv_host)
                    .replace("@localhost:", &format!("@{dv_host}:"));
                mgr.set_dsn_override(slug.clone(), swapped).await;
                applied += 1;
            }
            info!(
                count = applied,
                host = %dv_host,
                "Dataverse DSN overrides loaded"
            );
        }
    }

    Some(Arc::new(mgr))
}

/// Build the Postgres-backed docs search index over the shared `atelier_meta`
/// pool, rebuilding it from the filesystem if the table is empty. Returns `None`
/// when the control-plane pool is absent (Postgres down) — search then degrades
/// to 503, matching the previous behaviour when the SQLite index failed to open.
async fn open_docs_index(
    meta_pool: &Option<atelier_common::control_db::sqlx::PgPool>,
    docs_dir: &PathBuf,
) -> Option<Arc<atelier_docs::Index>> {
    let pool = meta_pool.clone()?;
    match atelier_docs::Index::new_or_rebuild(pool, docs_dir.clone()).await {
        Ok(idx) => {
            let count = idx.count().await.unwrap_or(0);
            info!(entries = count, "docs index ready (atelier_meta)");
            Some(Arc::new(idx))
        }
        Err(err) => {
            warn!(?err, "failed to open docs index — search disabled");
            None
        }
    }
}

async fn serve_ipc(path: PathBuf) -> Result<()> {
    if path.exists() {
        if let Err(err) = tokio::fs::remove_file(&path).await {
            warn!(?err, path = %path.display(), "failed to remove stale ipc socket");
        }
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind IPC {}", path.display()))?;
    info!(path = %path.display(), "ipc listener bound");

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(c) => c,
            Err(err) => {
                warn!(?err, "ipc accept failed");
                continue;
            }
        };
        drop(stream);
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
