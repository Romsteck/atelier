//! Read-only Apps routes (Phase 9 prep).
//!
//! Atelier consomme un mirror rsync de `/opt/homeroute/data/apps.json` et
//! `port-registry.json` depuis Medion (toutes les 2 min).
//! Les routes ci-dessous lisent ce fichier et exposent les mêmes shapes
//! que homeroute (`/api/apps`, `/api/apps/{slug}`).
//!
//! Mutations (create / update / delete / control / build / deploy / exec /
//! env update / regenerate-context) restent côté homeroute (Medion) car
//! elles déclenchent des actions dans hr-orchestrator (process supervisor).
//! Atelier link-out vers proxy.mynetwk.biz pour ces opérations jusqu'au
//! cutover Phase 9.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;
use tracing::warn;

use crate::state::ApiState;

/// Custom serializer that mirrors hr-ipc::types::ApplicationDto formatting
/// (rfc3339 with explicit +00:00 offset, not the default chrono `Z` shortcut).
fn serialize_rfc3339<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&dt.to_rfc3339())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Application {
    pub slug: String,
    pub name: String,
    pub stack: String,
    pub has_db: bool,
    pub visibility: String,
    pub domain: String,
    pub port: u16,
    pub run_command: String,
    pub build_command: String,
    pub build_artefact: String,
    pub health_path: String,
    pub env_vars: BTreeMap<String, String>,
    pub state: String,
    #[serde(default)]
    pub sources_on: Option<String>,
    #[serde(default)]
    pub db_backend: Option<String>,
    #[serde(serialize_with = "serialize_rfc3339")]
    pub created_at: DateTime<Utc>,
    #[serde(serialize_with = "serialize_rfc3339")]
    pub updated_at: DateTime<Utc>,
}

fn read_apps(state: &ApiState) -> Result<Vec<Application>, String> {
    let path = state.apps_state_dir.join("apps.json");
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse apps.json: {e}"))
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_apps))
        .route("/{slug}", get(get_app))
        .route("/{slug}/env", get(get_app_env))
}

async fn list_apps(State(state): State<ApiState>) -> impl IntoResponse {
    match read_apps(&state) {
        Ok(apps) => Json(json!({"success": true, "data": {"apps": apps}})).into_response(),
        Err(e) => {
            warn!(error = %e, "list_apps failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": e})),
            )
                .into_response()
        }
    }
}

async fn get_app(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    let apps = match read_apps(&state) {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": e})),
            )
                .into_response();
        }
    };
    match apps.into_iter().find(|a| a.slug == slug) {
        Some(app) => Json(json!({"success": true, "data": app})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "App not found"})),
        )
            .into_response(),
    }
}

async fn get_app_env(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    let apps = match read_apps(&state) {
        Ok(a) => a,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"success": false, "error": e})),
            )
                .into_response();
        }
    };
    match apps.into_iter().find(|a| a.slug == slug) {
        Some(app) => Json(json!({"success": true, "data": app.env_vars})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "App not found"})),
        )
            .into_response(),
    }
}
