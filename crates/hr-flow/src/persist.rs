//! Run persistence — JSON files under `runs/{run_id}.json`.
//!
//! v1 picks JSON-on-disk over Postgres tables: the pilot needs simple
//! drilldown and replay, runs are sparse, and the storage path stays usable
//! whether the consuming app holds a postgres pool or not. We can move to
//! `_flow_runs` / `_flow_run_steps` later by swapping in a different
//! `RunStore` implementation.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{FlowError, FlowResult};
use crate::executor::StepRecord;

/// One persisted run — header plus the full step tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDoc {
    pub run_id: String,
    pub flow_name: String,
    pub status: String,
    pub trigger_kind: String,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<RunErrorDoc>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: i64,
    pub steps: Vec<StepRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunErrorDoc {
    pub step_id: String,
    pub message: String,
    pub input: Option<Value>,
}

/// Storage abstraction. Default impl is JSON-on-disk; apps can plug their
/// own (e.g. straight to Postgres) by passing it to `FlowEngineBuilder`.
#[async_trait]
pub trait RunStore: Send + Sync {
    async fn save(&self, doc: &RunDoc) -> FlowResult<()>;
    async fn load(&self, run_id: &str) -> FlowResult<RunDoc>;
    async fn list(&self, flow_name: Option<&str>, limit: usize) -> FlowResult<Vec<RunDoc>>;
}

/// JSON-on-disk store. Writes one file per run; lists by sorted directory
/// scan. Good enough for pilot volumes (~hundreds of runs/day).
pub struct JsonRunStore {
    dir: PathBuf,
}

impl JsonRunStore {
    pub fn new(dir: impl Into<PathBuf>) -> FlowResult<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .map_err(|e| FlowError::Persistence(format!("create runs dir {}: {e}", dir.display())))?;
        Ok(Self { dir })
    }

    fn path_for(&self, run_id: &str) -> PathBuf {
        self.dir.join(format!("{run_id}.json"))
    }
}

#[async_trait]
impl RunStore for JsonRunStore {
    async fn save(&self, doc: &RunDoc) -> FlowResult<()> {
        let path = self.path_for(&doc.run_id);
        let body = serde_json::to_vec_pretty(doc)
            .map_err(|e| FlowError::Persistence(e.to_string()))?;
        tokio::fs::write(&path, body).await
            .map_err(|e| FlowError::Persistence(format!("write {}: {e}", path.display())))
    }

    async fn load(&self, run_id: &str) -> FlowResult<RunDoc> {
        let path = self.path_for(run_id);
        let body = tokio::fs::read(&path).await
            .map_err(|e| FlowError::Persistence(format!("read {}: {e}", path.display())))?;
        serde_json::from_slice(&body)
            .map_err(|e| FlowError::Persistence(e.to_string()))
    }

    async fn list(&self, flow_name: Option<&str>, limit: usize) -> FlowResult<Vec<RunDoc>> {
        list_dir(&self.dir, flow_name, limit).await
    }
}

async fn list_dir(dir: &Path, flow_name: Option<&str>, limit: usize) -> FlowResult<Vec<RunDoc>> {
    let mut entries: Vec<(std::fs::Metadata, std::path::PathBuf)> = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await
        .map_err(|e| FlowError::Persistence(format!("read_dir {}: {e}", dir.display())))?;
    while let Some(entry) = rd.next_entry().await
        .map_err(|e| FlowError::Persistence(e.to_string()))?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let meta = entry.metadata().await
            .map_err(|e| FlowError::Persistence(e.to_string()))?;
        entries.push((meta, path));
    }
    entries.sort_by(|a, b| {
        b.0.modified().ok().cmp(&a.0.modified().ok())
    });

    let mut out = Vec::new();
    for (_, path) in entries {
        if out.len() >= limit { break; }
        let body = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let doc: RunDoc = match serde_json::from_slice(&body) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if let Some(name) = flow_name {
            if doc.flow_name != name { continue; }
        }
        out.push(doc);
    }
    Ok(out)
}
