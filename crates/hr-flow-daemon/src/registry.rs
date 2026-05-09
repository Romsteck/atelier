//! Apps + flows snapshot, hot-swappable via `ArcSwap`.
//!
//! `Registry::load` scans `apps.json` and the per-slug `flows/` directory
//! exhaustively, returning a snapshot ready to publish via `ArcSwap::store`.
//! Parsing is fault-tolerant: a single broken TOML logs a warning but does
//! not poison the whole reload.

use chrono::{DateTime, Utc};
use hr_flow::{parse_flow_toml, FlowDef};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::error::{DaemonError, DaemonResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CircuitState {
    #[default]
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct AppEntry {
    pub slug: String,
    pub callback_url: Option<String>,
    pub callback_token: Option<String>,
    pub max_concurrent_runs: usize,
    pub circuit_breaker: CircuitState,
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub apps: HashMap<String, AppEntry>,
    /// keyed by (slug, flow_name)
    pub flows: HashMap<(String, String), FlowDef>,
    pub loaded_at: DateTime<Utc>,
}

/// Subset of `hr_apps::types::Application` we actually care about. We use a
/// local mirror struct (rather than depending on `hr-apps` for deserialisation)
/// so the daemon stays robust if `apps.json` carries unknown fields from a
/// future schema bump — `#[serde(deny_unknown_fields)]` is *not* set here.
#[derive(Debug, Deserialize)]
struct AppsJsonRecord {
    slug: String,
    #[serde(default)]
    flow_callback_url: Option<String>,
    #[serde(default)]
    flow_callback_token: Option<String>,
    #[serde(default)]
    max_concurrent_runs: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AppsJsonShape {
    /// `[{ slug, ... }, ...]` — bare array.
    Bare(Vec<AppsJsonRecord>),
    /// `{ "apps": [{ slug, ... }, ...] }` — wrapped form used by some
    /// historical exports of apps.json.
    Wrapped { apps: Vec<AppsJsonRecord> },
}

impl AppsJsonShape {
    fn into_records(self) -> Vec<AppsJsonRecord> {
        match self {
            Self::Bare(v) => v,
            Self::Wrapped { apps } => apps,
        }
    }
}

impl Registry {
    /// Build a registry from `apps.json` + `${src_root}/{slug}/src/flows/*.toml`.
    pub fn load(apps_json: &Path, src_root: &Path) -> DaemonResult<Self> {
        let body = std::fs::read(apps_json).map_err(|e| {
            DaemonError::Internal(format!(
                "read {}: {e}",
                apps_json.display()
            ))
        })?;
        let shape: AppsJsonShape = serde_json::from_slice(&body).map_err(|e| {
            DaemonError::Internal(format!(
                "parse {}: {e}",
                apps_json.display()
            ))
        })?;
        let records = shape.into_records();
        info!(apps = records.len(), path = %apps_json.display(), "registry: parsing apps.json");

        let mut apps = HashMap::with_capacity(records.len());
        let mut flows = HashMap::new();

        for rec in records {
            let entry = AppEntry {
                slug: rec.slug.clone(),
                callback_url: rec.flow_callback_url.clone(),
                callback_token: rec.flow_callback_token.clone(),
                max_concurrent_runs: rec.max_concurrent_runs.unwrap_or(0),
                circuit_breaker: CircuitState::default(),
            };
            // Scan flows for this slug. Sources are at
            // `${src_root}/{slug}/src/flows/*.toml`. If the directory is
            // missing the slug simply has no flows — not an error.
            let flow_dir = src_root.join(&rec.slug).join("src").join("flows");
            match scan_flow_dir(&flow_dir) {
                Ok(parsed) => {
                    debug!(slug = %rec.slug, count = parsed.len(), dir = %flow_dir.display(), "registry: flows scanned");
                    for def in parsed {
                        flows.insert((rec.slug.clone(), def.name.clone()), def);
                    }
                }
                Err(err) => {
                    warn!(slug = %rec.slug, dir = %flow_dir.display(), ?err, "registry: flow scan failed; slug has no flows in this snapshot");
                }
            }
            apps.insert(rec.slug.clone(), entry);
        }

        let registry = Self {
            apps,
            flows,
            loaded_at: Utc::now(),
        };
        info!(apps = registry.apps.len(), flows = registry.flows.len(), "registry: snapshot built");
        Ok(registry)
    }
}

/// Read every `*.toml` under `dir`, parse each in isolation, log + skip the
/// broken ones. Returns whatever parsed successfully.
fn scan_flow_dir(dir: &Path) -> DaemonResult<Vec<FlowDef>> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(DaemonError::Internal(format!(
                "read_dir {}: {e}",
                dir.display()
            )))
        }
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %path.display(), ?e, "flow file unreadable; skipping");
                continue;
            }
        };
        match parse_flow_toml(&body) {
            Ok(def) => out.push(def),
            Err(e) => {
                warn!(path = %path.display(), ?e, "flow TOML invalid; skipping");
            }
        }
    }
    Ok(out)
}
