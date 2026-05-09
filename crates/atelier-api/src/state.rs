use std::path::PathBuf;
use std::sync::Arc;

use hr_apps::{AppRegistry, AppSupervisor, PortRegistry, db_manager::DbManager, todos::TodosManager};
use hr_apps::context::ContextGenerator;
use hr_common::events::EventBus;
use hr_common::task_store::TaskStore;

#[derive(Clone)]
pub struct ApiState {
    // Docs
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<hr_docs::Index>>,

    // Store / Git
    pub store_dir: PathBuf,
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
    pub todos_manager: Arc<TodosManager>,
    pub context_generator: Arc<ContextGenerator>,
}

impl ApiState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        docs_dir: PathBuf,
        docs_index: Option<Arc<hr_docs::Index>>,
        store_dir: PathBuf,
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
        todos_manager: Arc<TodosManager>,
        context_generator: Arc<ContextGenerator>,
    ) -> Self {
        Self {
            docs_dir,
            docs_index,
            store_dir,
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
            todos_manager,
            context_generator,
        }
    }
}
