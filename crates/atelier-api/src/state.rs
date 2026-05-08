use std::path::PathBuf;
use std::sync::Arc;

use hr_common::task_store::TaskStore;

#[derive(Clone)]
pub struct ApiState {
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<hr_docs::Index>>,
    pub store_dir: PathBuf,
    pub git: Arc<hr_git::GitService>,
    pub apps_state_dir: PathBuf,
    pub dv: Option<Arc<hr_dataverse::manager::DataverseManager>>,
    pub task_store: Arc<TaskStore>,
    /// CloudMaster local — sources canoniques des apps. Les flow defs vivent
    /// sous `{apps_src_root}/{slug}/src/flows/*.toml`.
    pub apps_src_root: PathBuf,
    /// Sync local depuis Medion runtime — `{apps_runtime_root}/{slug}/runs/*.json`.
    pub apps_runtime_root: PathBuf,
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
    ) -> Self {
        Self {
            docs_dir,
            docs_index,
            store_dir,
            git,
            apps_state_dir,
            dv,
            task_store,
            apps_src_root,
            apps_runtime_root,
        }
    }
}
