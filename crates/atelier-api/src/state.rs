use std::path::PathBuf;
use std::sync::Arc;

use atelier_logging::LogIngestService;
use atelier_watcher::SurveillanceService;
use hr_apps::{AppRegistry, AppSupervisor, PortRegistry, db_manager::DbManager};
use hr_apps::context::ContextGenerator;
use hr_common::events::EventBus;
use hr_common::task_store::TaskStore;

#[derive(Clone)]
pub struct ApiState {
    // Docs
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<hr_docs::Index>>,

    // Git
    pub git: Arc<hr_git::GitService>,

    // Apps : sources synced + canonical writer
    pub apps_state_dir: PathBuf,
    pub apps_src_root: PathBuf,
    pub apps_runtime_root: PathBuf,

    // Tasks
    pub task_store: Arc<TaskStore>,

    // Dataverse
    pub dv: Option<Arc<hr_dataverse::manager::DataverseManager>>,

    // Apps supervisor (Phase 9 cutover) — Atelier devient le writer.
    pub events: Arc<EventBus>,
    pub app_registry: AppRegistry,
    pub port_registry: PortRegistry,
    pub supervisor: Arc<AppSupervisor>,
    pub db_manager: Arc<DbManager>,
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
}

impl ApiState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        docs_dir: PathBuf,
        docs_index: Option<Arc<hr_docs::Index>>,
        git: Arc<hr_git::GitService>,
        apps_state_dir: PathBuf,
        dv: Option<Arc<hr_dataverse::manager::DataverseManager>>,
        task_store: Arc<TaskStore>,
        apps_src_root: PathBuf,
        apps_runtime_root: PathBuf,
        events: Arc<EventBus>,
        app_registry: AppRegistry,
        port_registry: PortRegistry,
        supervisor: Arc<AppSupervisor>,
        db_manager: Arc<DbManager>,
        context_generator: Arc<ContextGenerator>,
        logs: LogIngestService,
        surveillance: SurveillanceService,
    ) -> Self {
        Self {
            docs_dir,
            docs_index,
            git,
            apps_state_dir,
            apps_src_root,
            apps_runtime_root,
            task_store,
            dv,
            events,
            app_registry,
            port_registry,
            supervisor,
            db_manager,
            context_generator,
            build_locks: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            logs,
            surveillance,
        }
    }
}
