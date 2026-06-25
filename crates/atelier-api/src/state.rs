use std::path::PathBuf;
use std::sync::Arc;

use atelier_backup::BackupService;
use atelier_logging::LogIngestService;
use atelier_watcher::SurveillanceService;
use atelier_apps::{AppRegistry, AppSupervisor, PortRegistry};
use atelier_apps::context::ContextGenerator;
use atelier_common::agent_ui_state::OpenTabsStore;
use atelier_common::events::EventBus;
use atelier_common::task_store::TaskStore;

#[derive(Clone)]
pub struct ApiState {
    // Docs
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<atelier_docs::Index>>,

    // Git
    pub git: Arc<atelier_git::GitService>,

    // Apps : sources synced + canonical writer
    pub apps_state_dir: PathBuf,
    pub apps_src_root: PathBuf,
    pub apps_runtime_root: PathBuf,

    // Tasks
    pub task_store: Arc<TaskStore>,

    /// Studio open-tabs state (conversations/files/diffs/commits + active tab),
    /// per app, in `atelier_meta`. Source of truth for cross-PC tab sync; pairs
    /// with the `agent_open_tabs` WS broadcast. No-op when Postgres is down.
    pub open_tabs: OpenTabsStore,

    // Dataverse
    pub dv: Option<Arc<atelier_dataverse::manager::DataverseManager>>,

    // Apps supervisor (Phase 9 cutover) — Atelier devient le writer.
    pub events: Arc<EventBus>,
    pub app_registry: AppRegistry,
    pub port_registry: PortRegistry,
    pub supervisor: Arc<AppSupervisor>,
    pub context_generator: Arc<ContextGenerator>,

    /// Per-slug build/ship locks, created once at boot and shared by the HTTP
    /// `ship` route and the MCP `app.build`/`app.ship` handlers. Without a
    /// shared map each request rebuilds an empty one and the BUILD_BUSY guard
    /// never fires.
    pub build_locks:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,

    /// Centralized logging — Postgres-backed ring/flush ingest service. Used
    /// by the in-process tracing layer, the HTTP `/api/logs/ingest` endpoint,
    /// and the WebSocket live stream.
    pub logs: LogIngestService,

    /// Surveillance IA service (findings / cron / memory). Endpoints under
    /// `/api/findings` and `/api/apps/:slug/surveillance/*` return 503 when
    /// this service is in noop mode (Postgres unreachable at boot).
    pub surveillance: SurveillanceService,

    /// Service de sauvegarde (restic+rclone vers Samba). Endpoints sous
    /// `/api/backup/*` ; renvoie 503 en mode noop (Postgres injoignable au boot).
    pub backup: BackupService,

    /// Intégration reverse-proxy Homeroute : appelle l'API hr-api existante pour
    /// créer/retirer des routes hostname pour les apps. Endpoints sous
    /// `/api/homeroute/*` ; renvoie 503 si le control-plane Postgres est absent.
    pub homeroute: crate::clients::homeroute_service::HomerouteService,

    /// Slugs whose `/apps/{slug}` path prefix must be PRESERVED (no-strip) when
    /// proxying to the app — required by Next.js apps whose `basePath`/`assetPrefix`
    /// expect the prefix on every request. SPA (Vite) / Axum apps want the prefix
    /// stripped and are absent here. Parsed once at boot from
    /// `ATELIER_PRESERVE_PREFIX_SLUGS` (comma-separated); defaults to `{"www"}`.
    pub preserve_prefix_slugs: std::collections::HashSet<String>,
}

impl ApiState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        docs_dir: PathBuf,
        docs_index: Option<Arc<atelier_docs::Index>>,
        git: Arc<atelier_git::GitService>,
        apps_state_dir: PathBuf,
        dv: Option<Arc<atelier_dataverse::manager::DataverseManager>>,
        task_store: Arc<TaskStore>,
        open_tabs: OpenTabsStore,
        apps_src_root: PathBuf,
        apps_runtime_root: PathBuf,
        events: Arc<EventBus>,
        app_registry: AppRegistry,
        port_registry: PortRegistry,
        supervisor: Arc<AppSupervisor>,
        context_generator: Arc<ContextGenerator>,
        logs: LogIngestService,
        surveillance: SurveillanceService,
        backup: BackupService,
        homeroute: crate::clients::homeroute_service::HomerouteService,
    ) -> Self {
        Self {
            docs_dir,
            docs_index,
            git,
            apps_state_dir,
            apps_src_root,
            apps_runtime_root,
            task_store,
            open_tabs,
            dv,
            events,
            app_registry,
            port_registry,
            supervisor,
            context_generator,
            build_locks: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            logs,
            surveillance,
            backup,
            homeroute,
            preserve_prefix_slugs: parse_preserve_prefix_slugs(),
        }
    }
}

/// Read `ATELIER_PRESERVE_PREFIX_SLUGS` (comma-separated app slugs) into a set.
/// Defaults to `{"www"}` when unset — `www` is the canonical path-routed Next.js
/// app, and this mirrors the `www` default of `ATELIER_NEXTJS_FALLBACK_SLUG`.
pub fn parse_preserve_prefix_slugs() -> std::collections::HashSet<String> {
    match std::env::var("ATELIER_PRESERVE_PREFIX_SLUGS") {
        Ok(raw) => raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => ["www".to_string()].into_iter().collect(),
    }
}
