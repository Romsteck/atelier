//! `GET /v1/definitions?slug=` — list flow defs known to the daemon for a slug.
//!
//! Same shape as the Atelier API viewer's `/api/apps/:slug/flows`, computed
//! from the in-memory registry (zero filesystem hits per request).

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::error::DaemonResult;
use crate::state::DaemonState;

#[derive(Debug, Deserialize)]
pub struct DefsQuery {
    pub slug: String,
}

#[derive(Debug, Serialize)]
pub struct DefsResponse {
    pub flows: Vec<DefSummary>,
}

#[derive(Debug, Serialize)]
pub struct DefSummary {
    pub name: String,
    pub description: Option<String>,
    pub step_count: usize,
}

#[instrument(skip(state), fields(slug = %q.slug))]
pub async fn list(
    State(state): State<Arc<DaemonState>>,
    Query(q): Query<DefsQuery>,
) -> DaemonResult<Json<DefsResponse>> {
    let registry = state.registry.load_full();
    let mut flows: Vec<DefSummary> = registry
        .flows
        .iter()
        .filter(|((slug, _), _)| slug == &q.slug)
        .map(|((_, _), def)| DefSummary {
            name: def.name.clone(),
            description: def.description.clone(),
            step_count: def.steps.len(),
        })
        .collect();
    flows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(DefsResponse { flows }))
}
