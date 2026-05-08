use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiState {
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<hr_docs::Index>>,
    pub store_dir: PathBuf,
    pub git: Arc<hr_git::GitService>,
    pub apps_state_dir: PathBuf,
    pub dv: Option<Arc<hr_dataverse::manager::DataverseManager>>,
}

impl ApiState {
    pub fn new(
        docs_dir: PathBuf,
        docs_index: Option<Arc<hr_docs::Index>>,
        store_dir: PathBuf,
        git: Arc<hr_git::GitService>,
        apps_state_dir: PathBuf,
        dv: Option<Arc<hr_dataverse::manager::DataverseManager>>,
    ) -> Self {
        Self {
            docs_dir,
            docs_index,
            store_dir,
            git,
            apps_state_dir,
            dv,
        }
    }
}
