use std::sync::Arc;

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::state::DaemonState;

/// `GET /v1/health` — unauthenticated. Used by systemd healthcheck and the
/// Atelier API liveness probe.
pub async fn handler(State(state): State<Arc<DaemonState>>) -> Json<Value> {
    let registry = state.registry.load();
    let uptime_s = (chrono::Utc::now() - state.started_at).num_seconds().max(0) as u64;
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": uptime_s,
        "registry_apps": registry.apps.len(),
        "registry_flows": registry.flows.len(),
        "registry_loaded_at": registry.loaded_at.to_rfc3339(),
        "active_runs": state.runs.len(),
    }))
}
