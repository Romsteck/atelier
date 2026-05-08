//! Read-only tasks routes (Phase 8).
//!
//! Atelier consomme un snapshot du `tasks.db` de Medion via sync-state.timer
//! (rsync .backup d'une SQLite WAL toutes les 2 min) et expose les mêmes
//! endpoints que homeroute hr-api. La mutation `POST /tasks/:id/cancel` est
//! refusée (503) — seul homeroute peut annuler une task active.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_tasks))
        .route("/active", get(get_active_tasks))
        .route("/{id}", get(get_task))
        .route("/{id}/cancel", post(cancel_task))
}

#[derive(Deserialize)]
struct ListParams {
    limit: Option<u32>,
    offset: Option<u32>,
    status: Option<String>,
}

async fn list_tasks(
    State(state): State<ApiState>,
    Query(params): Query<ListParams>,
) -> Json<serde_json::Value> {
    let limit = params.limit.unwrap_or(30).min(100);
    let offset = params.offset.unwrap_or(0);
    let (tasks, total) = state
        .task_store
        .list_tasks(limit, offset, params.status.as_deref())
        .await;
    Json(json!({ "tasks": tasks, "total": total }))
}

async fn get_active_tasks(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let tasks = state.task_store.get_active_tasks().await;
    Json(json!(tasks))
}

async fn get_task(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    match state.task_store.get_task(&id).await {
        Some(task) => {
            let steps = state.task_store.get_steps(&id).await;
            Json(json!({ "task": task, "steps": steps }))
        }
        None => Json(json!({ "error": "Task not found" })),
    }
}

async fn cancel_task(
    State(_state): State<ApiState>,
    Path(_id): Path<String>,
) -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "success": false,
            "error": "Atelier est read-only — annule la task depuis proxy.mynetwk.biz/tasks"
        })),
    )
}
