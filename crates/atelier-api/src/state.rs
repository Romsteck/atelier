use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiState {
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<hr_docs::Index>>,
}

impl ApiState {
    pub fn new(docs_dir: PathBuf, docs_index: Option<Arc<hr_docs::Index>>) -> Self {
        Self { docs_dir, docs_index }
    }
}
