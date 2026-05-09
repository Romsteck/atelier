//! `atelier-flowd` — daemon binary entrypoint.
//!
//! Boots a multi-thread Tokio runtime, loads the apps + flows registry,
//! exposes the HTTP API on `${HR_FLOWD_BIND}` (default `127.0.0.1:4002`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use chrono::Utc;
use dashmap::DashMap;
use hr_flow_daemon::{registry::Registry, routes, state::DaemonState};
use tokio::sync::Semaphore;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn init_tracing() {
    let filter = EnvFilter::try_from_env("HR_FLOWD_LOG")
        .or_else(|_| EnvFilter::try_new("info,hyper=warn,tower=warn"))
        .unwrap();
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().json().with_target(true))
        .init();
}

fn main() -> Result<()> {
    init_tracing();

    let workers = std::env::var("HR_FLOWD_WORKERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(num_cpus::get)
        .max(2);

    info!(workers, "starting atelier-flowd");

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .thread_name("flowd-worker")
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    let bind: SocketAddr = std::env::var("HR_FLOWD_BIND")
        .unwrap_or_else(|_| "127.0.0.1:4002".to_string())
        .parse()
        .context("HR_FLOWD_BIND must be a SocketAddr")?;

    let bearer = std::env::var("ATELIER_FLOW_TOKEN")
        .context("ATELIER_FLOW_TOKEN must be set in the daemon environment")?;
    if bearer.len() < 16 {
        anyhow::bail!("ATELIER_FLOW_TOKEN too short (need ≥ 16 chars)");
    }

    let apps_runtime_root: PathBuf = std::env::var("ATELIER_APPS_RUNTIME_ROOT")
        .unwrap_or_else(|_| "/var/lib/atelier/apps".to_string())
        .into();
    let apps_src_root: PathBuf = std::env::var("ATELIER_APPS_SRC_ROOT")
        .unwrap_or_else(|_| apps_runtime_root.display().to_string())
        .into();
    let apps_json_path: PathBuf = std::env::var("ATELIER_APPS_JSON")
        .unwrap_or_else(|_| "/opt/atelier/data/apps.json".to_string())
        .into();

    let default_slug_concurrency = std::env::var("HR_FLOWD_SLUG_CONCURRENCY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16usize);
    let global_max = std::env::var("HR_FLOWD_MAX_CONCURRENT_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256usize);
    let step_timeout_max_ms: u64 = std::env::var("HR_FLOWD_STEP_TIMEOUT_MAX_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300_000);
    let run_timeout_ms: u64 = std::env::var("HR_FLOWD_RUN_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600_000);
    let callback_timeout_ms: u64 = std::env::var("HR_FLOWD_CALLBACK_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000);

    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(8)
        .tcp_nodelay(true)
        .build()
        .context("build reqwest client")?;

    let registry = match Registry::load(&apps_json_path, &apps_src_root) {
        Ok(r) => r,
        Err(err) => {
            warn!(?err, "registry initial load failed; starting with empty registry");
            Registry::default()
        }
    };
    info!(
        apps = registry.apps.len(),
        flows = registry.flows.len(),
        "registry loaded"
    );

    let state = Arc::new(DaemonState {
        registry: ArcSwap::from_pointee(registry),
        runs: DashMap::new(),
        slug_semaphores: DashMap::new(),
        global_semaphore: Some(Arc::new(Semaphore::new(global_max))),
        apps_runtime_root,
        apps_src_root,
        apps_json_path,
        bearer,
        default_slug_concurrency,
        step_timeout_max_ms,
        run_timeout_ms,
        callback_timeout_ms,
        http,
        started_at: Utc::now(),
    });

    // Background task: SIGHUP triggers a registry reload (Phase 1.9).
    spawn_sighup_reload(state.clone());

    let app = routes::router(state.clone());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    info!(%bind, "atelier-flowd listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum::serve")?;

    info!("atelier-flowd stopped");
    Ok(())
}

fn spawn_sighup_reload(state: Arc<hr_flow_daemon::DaemonState>) {
    use tokio::signal::unix::{signal, SignalKind};
    tokio::spawn(async move {
        let mut hup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(err) => {
                warn!(?err, "SIGHUP handler not available; reload only via /v1/_admin/reload");
                return;
            }
        };
        while hup.recv().await.is_some() {
            info!("SIGHUP received; reloading registry");
            match Registry::load(&state.apps_json_path, &state.apps_src_root) {
                Ok(new) => state.registry.store(Arc::new(new)),
                Err(err) => warn!(?err, "registry reload failed"),
            }
        }
    });
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => info!("received SIGTERM"),
        _ = int.recv()  => info!("received SIGINT"),
    }
}
