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
        .route("/{slug}/surveillance/scan", get(get_scan))
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

#[derive(Debug, Deserialize, Default)]
struct RunBody {
    /// `manual` (default) or `cron` (a scheduled timer).
    #[serde(default)]
    trigger: Option<String>,
}

/// Run a data-gated scan's `gate_sql` (a read-only SELECT) and return its scalar
/// watermark (empty string when no rows). `None` only when the dataverse backend
/// is unreachable — the gate then can't evaluate and the run proceeds. Keeps the
/// app-specific SQL out of `atelier-watcher`, which stays dataverse-agnostic.
/// The user SQL is wrapped as a FROM-subquery so it cannot mutate (same guarantee
/// as `pm_query`); `scan_set` already validated it is SELECT-only.
async fn data_watermark(state: &ApiState, slug: &str, gate_sql: &str) -> Option<String> {
    use sqlx_core::row::Row;
    let mgr = state.dv.as_ref()?;
    let engine = mgr.engine_for(slug).await.ok()?;
    let inner = gate_sql.trim().trim_end_matches(';');
    let wrapped = format!(
        "SELECT (to_jsonb(t)->>(SELECT jsonb_object_keys(to_jsonb(t)) LIMIT 1))::text AS w \
         FROM ( {inner} ) t LIMIT 1"
    );
    match sqlx_core::query::query_with(
        sqlx_core::sql_str::AssertSqlSafe(wrapped.as_str()),
        sqlx_postgres::PgArguments::default(),
    )
    .fetch_optional(engine.pool())
    .await
    {
        Ok(Some(row)) => Some(row.try_get::<Option<String>, _>("w").ok().flatten().unwrap_or_default()),
        Ok(None) => Some(String::new()),
        Err(e) => {
            warn!(slug = %slug, ?e, "gate_sql watermark failed — running unconditionally");
            None
        }
    }
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
    let trigger = match body.trigger.as_deref() {
        None | Some("manual") => "manual",
        Some("cron") => "cron",
        Some(other) => {
            return err(
                StatusCode::BAD_REQUEST,
                format!("trigger must be manual|cron (got '{other}')"),
            );
        }
    };
    // For a data-gated scan, compute the freshness watermark by running its
    // gate_sql (the REST layer has dataverse access; the watcher does not).
    let data_watermark = match state.surveillance.scan_get(&slug).await {
        Some(scan) if scan.gate == atelier_watcher::Gate::Data => match scan.gate_sql.as_deref() {
            Some(sql) => data_watermark(&state, &slug, sql).await,
            None => None,
        },
        _ => None,
    };
    // Fire-and-forget: spawns Codex async, returns the run id immediately.
    match state
        .surveillance
        .run_now(slug.clone(), trigger, data_watermark)
        .await
    {
        Ok(run_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"ok": true, "run_id": run_id, "slug": slug})),
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

/// The app's single scan definition (label/prompt/cadence/gate/categories). The
/// UI reads this to render the scan by its agent-given name (or "en veille" when
/// blank). Returns `{scan: null}` when no row yet.
#[instrument(skip(state))]
async fn get_scan(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if state.surveillance.findings().is_none() {
        return err503();
    }
    let scan = state.surveillance.scan_get(&slug).await;
    let blank = scan.as_ref().map(|s| s.is_blank()).unwrap_or(true);
    (StatusCode::OK, Json(json!({ "scan": scan, "blank": blank }))).into_response()
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
