//! Per-engine schema cache.
//!
//! The cache key is `schema_version`: a single bump in `_dv_meta` is
//! enough to invalidate the cached `Arc<Schema>`. We never store more
//! than one entry — the prior version is dropped on update.
//!
//! This is intentionally tiny because a heavier cache (e.g. per-user
//! variants when auth lands) can be layered on top later.

use std::sync::Arc;

use async_graphql::dynamic::Schema;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct SchemaCache {
    entry: RwLock<Option<(u64, Arc<Schema>)>>,
}

impl SchemaCache {
    pub fn new() -> Self { Self::default() }

    /// Return the cached schema if its version matches `expected_version`.
    pub async fn get(&self, expected_version: u64) -> Option<Arc<Schema>> {
        let guard = self.entry.read().await;
        match &*guard {
            Some((v, sc)) if *v == expected_version => Some(Arc::clone(sc)),
            _ => None,
        }
    }

    /// Insert a new entry, replacing any stale one.
    pub async fn put(&self, version: u64, schema: Arc<Schema>) {
        let mut guard = self.entry.write().await;
        *guard = Some((version, schema));
    }

    /// Drop the cached entry.
    #[allow(dead_code)]
    pub async fn invalidate(&self) {
        let mut guard = self.entry.write().await;
        *guard = None;
    }
}
