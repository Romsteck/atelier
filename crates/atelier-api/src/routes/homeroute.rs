//! HTTP surface for the Homeroute reverse-proxy integration (`/api/homeroute/*`).
//!
//! Thin handlers over [`HomerouteService`]; all the orchestration (settings,
//! upsert-by-subdomain, live reconcile) lives in the service. Returns 503 when
//! the control-plane Postgres is down — mirrors `routes/backup.rs`.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;
use tracing::instrument;

use atelier_common::homeroute::NewSettings;

use crate::clients::homeroute_service::{AssignBody, HrServiceError};
use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/settings", get(get_settings).put(set_settings))
        .route("/test", post(test))
        .route("/register", post(register))
        .route("/app-routes", get(list_app_routes))
        .route(
            "/app-routes/{slug}",
            post(assign_route).delete(remove_route),
        )
        .route("/app-routes/{slug}/toggle", post(toggle_route))
}

fn err503() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "homeroute integration disabled (postgres unreachable)"})),
    )
        .into_response()
}

/// Map a service error to its HTTP response.
fn svc_err(e: HrServiceError) -> axum::response::Response {
    let status = StatusCode::from_u16(e.code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(json!({"error": e.to_string()}))).into_response()
}

#[instrument(skip(state))]
async fn get_settings(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.settings().await {
        Ok(s) => (StatusCode::OK, Json(json!({ "settings": s }))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state, body))]
async fn set_settings(
    State(state): State<ApiState>,
    Json(body): Json<NewSettings>,
) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.set_settings(&body).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state))]
async fn test(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.test().await {
        Ok(r) => (StatusCode::OK, Json(json!(r))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state))]
async fn register(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.register().await {
        Ok(s) => (StatusCode::OK, Json(json!({ "status": s }))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state))]
async fn list_app_routes(State(state): State<ApiState>) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.list_app_routes().await {
        Ok(r) => (StatusCode::OK, Json(json!(r))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state, body))]
async fn assign_route(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    body: Option<Json<AssignBody>>,
) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    let body = body.map(|Json(b)| b).unwrap_or_default();
    match state.homeroute.assign(&slug, body).await {
        Ok(route) => (StatusCode::OK, Json(json!({ "route": route }))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state))]
async fn remove_route(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.remove(&slug).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(e) => svc_err(e),
    }
}

#[instrument(skip(state))]
async fn toggle_route(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !state.homeroute.is_available() {
        return err503();
    }
    match state.homeroute.toggle(&slug).await {
        Ok(()) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(e) => svc_err(e),
    }
}
