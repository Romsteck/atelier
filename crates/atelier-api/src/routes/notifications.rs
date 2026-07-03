//! Notifications plateforme — surface HTTP du store `platform_notifications`.
//!
//! Canal **agent → utilisateur** : les entrées sont produites par le tool MCP
//! `notify_user` (kind=notice), le journal automatique des mutations MCP des
//! agents projet (kind=action) et la plateforme (source=system). Ces endpoints
//! ne servent que la CONSOMMATION côté UI (cloche + tiroir des deux builds) ;
//! le live passe par le WS `notify:event` (created + mutations read/deleted).
//!
//! Endpoints (non authentifiés, confiance LAN comme les siblings issues/ship) :
//!   GET    /api/notifications            ?unread=1&slug=<s>&limit=<n>  → {items, unread}
//!   POST   /api/notifications/read-all
//!   POST   /api/notifications/{id}/read
//!   DELETE /api/notifications/{id}

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, instrument, warn};

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_notifications))
        .route("/read-all", post(read_all))
        .route("/{id}/read", post(read_one))
        .route("/{id}", delete(delete_notification))
}

fn ok(data: Value) -> axum::response::Response {
    Json(json!({"success": true, "data": data})).into_response()
}

fn fail(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"success": false, "error": msg.into()}))).into_response()
}

#[derive(Deserialize, Default)]
struct ListQuery {
    unread: Option<u8>,
    slug: Option<String>,
    limit: Option<i64>,
}

/// `GET /api/notifications` — liste récente (défaut 100) + compte unread
/// GLOBAL (badge, indépendant des filtres).
#[instrument(skip_all)]
async fn list_notifications(
    State(state): State<ApiState>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let (items, unread) = state
        .notifications
        .list(q.unread == Some(1), q.slug.as_deref(), q.limit.unwrap_or(100))
        .await;
    ok(json!({ "items": items, "unread": unread }))
}

/// `POST /api/notifications/{id}/read` — idempotent (déjà lue → 200).
#[instrument(skip_all)]
async fn read_one(State(state): State<ApiState>, Path(id): Path<String>) -> impl IntoResponse {
    match state.notifications.mark_read(&id).await {
        Ok(Some(entry)) => {
            info!(id = %id, "NotificationRead");
            ok(entry)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "notification introuvable"),
        Err(e) => {
            warn!(id = %id, err = %e, "notification mark_read failed");
            crate::routes::internal_err("mark notification read", e)
        }
    }
}

/// `POST /api/notifications/read-all`
#[instrument(skip_all)]
async fn read_all(State(state): State<ApiState>) -> impl IntoResponse {
    match state.notifications.mark_all_read().await {
        Ok(n) => {
            info!(read = n, "NotificationReadAll");
            ok(json!({ "read": n }))
        }
        Err(e) => {
            warn!(err = %e, "notification mark_all_read failed");
            crate::routes::internal_err("mark all notifications read", e)
        }
    }
}

/// `DELETE /api/notifications/{id}`
#[instrument(skip_all)]
async fn delete_notification(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.notifications.delete(&id).await {
        Ok(true) => {
            info!(id = %id, "NotificationDelete");
            ok(json!({ "deleted": true }))
        }
        Ok(false) => fail(StatusCode::NOT_FOUND, "notification introuvable"),
        Err(e) => {
            warn!(id = %id, err = %e, "notification delete failed");
            crate::routes::internal_err("delete notification", e)
        }
    }
}
