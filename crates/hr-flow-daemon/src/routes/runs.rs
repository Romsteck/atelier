//! `/v1/runs*` routes.
//!
//! Phase 1.7 contract :
//!   POST /v1/runs                     { slug, flow_name, input } → RunResult
//!   GET  /v1/runs?slug=&flow_name=&limit=                        → RunDoc[]
//!   GET  /v1/runs/{run_id}?slug=                                  → RunDoc
//!   POST /v1/runs/{run_id}/replay?slug=                           → RunResult
//!   POST /v1/runs/{run_id}/cancel?slug=                           → { cancelled: bool }
//!
//! Reads delegate to `JsonRunStore` rooted at `${runtime_root}/{slug}/runs/`,
//! same files the Atelier API viewer uses (single source of truth).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use hr_flow::{JsonRunStore, RunDoc, RunResult, RunStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, instrument};

use crate::engine_factory::{build_engine_for_flow, EngineFactoryInput};
use crate::error::{DaemonError, DaemonResult};
use crate::state::DaemonState;
use crate::supervisor::dispatch_run;

#[derive(Debug, Deserialize)]
pub struct RunRequest {
    pub slug: String,
    pub flow_name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunResponse {
    pub run_id: String,
    pub flow_name: String,
    pub status: String,
    pub output: Option<Value>,
    pub error: Option<RunErrorWire>,
    pub duration_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct RunErrorWire {
    pub step_id: String,
    pub message: String,
}

impl From<RunResult> for RunResponse {
    fn from(r: RunResult) -> Self {
        Self {
            run_id: r.run_id,
            flow_name: r.flow_name,
            status: format!("{:?}", r.status).to_lowercase(),
            output: r.output,
            error: r.error.map(|e| RunErrorWire {
                step_id: e.step_id,
                message: e.message,
            }),
            duration_ms: r.duration_ms,
        }
    }
}

#[instrument(skip(state, body), fields(slug = %body.slug, flow_name = %body.flow_name))]
pub async fn start(
    State(state): State<Arc<DaemonState>>,
    Json(body): Json<RunRequest>,
) -> DaemonResult<Json<RunResponse>> {
    let trigger = body.trigger.as_deref().unwrap_or("manual").to_string();
    let result = dispatch_run(
        state.clone(),
        body.slug,
        body.flow_name,
        body.input,
        &trigger,
        state.http.clone(),
        state.callback_timeout_ms,
        state.run_timeout_ms,
    )
    .await?;
    Ok(Json(result.into()))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub slug: String,
    #[serde(default)]
    pub flow_name: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub runs: Vec<RunDoc>,
}

#[instrument(skip(state), fields(slug = %q.slug))]
pub async fn list(
    State(state): State<Arc<DaemonState>>,
    Query(q): Query<ListQuery>,
) -> DaemonResult<Json<ListResponse>> {
    let store = open_store(&state, &q.slug)?;
    let runs = store
        .list(q.flow_name.as_deref(), q.limit)
        .await
        .map_err(DaemonError::Flow)?;
    Ok(Json(ListResponse { runs }))
}

#[derive(Debug, Deserialize)]
pub struct SlugQuery {
    pub slug: String,
}

#[derive(Debug, Serialize)]
pub struct GetResponse {
    pub run: RunDoc,
}

#[instrument(skip(state), fields(slug = %q.slug, run_id = %run_id))]
pub async fn get(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
    Query(q): Query<SlugQuery>,
) -> DaemonResult<Json<GetResponse>> {
    let store = open_store(&state, &q.slug)?;
    let run = store.load(&run_id).await.map_err(DaemonError::Flow)?;
    Ok(Json(GetResponse { run }))
}

#[instrument(skip(state), fields(slug = %q.slug, run_id = %run_id))]
pub async fn replay(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
    Query(q): Query<SlugQuery>,
) -> DaemonResult<Json<RunResponse>> {
    let store = open_store(&state, &q.slug)?;
    let doc = store.load(&run_id).await.map_err(DaemonError::Flow)?;
    info!(slug = %q.slug, flow = %doc.flow_name, source_run = %run_id, "replay: dispatching");
    let result = dispatch_run(
        state.clone(),
        q.slug,
        doc.flow_name,
        doc.input,
        "replay",
        state.http.clone(),
        state.callback_timeout_ms,
        state.run_timeout_ms,
    )
    .await?;
    Ok(Json(result.into()))
}

/// Run cancellation is not implemented: the engine assigns the run_id
/// internally, so a caller-supplied run_id cannot be mapped to an in-flight
/// dispatch, and nothing currently consults a cancellation signal. The route
/// is kept so clients get an honest `501` instead of a silent no-op `200`.
#[instrument(skip(_state), fields(run_id = %run_id))]
pub async fn cancel(
    State(_state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
    Query(_q): Query<SlugQuery>,
) -> (StatusCode, Json<Value>) {
    let _ = run_id;
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "cancelled": false,
            "error": "run cancellation is not implemented",
            "code": "not_implemented",
        })),
    )
}

fn open_store(state: &DaemonState, slug: &str) -> DaemonResult<JsonRunStore> {
    let dir = state.apps_runtime_root.join(slug).join("runs");
    JsonRunStore::new(&dir).map_err(DaemonError::Flow)
}

// ───── Used by definitions.rs to avoid duplicating the engine factory ─────
#[allow(dead_code)]
pub(crate) fn _engine_factory_input_marker(args: EngineFactoryInput<'_>) {
    let _ = build_engine_for_flow(args);
}
