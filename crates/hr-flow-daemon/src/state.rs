use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

use crate::registry::Registry;
use crate::supervisor::RunHandle;

/// Top-level shared daemon state. Cheaply cloneable — all heavy fields are
/// behind `Arc`. Pass as `State<Arc<DaemonState>>` in axum handlers.
pub struct DaemonState {
    /// RCU-swappable snapshot of apps + flows. Hot-reload swaps the whole
    /// `Arc<Registry>` atomically; in-flight runs keep their captured pointer
    /// for the entire run lifetime — no STW.
    pub registry: ArcSwap<Registry>,

    /// Active runs, keyed by run_id. Sharded for high concurrency.
    pub runs: DashMap<String, RunHandle>,

    /// Per-slug semaphores. A run is dispatched only after acquiring a permit
    /// for its slug; saturation returns 429 instead of queueing forever.
    pub slug_semaphores: DashMap<String, Arc<Semaphore>>,

    /// Optional global cap on concurrent runs (defense in depth on top of the
    /// per-slug semaphores). `None` = unlimited.
    pub global_semaphore: Option<Arc<Semaphore>>,

    /// Where this app's runs are persisted: `${root}/{slug}/runs/*.json`.
    pub apps_runtime_root: PathBuf,

    /// Where flow TOML sources live: `${root}/{slug}/src/flows/*.toml`. On the
    /// post-rapatriement Atelier this is the same as `apps_runtime_root` since
    /// rsync brings sources into `/var/lib/atelier/apps/{slug}/src/`.
    pub apps_src_root: PathBuf,

    /// Path to `apps.json` — read at boot and on `/v1/_admin/reload`.
    pub apps_json_path: PathBuf,

    /// Shared bearer token (`ATELIER_FLOW_TOKEN`) presented by Atelier API.
    pub bearer: String,

    /// Default per-slug max concurrent runs when `apps.json` does not specify.
    pub default_slug_concurrency: usize,

    /// Hard cap on per-step duration (ms). TOML `step.timeout_ms` is clamped
    /// to this value.
    pub step_timeout_max_ms: u64,

    /// Default per-run timeout (ms). Run is dropped past this. Connectors
    /// may still complete their last in-flight call.
    pub run_timeout_ms: u64,

    /// Default callback HTTP timeout (ms) used by the daemon when calling out
    /// to apps for custom actions / connectors.
    pub callback_timeout_ms: u64,

    /// Shared `reqwest::Client` (connection pool) used both for callbacks and
    /// other outbound HTTP from the daemon.
    pub http: Client,

    /// Dataverse manager — when set, the `dataverse` connector is wired
    /// natively in the engine factory (no callback to the app). `None` when
    /// `ATELIER_DV_ADMIN_URL` is not configured.
    pub dv: Option<std::sync::Arc<hr_dataverse::DataverseManager>>,

    pub started_at: DateTime<Utc>,
}

impl DaemonState {
    /// Acquire (or lazily create) a semaphore for a slug.
    pub fn semaphore_for(&self, slug: &str) -> Arc<Semaphore> {
        if let Some(s) = self.slug_semaphores.get(slug) {
            return s.clone();
        }
        let permits = self
            .registry
            .load()
            .apps
            .get(slug)
            .map(|a| a.max_concurrent_runs)
            .unwrap_or(self.default_slug_concurrency);
        let sem = Arc::new(Semaphore::new(permits.max(1)));
        // Race-safe insert: another caller may have inserted concurrently.
        self.slug_semaphores
            .entry(slug.to_string())
            .or_insert_with(|| sem.clone())
            .clone()
    }
}
