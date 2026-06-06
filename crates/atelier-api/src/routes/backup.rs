use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tracing::instrument;

use atelier_backup::target::NewTarget;

use crate::state::ApiState;

/// Routes montées sous `/api/backup`.
pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/status", get(status))
        .route("/target", get(get_target).put(set_target))
        .route("/target/test", post(test_target))
        .route("/discover", post(discover))
        .route("/repo/password", get(reveal_password))
        .route("/run", post(run))
        .route("/run/{id}/cancel", post(cancel))
        .route("/runs", get(list_runs))
        .route("/runs/{id}", get(get_run))
}

fn err503() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "backup disabled (postgres unreachable)"})),
    )
        .into_response()
}

fn err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

#[instrument(skip(state))]
async fn status(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state.backup.status().await {
        Ok(s) => (StatusCode::OK, Json(json!(s))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[instrument(skip(state))]
async fn get_target(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state.backup.target().await {
        Ok(t) => (StatusCode::OK, Json(json!({ "target": t }))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[instrument(skip(state, body))]
async fn set_target(
    State(state): State<ApiState>,
    Json(body): Json<NewTarget>,
) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state.backup.set_target(&body).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Debug, Deserialize)]
struct DiscoverBody {
    host: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    domain: String,
}

#[instrument(skip(state, body))]
async fn discover(State(state): State<ApiState>, Json(body): Json<DiscoverBody>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state
        .backup
        .discover_shares(&body.host, &body.username, &body.password, &body.domain)
        .await
    {
        Ok(shares) => (StatusCode::OK, Json(json!({"shares": shares}))).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

#[instrument(skip(state))]
async fn test_target(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state.backup.test_target().await {
        Ok(report) => (StatusCode::OK, Json(json!({"ok": true, "report": report}))).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

#[instrument(skip(state))]
async fn reveal_password(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    // Volontairement non journalisé avec la valeur (cf. logging rules).
    match state.backup.reveal_restic_password().await {
        Ok(Some(pw)) => (StatusCode::OK, Json(json!({"password": pw}))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "dépôt non encore initialisé (lancez une première sauvegarde)"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[instrument(skip(state))]
async fn run(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state.backup.run_now("manual").await {
        Ok(run_id) => (StatusCode::ACCEPTED, Json(json!({"ok": true, "run_id": run_id}))).into_response(),
        Err(e) if e.contains("déjà en cours") => err(StatusCode::CONFLICT, e),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

#[instrument(skip(state))]
async fn cancel(
    State(state): State<ApiState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    if state.backup.cancel_run(id) {
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run non actif")
    }
}

#[derive(Debug, Deserialize)]
struct RunsQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[instrument(skip(state, q))]
async fn list_runs(State(state): State<ApiState>, Query(q): Query<RunsQuery>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    let limit = q.limit.unwrap_or(50);
    let offset = q.offset.unwrap_or(0);
    match state.backup.list_runs(limit, offset).await {
        Ok((runs, total)) => (StatusCode::OK, Json(json!({"runs": runs, "total": total}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[instrument(skip(state))]
async fn get_run(State(state): State<ApiState>, Path(id): Path<uuid::Uuid>) -> impl IntoResponse {
    if !state.backup.is_enabled() {
        return err503();
    }
    match state.backup.get_run(id).await {
        // `run` embarque déjà ses snapshots (champ `snapshots`).
        Ok(Some(run)) => (StatusCode::OK, Json(json!({"run": run}))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "run introuvable"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}
