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
const DEFAULT_WEB_DIST: &str = "/opt/atelier/web/dist";

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let http_addr = std::env::var("ATELIER_HTTP_ADDR").unwrap_or_else(|_| DEFAULT_HTTP_ADDR.to_string());
    let ipc_sock = PathBuf::from(std::env::var("ATELIER_IPC_SOCK").unwrap_or_else(|_| DEFAULT_IPC_SOCK.to_string()));
    let docs_dir = PathBuf::from(std::env::var("ATELIER_DOCS_DIR").unwrap_or_else(|_| DEFAULT_DOCS_DIR.to_string()));
    let docs_index_path = PathBuf::from(std::env::var("ATELIER_DOCS_INDEX").unwrap_or_else(|_| DEFAULT_DOCS_INDEX.to_string()));
    let store_dir = PathBuf::from(std::env::var("ATELIER_STORE_DIR").unwrap_or_else(|_| DEFAULT_STORE_DIR.to_string()));
    let web_dist = PathBuf::from(std::env::var("ATELIER_WEB_DIST").unwrap_or_else(|_| DEFAULT_WEB_DIST.to_string()));

    info!(
        http_addr = %http_addr,
        ipc_sock = %ipc_sock.display(),
        docs_dir = %docs_dir.display(),
        docs_index = %docs_index_path.display(),
        store_dir = %store_dir.display(),
        web_dist = %web_dist.display(),
        "atelier starting"
    );

    let docs_index = open_docs_index(&docs_index_path, &docs_dir);
    let state = ApiState::new(docs_dir.clone(), docs_index, store_dir.clone());

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
