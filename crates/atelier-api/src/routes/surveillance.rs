use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tracing::{instrument, warn};

use atelier_watcher::FindingFilter;

use crate::state::ApiState;

/// Routes mounted under `/api`. Exposed:
///   GET    /api/findings
///   GET    /api/apps/:slug/findings
///   POST   /api/apps/:slug/findings/:id/dismiss
///   POST   /api/apps/:slug/findings/:id/resolve
///   POST   /api/apps/:slug/surveillance/run     -- P3+ (returns 501 for now)
///   GET    /api/apps/:slug/surveillance/runs
pub fn global_router() -> Router<ApiState> {
    Router::new().route("/", get(list_findings_global))
}

pub fn app_router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/findings", get(list_findings_app))
        .route(
            "/{slug}/findings/{id}/dismiss",
            post(dismiss_finding),
        )
        .route(
            "/{slug}/findings/{id}/resolve",
            post(resolve_finding),
        )
        .route("/{slug}/surveillance/run", post(run_surveillance))
        .route(
            "/{slug}/surveillance/runs/{run_id}/cancel",
            post(cancel_run),
        )
        .route(
            "/{slug}/surveillance/runs/{run_id}/transcript",
            get(get_transcript),
        )
        .route("/{slug}/surveillance/runs", get(list_runs))
}

fn err503() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"error": "surveillance disabled (postgres unreachable)"})),
    )
        .into_response()
}

fn err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

#[instrument(skip(state, q))]
async fn list_findings_global(
    State(state): State<ApiState>,
    Query(q): Query<FindingFilter>,
) -> impl IntoResponse {
    let Some(store) = state.surveillance.findings() else {
        return err503();
    };
    match store.list(q).await {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({"findings": items, "total": items.len()})),
        )
            .into_response(),
        Err(e) => {
            warn!(?e, "list findings failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

#[instrument(skip(state, q))]
async fn list_findings_app(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(mut q): Query<FindingFilter>,
) -> impl IntoResponse {
    let Some(store) = state.surveillance.findings() else {
        return err503();
    };
    q.slug = Some(slug);
    match store.list(q).await {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({"findings": items, "total": items.len()})),
        )
            .into_response(),
        Err(e) => {
            warn!(?e, "list findings app failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

#[derive(Debug, Deserialize)]
struct DismissBody {
    #[serde(default)]
    reason: Option<String>,
}

#[instrument(skip(state, body))]
async fn dismiss_finding(
    State(state): State<ApiState>,
    Path((slug, id)): Path<(String, i64)>,
    Json(body): Json<DismissBody>,
) -> impl IntoResponse {
    let Some(findings) = state.surveillance.findings() else {
        return err503();
    };
    // Ownership check + persist a `dismissed_pattern` mémoire entry by
    // fingerprint, so Codex évite de re-suggérer le même pattern.
    let item = match findings.get(id).await {
        Ok(Some(f)) if f.slug == slug => f,
        Ok(Some(_)) => return err(StatusCode::NOT_FOUND, "slug mismatch"),
        Ok(None) => return err(StatusCode::NOT_FOUND, "finding not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match findings.dismiss(id).await {
        Ok(_) => {}
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    if let Some(memory) = state.surveillance.memory() {
        let value = json!({
            "fingerprint": item.fingerprint,
            "title": item.title,
            "reason": body.reason,
            "dismissed_at": chrono::Utc::now(),
        });
        if let Err(e) = memory
            .upsert(&slug, "dismissed_pattern", &item.fingerprint, &value, None)
            .await
        {
            warn!(?e, "dismissed_pattern memory upsert failed");
        }
    }
    state.surveillance.emit("finding", &slug, "dismiss");
    (StatusCode::OK, Json(json!({"ok": true}))).into_response()
}

#[derive(Debug, Deserialize)]
struct ResolveBody {
    #[serde(default)]
    commit_sha: Option<String>,
}

#[instrument(skip(state, body))]
async fn resolve_finding(
    State(state): State<ApiState>,
    Path((slug, id)): Path<(String, i64)>,
    Json(body): Json<ResolveBody>,
) -> impl IntoResponse {
    let Some(findings) = state.surveillance.findings() else {
        return err503();
    };
    let item = match findings.get(id).await {
        Ok(Some(f)) if f.slug == slug => f,
        Ok(Some(_)) => return err(StatusCode::NOT_FOUND, "slug mismatch"),
        Ok(None) => return err(StatusCode::NOT_FOUND, "finding not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    match findings.resolve(id, body.commit_sha.as_deref()).await {
        Ok(_) => {}
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
    if let Some(memory) = state.surveillance.memory() {
        let value = json!({
            "finding_id": id,
            "title": item.title,
            "commit_sha": body.commit_sha,
            "completed_at": chrono::Utc::now(),
        });
        let key = format!("finding:{}", id);
        if let Err(e) = memory.upsert(&slug, "applied_fix", &key, &value, None).await {
            warn!(?e, "applied_fix memory upsert failed");
        }
    }
    state.surveillance.emit("finding", &slug, "resolve");
    (StatusCode::OK, Json(json!({"ok": true}))).into_response()
}

#[derive(Debug, Deserialize)]
struct RunBody {
    kind: String,
}

#[instrument(skip(state, body))]
async fn run_surveillance(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<RunBody>,
) -> impl IntoResponse {
    if state.surveillance.findings().is_none() {
        return err503();
    }
    let Some(kind) = atelier_watcher::RunKind::from_str(&body.kind) else {
        return err(StatusCode::BAD_REQUEST, "kind must be code_review|suggestions|security");
    };
    // Fire-and-forget: spawns Codex async, returns the run id immediately.
    match state.surveillance.run_now(slug.clone(), kind, "manual").await {
        Ok(run_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"ok": true, "run_id": run_id, "slug": slug, "kind": body.kind})),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[instrument(skip(state))]
async fn cancel_run(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, uuid::Uuid)>,
) -> impl IntoResponse {
    if state.surveillance.findings().is_none() {
        return err503();
    }
    if state.surveillance.cancel_run(run_id) {
        (
            StatusCode::OK,
            Json(json!({"ok": true, "slug": slug, "run_id": run_id})),
        )
            .into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run not active")
    }
}

#[instrument(skip(state))]
async fn get_transcript(
    State(state): State<ApiState>,
    Path((_slug, run_id)): Path<(String, uuid::Uuid)>,
) -> impl IntoResponse {
    if state.surveillance.findings().is_none() {
        return err503();
    }
    let lines = state.surveillance.transcript(run_id);
    let total = lines.len();
    (StatusCode::OK, Json(json!({"lines": lines, "total": total}))).into_response()
}

#[derive(Debug, Deserialize)]
struct RunsQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[instrument(skip(state, q))]
async fn list_runs(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<RunsQuery>,
) -> impl IntoResponse {
    let Some(runs) = state.surveillance.runs() else {
        return err503();
    };
    let limit = q.limit.unwrap_or(50);
    match runs.list(Some(&slug), limit).await {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({"runs": items, "total": items.len()})),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
