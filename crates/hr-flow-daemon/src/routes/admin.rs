//! `POST /v1/_admin/reload[?slug=]` — hot-reload the registry.
//!
//! Atomic via `ArcSwap::store(new)`. In-flight runs keep their captured
//! `Arc<Registry>` for their entire lifetime — no STW.
//!
//! `slug=` is accepted for forward-compat but currently the daemon always
//! reloads the full registry; per-slug deltas are a Phase 7 optimisation.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};

use crate::error::{DaemonError, DaemonResult};
use crate::registry::Registry;
use crate::state::DaemonState;

#[derive(Debug, Deserialize)]
pub struct ReloadQuery {
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReloadResponse {
    pub apps_loaded: usize,
    pub flows_loaded: usize,
}

#[instrument(skip(state))]
pub async fn reload(
    State(state): State<Arc<DaemonState>>,
    Query(_q): Query<ReloadQuery>,
) -> DaemonResult<Json<ReloadResponse>> {
    let new = Registry::load(&state.apps_json_path, &state.apps_src_root)
        .map_err(|e| DaemonError::Internal(format!("registry reload: {e}")))?;
    let apps_loaded = new.apps.len();
    let flows_loaded = new.flows.len();
    state.registry.store(Arc::new(new));
    info!(apps_loaded, flows_loaded, "admin: registry reloaded");
    Ok(Json(ReloadResponse {
        apps_loaded,
        flows_loaded,
    }))
}
