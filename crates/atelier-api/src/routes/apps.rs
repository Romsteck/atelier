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
//! - GET    /api/apps/{slug}/todos
//! - POST   /api/apps/{slug}/control  body {action: start|stop|restart}
//! - GET    /api/apps/{slug}/status   process state (pid, uptime, port)
//! - POST   /api/apps/{slug}/ship     body {timeout_secs?: u64} (wrapper MCP `app.ship`)
//!
//! TODO Phase 9.2 suite : create / update / delete / build / deploy / exec /
//! env update / regenerate-context / logs.

use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::mcp::apps_ops::AppsContext;
use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_apps))
        .route("/{slug}", get(get_app))
        .route("/{slug}/env", get(get_app_env))
        .route("/{slug}/todos", get(get_app_todos))
        .route("/{slug}/control", post(control_app))
        .route("/{slug}/status", get(app_status))
        .route("/{slug}/ship", post(ship_app))
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

async fn get_app_todos(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    match state.todos_manager.list(&slug, None).await {
        Ok(todos) => Json(json!({"success": true, "data": {"todos": todos}})).into_response(),
        Err(e) => {
            warn!(slug = %slug, error = %e, "get_app_todos failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": format!("{e}")})),
            )
                .into_response()
        }
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

#[derive(Deserialize, Default)]
struct ShipBody {
    timeout_secs: Option<u64>,
}

/// `POST /api/apps/{slug}/ship`
///
/// Thin HTTP wrapper around the MCP `app.ship` op. Stops the supervised
/// process, rsyncs pre-built artefacts from CloudMaster (`/opt/homeroute/apps/<slug>/src/`)
/// to Medion (`/var/lib/atelier/apps/<slug>/src/`), and restarts.
///
/// Body: `{"timeout_secs": u64}` (optional, clamped 60..=7200, default 900).
async fn ship_app(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    body: Option<Json<ShipBody>>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let timeout_secs = body.and_then(|Json(b)| b.timeout_secs);
    info!(slug = %slug, timeout_secs = ?timeout_secs, "AppShip");

    let (app_build_tx, _) = tokio::sync::broadcast::channel(256);
    let ctx = AppsContext {
        supervisor: (*state.supervisor).clone(),
        db_manager: (*state.db_manager).clone(),
        dataverse_manager: state.dv.clone(),
        todos: (*state.todos_manager).clone(),
        context_generator: state.context_generator.clone(),
        edge: None,
        git: state.git.clone(),
        base_domain: state.context_generator.base_domain.clone(),
        build_locks: state.build_locks.clone(),
        app_build_tx,
    };

    let resp = ctx.ship(slug.clone(), timeout_secs).await;
    // ship()'s pipeline returns ok_data even on a pipeline failure (rsync /
    // start errors are stuffed into AppExecResult.exit_code). Inspect that so
    // the HTTP envelope never reports success while the app is actually down.
    let exit_code = resp
        .data
        .as_ref()
        .and_then(|d| d.get("exit_code"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if resp.ok && exit_code == 0 {
        Json(json!({
            "success": true,
            "data": resp.data.unwrap_or(json!({"ok": true})),
        }))
        .into_response()
    } else {
        let err_msg = resp.error.unwrap_or_else(|| {
            "ship pipeline failed — app may be down, check atelier logs".to_string()
        });
        let status = if err_msg.starts_with("BUILD_BUSY") {
            StatusCode::CONFLICT
        } else if err_msg.starts_with("app not found") {
            StatusCode::NOT_FOUND
        } else if err_msg.starts_with("invalid slug") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        warn!(slug = %slug, error = %err_msg, "AppShip failed");
        (status, Json(json!({"success": false, "error": err_msg}))).into_response()
    }
}

