use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;
use tracing::{instrument, warn};

use atelier_logging::{LogQuery, RawIngestEntry};

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(get_logs))
        .route("/stats", get(get_stats))
        .route("/ingest", post(ingest_logs))
        .route("/by-request/{rid}", get(by_request_id))
}

#[instrument(skip(state, q))]
async fn get_logs(State(state): State<ApiState>, Query(q): Query<LogQuery>) -> impl IntoResponse {
    match state.logs.query(&q).await {
        Ok(logs) => (
            StatusCode::OK,
            Json(json!({ "logs": logs, "total": logs.len() })),
        )
            .into_response(),
        Err(err) => {
            warn!(?err, "logs query failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

#[instrument(skip(state, q))]
async fn get_stats(State(state): State<ApiState>, Query(q): Query<LogQuery>) -> impl IntoResponse {
    match state.logs.stats(&q).await {
        Ok(s) => (StatusCode::OK, Json(s)).into_response(),
        Err(err) => {
            warn!(?err, "logs stats failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
                .into_response()
        }
    }
}

#[instrument(skip(state, headers, entries))]
async fn ingest_logs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(entries): Json<Vec<RawIngestEntry>>,
) -> impl IntoResponse {
    let expected = match std::env::var("ATELIER_LOGS_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "log ingest disabled (ATELIER_LOGS_TOKEN unset)" })),
            )
                .into_response();
        }
    };
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let provided = auth.strip_prefix("Bearer ").unwrap_or("");
    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid bearer token" })),
        )
            .into_response();
    }

    if entries.len() > 1000 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "max 1000 entries per batch" })),
        )
            .into_response();
    }

    let count = state.logs.ingest_batch("unknown", entries).await;
    (StatusCode::OK, Json(json!({ "ok": true, "count": count }))).into_response()
}

#[instrument(skip(state))]
async fn by_request_id(
    State(state): State<ApiState>,
    Path(rid): Path<String>,
) -> impl IntoResponse {
    match state.logs.by_request(&rid).await {
        Ok(logs) => (
            StatusCode::OK,
            Json(json!({ "logs": logs, "total": logs.len() })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}
