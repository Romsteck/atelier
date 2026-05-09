use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use atelier_api::state::ApiState;
use tokio::net::{TcpListener, UnixListener};
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_HTTP_ADDR: &str = "0.0.0.0:4100";
const DEFAULT_IPC_SOCK: &str = "/run/atelier.sock";
const DEFAULT_DOCS_DIR: &str = "/var/lib/atelier/docs";
const DEFAULT_DOCS_INDEX: &str = "/var/lib/atelier/docs-index.sqlite";
const DEFAULT_STORE_DIR: &str = "/var/lib/atelier/store";
const DEFAULT_GIT_REPOS_DIR: &str = "/var/lib/atelier/git/repos";
const DEFAULT_APPS_STATE_DIR: &str = "/var/lib/atelier/state";
const DEFAULT_APPS_SRC_ROOT: &str = "/opt/homeroute/apps";
const DEFAULT_APPS_RUNTIME_ROOT: &str = "/var/lib/atelier/apps";
const DEFAULT_WEB_DIST: &str = "/opt/atelier/web/dist";
/// Données canoniques d'Atelier post-cutover (Atelier owns these files).
const DEFAULT_APPS_DATA_DIR: &str = "/opt/atelier/data";
const DEFAULT_BASE_DOMAIN: &str = "mynetwk.biz";
const DEFAULT_MCP_ENDPOINT: &str = "http://127.0.0.1:4100/mcp";
/// Hôte des Postgres apps (Medion). Le secret synchronisé contient `127.0.0.1`
/// (point de vue de Medion) — Atelier le swap vers cet hôte au registre des DSN.
const DEFAULT_DV_HOST: &str = "10.0.0.254";

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let http_addr = std::env::var("ATELIER_HTTP_ADDR").unwrap_or_else(|_| DEFAULT_HTTP_ADDR.to_string());
    let ipc_sock = PathBuf::from(std::env::var("ATELIER_IPC_SOCK").unwrap_or_else(|_| DEFAULT_IPC_SOCK.to_string()));
    let docs_dir = PathBuf::from(std::env::var("ATELIER_DOCS_DIR").unwrap_or_else(|_| DEFAULT_DOCS_DIR.to_string()));
    let docs_index_path = PathBuf::from(std::env::var("ATELIER_DOCS_INDEX").unwrap_or_else(|_| DEFAULT_DOCS_INDEX.to_string()));
    let store_dir = PathBuf::from(std::env::var("ATELIER_STORE_DIR").unwrap_or_else(|_| DEFAULT_STORE_DIR.to_string()));
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
        docs_index = %docs_index_path.display(),
        store_dir = %store_dir.display(),
        git_repos_dir = %git_repos_dir.display(),
        apps_state_dir = %apps_state_dir.display(),
        apps_src_root = %apps_src_root.display(),
        apps_runtime_root = %apps_runtime_root.display(),
        web_dist = %web_dist.display(),
        "atelier starting"
    );

    let docs_index = open_docs_index(&docs_index_path, &docs_dir);
    let git = Arc::new(hr_git::GitService::with_repos_dir(git_repos_dir));
    let dv = init_dv(&apps_state_dir).await;
    let task_store = open_task_store(&apps_state_dir);

    // Phase 9 — Apps supervisor wiring. Atelier devient le canonical owner des
    // registries (apps.json, port-registry.json) sous apps_data_dir. Au premier
    // boot, on seed depuis le mirror synced.
    seed_apps_data(&apps_data_dir, &apps_state_dir);
    let events = Arc::new(hr_common::events::EventBus::new());
    let app_registry = hr_apps::AppRegistry::load_from(apps_data_dir.join("apps.json"))
        .await
        .expect("Failed to load app registry");
    let port_registry = hr_apps::PortRegistry::load_from(
        apps_data_dir.join("port-registry.json"),
        3001,
    )
    .await
    .expect("Failed to load port registry");
    let supervisor = Arc::new(hr_apps::AppSupervisor::new(
        app_registry.clone(),
        port_registry.clone(),
        events.app_state.clone(),
    ));
    let db_manager = Arc::new(hr_apps::db_manager::DbManager::new(apps_src_root.clone()));
    let todos_manager = Arc::new(hr_apps::todos::TodosManager::new(
        apps_src_root.clone(),
        events.clone(),
    ));
    let context_generator = Arc::new(hr_apps::context::ContextGenerator::new(
        apps_src_root.clone(),
        base_domain.clone(),
        mcp_endpoint.clone(),
    ));
    info!(
        apps = app_registry.list().await.len(),
        "hr-apps supervisor wired (Phase 9 prep)"
    );

    // Adopt existing transient units for apps marked Running. À chaque boot
    // d'Atelier, on raccroche les processus déjà supervisés par systemd
    // (sinon /api/apps/.../status renvoie null jusqu'au prochain control).
    if let Err(err) = supervisor.start_all_running().await {
        warn!(?err, "supervisor.start_all_running failed");
    }

    let state = ApiState::new(
        docs_dir.clone(),
        docs_index,
        store_dir.clone(),
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
        db_manager,
        todos_manager,
        context_generator,
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

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
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

fn open_task_store(state_dir: &PathBuf) -> Arc<hr_common::task_store::TaskStore> {
    let path = state_dir.join("tasks.db");
    match hr_common::task_store::TaskStore::new(&path) {
        Ok(store) => {
            info!(path = %path.display(), "task_store opened");
            Arc::new(store)
        }
        Err(err) => {
            warn!(?err, "task_store init failed — endpoints retourneront vide");
            // Fallback : on ouvre une DB temporaire éphémère, pour ne pas casser
            // le boot du service. Les endpoints /api/tasks renverront simplement
            // "tasks: [], total: 0" jusqu'au prochain sync.
            let tmp = std::env::temp_dir().join("atelier-tasks-empty.db");
            let _ = std::fs::remove_file(&tmp);
            Arc::new(
                hr_common::task_store::TaskStore::new(&tmp)
                    .expect("fallback task_store"),
            )
        }
    }
}

async fn init_dv(state_dir: &PathBuf) -> Option<Arc<hr_dataverse::manager::DataverseManager>> {
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

    let mgr = match hr_dataverse::manager::DataverseManager::connect_admin(
        admin_dsn,
        hr_dataverse::provisioning::ProvisioningConfig::default(),
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
            serde_json::from_slice::<hr_dataverse::manager::SecretsFile>(&bytes)
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

fn open_docs_index(index_path: &PathBuf, docs_dir: &PathBuf) -> Option<Arc<hr_docs::Index>> {
    match hr_docs::Index::open_or_rebuild(index_path, docs_dir.clone()) {
        Ok(idx) => {
            let count = idx.count().unwrap_or(0);
            info!(
                fts5 = idx.fts5_available,
                entries = count,
                index = %index_path.display(),
                "docs index opened"
            );
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
