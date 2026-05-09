//! Apps routes — Phase 9 (cutover en cours).
//!
//! Lecture **et** écriture sur `state.app_registry` + `state.supervisor`.
//! Atelier devient le canonical writer post-cutover. Les routes appellent
//! directement la lib `hr-apps` (pas d'IPC orchestrator côté CloudMaster).
//!
//! Endpoints exposés :
//! - GET    /api/apps            (list)
//! - GET    /api/apps/{slug}     (single)
//! - GET    /api/apps/{slug}/env
//! - POST   /api/apps/{slug}/control  body {action: start|stop|restart}
//! - GET    /api/apps/{slug}/status   process state (pid, uptime, port)
//!
//! TODO Phase 9.2 suite : create / update / delete / build / deploy / exec /
//! env update / regenerate-context / logs / todos.

use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_apps))
        .route("/{slug}", get(get_app))
        .route("/{slug}/env", get(get_app_env))
        .route("/{slug}/control", post(control_app))
        .route("/{slug}/status", get(app_status))
        .route("/{slug}/regenerate_flow_token", post(regenerate_flow_token))
}

fn validate_slug(slug: &str) -> Result<(), axum::response::Response> {
    if hr_apps::valid_slug(slug) {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "success": false,
                "error": "slug must match ^[a-z][a-z0-9-]*$ (max 64 chars)"
            })),
        )
            .into_response())
    }
}

async fn list_apps(State(state): State<ApiState>) -> impl IntoResponse {
    let started = Instant::now();
    let apps = state.app_registry.list().await;
    let value = serde_json::to_value(&apps).unwrap_or_else(|_| Value::Array(vec![]));
    info!(
        count = apps.len(),
        duration_ms = started.elapsed().as_millis() as u64,
        "list_apps"
    );
    Json(json!({"success": true, "data": {"apps": value}})).into_response()
}

async fn get_app(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    match state.app_registry.get(&slug).await {
        Some(app) => {
            let value = serde_json::to_value(&app).unwrap_or(Value::Null);
            Json(json!({"success": true, "data": value})).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "App not found"})),
        )
            .into_response(),
    }
}

async fn get_app_env(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    match state.app_registry.get(&slug).await {
        Some(app) => Json(json!({"success": true, "data": app.env_vars})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "App not found"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ControlBody {
    action: String,
}

async fn control_app(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<ControlBody>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    info!(slug = %slug, action = %body.action, "AppControl");
    let result = match body.action.as_str() {
        "start" => state.supervisor.start(&slug).await,
        "stop" => state.supervisor.stop(&slug).await,
        "restart" => state.supervisor.restart(&slug).await,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": format!("invalid action '{other}' (use start|stop|restart)")
                })),
            )
                .into_response();
        }
    };
    match result {
        Ok(()) => Json(json!({"success": true, "data": {"slug": slug, "action": body.action}}))
            .into_response(),
        Err(e) => {
            warn!(slug = %slug, action = %body.action, error = %e, "AppControl failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": format!("{e}")})),
            )
                .into_response()
        }
    }
}

async fn app_status(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    match state.supervisor.status(&slug).await {
        Some(status) => {
            // ProcessStatus n'est pas Serialize → on aplatit à la main pour
            // matcher la shape AppStatusData exposée par homeroute.
            Json(json!({
                "success": true,
                "data": {
                    "slug": slug,
                    "pid": status.pid,
                    "state": status.state,
                    "port": status.port,
                    "uptime_secs": status.uptime_secs,
                    "restart_count": status.restart_count,
                }
            }))
            .into_response()
        }
        None => Json(json!({"success": true, "data": null})).into_response(),
    }
}

/// `POST /api/apps/{slug}/regenerate_flow_token`
///
/// Generates a fresh 32-byte hex token for callbacks daemon ↔ app and writes
/// it back into `apps.json` (`flow_callback_url` + `flow_callback_token`).
/// Sets `flow_callback_url=http://127.0.0.1:<port>` from the app's port.
///
/// Returns the new token in the response body so the caller can persist it
/// in the app's `.env` (e.g. `HR_FLOW_TOKEN=...`). After this the daemon
/// registry must be reloaded — `POST /api/flows/_admin/reload?slug=<slug>`
/// (handled separately).
async fn regenerate_flow_token(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    use rand::RngCore;
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let mut app = match state.app_registry.get(&slug).await {
        Some(a) => a,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"success": false, "error": format!("app '{slug}' not found")})),
            )
                .into_response();
        }
    };

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let url = format!("http://127.0.0.1:{}", app.port);

    app.flow_callback_token = Some(token.clone());
    app.flow_callback_url = Some(url.clone());

    if let Err(e) = state.app_registry.upsert(app).await {
        warn!(slug = %slug, error = %e, "regenerate_flow_token: persist failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"success": false, "error": format!("persist apps.json: {e}")})),
        )
            .into_response();
    }

    info!(slug = %slug, "regenerate_flow_token: token rotated");
    Json(json!({
        "success": true,
        "data": {
            "slug": slug,
            "flow_callback_url": url,
            "flow_callback_token": token,
            "next_steps": [
                "Pose HR_FLOW_TOKEN=<token> dans le .env canonique de l'app (CloudMaster)",
                "POST /api/flows/_admin/reload?slug=<slug> pour recharger le registry du daemon",
                "make deploy-app SLUG=<slug> pour rsync le .env mis à jour vers Medion"
            ]
        }
    }))
    .into_response()
}
