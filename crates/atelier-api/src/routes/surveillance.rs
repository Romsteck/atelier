use std::collections::HashMap;

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
///   GET    /api/surveillance/overview
///   GET    /api/apps/:slug/findings
///   POST   /api/apps/:slug/findings/:id/dismiss
///   POST   /api/apps/:slug/findings/:id/resolve
///   POST   /api/apps/:slug/surveillance/run     -- P3+ (returns 501 for now)
///   GET    /api/apps/:slug/surveillance/runs
pub fn global_router() -> Router<ApiState> {
    Router::new().route("/", get(list_findings_global))
}

pub fn overview_router() -> Router<ApiState> {
    Router::new()
        .route("/overview", get(surveillance_overview))
        .route("/resolving", get(get_resolving))
        .route("/sweep", get(get_sweep).post(start_sweep))
        .route("/sweep/cancel", post(cancel_sweep))
        .route(
            "/sweep/schedule",
            get(get_sweep_schedule).put(put_sweep_schedule),
        )
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
        .route(
            "/{slug}/findings/{id}/delete",
            post(delete_finding),
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

/// Aggregated snapshot for the global dashboard: per app × kind, open-finding
/// counts by severity + last run, plus global totals. One round-trip — the
/// detail (findings list, actions, live console) lives in the per-app Studio tab.
#[instrument(skip(state))]
async fn surveillance_overview(State(state): State<ApiState>) -> impl IntoResponse {
    let (Some(findings), Some(runs)) = (state.surveillance.findings(), state.surveillance.runs())
    else {
        return err503();
    };
    let counts = match findings.count_open_grouped().await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(?e, "overview: count_open_grouped failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    };
    let latest = match runs.latest_per_app_kind().await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(?e, "overview: latest_per_app_kind failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    };

    const SEVERITIES: [&str; 4] = ["critical", "high", "medium", "low"];
    let mut count_map: HashMap<(String, String), [i64; 4]> = HashMap::new();
    for row in counts {
        let Some(idx) = SEVERITIES.iter().position(|s| *s == row.severity) else {
            continue;
        };
        count_map.entry((row.slug, row.kind)).or_default()[idx] += row.count;
    }
    let mut run_map: HashMap<(String, String), atelier_watcher::Run> = HashMap::new();
    for run in latest {
        run_map.insert((run.slug.clone(), run.kind.clone()), run);
    }

    let kinds = [
        atelier_watcher::SECURITY_KIND,
        atelier_watcher::CODE_REVIEW_KIND,
        atelier_watcher::BIZ_KIND,
    ];
    let mut totals = [0i64; 4];
    let (mut running, mut failed) = (0u32, 0u32);
    let mut apps_json = Vec::new();
    for app in state.app_registry.list().await {
        let biz_scan = state.surveillance.scan_get(&app.slug).await;
        let mut app_total = 0i64;
        let mut kinds_json = Vec::new();
        for kind in kinds {
            let (label, blank) = if kind == atelier_watcher::BIZ_KIND {
                let blank = biz_scan.as_ref().map(|s| s.is_blank()).unwrap_or(true);
                let label = biz_scan
                    .as_ref()
                    .filter(|s| !s.label.is_empty())
                    .map(|s| s.label.clone())
                    .unwrap_or_else(|| "Business".to_string());
                (label, blank)
            } else if kind == atelier_watcher::SECURITY_KIND {
                ("Sécurité".to_string(), false)
            } else {
                ("Qualité".to_string(), false)
            };
            let open = count_map
                .get(&(app.slug.clone(), kind.to_string()))
                .copied()
                .unwrap_or_default();
            let open_total: i64 = open.iter().sum();
            app_total += open_total;
            for (i, n) in open.iter().enumerate() {
                totals[i] += n;
            }
            let last_run = run_map.get(&(app.slug.clone(), kind.to_string())).map(|r| {
                match r.status.as_str() {
                    "running" => running += 1,
                    "failed" => failed += 1,
                    _ => {}
                }
                json!({
                    "id": r.id,
                    "status": r.status,
                    "trigger": r.trigger,
                    "started_at": r.started_at,
                    "finished_at": r.finished_at,
                    "findings_count": r.findings_count,
                    "error": r.error,
                })
            });
            kinds_json.push(json!({
                "kind": kind,
                "label": label,
                "blank": blank,
                "open": SEVERITIES.iter().zip(open).collect::<HashMap<_, _>>(),
                "open_total": open_total,
                "last_run": last_run,
            }));
        }
        apps_json.push(json!({
            "slug": app.slug,
            "name": app.name,
            "open_total": app_total,
            "kinds": kinds_json,
        }));
    }

    let apps_count = apps_json.len();
    let body = json!({
        "apps": apps_json,
        "totals": {
            "open": SEVERITIES.iter().zip(totals).collect::<HashMap<_, _>>(),
            "open_total": totals.iter().sum::<i64>(),
            "apps": apps_count,
            "running": running,
            "failed": failed,
        },
    });
    (StatusCode::OK, Json(body)).into_response()
}

/// Findings with an OPEN resolution conversation right now (across all apps),
/// derived from `agent_open_tabs` (conversation tabs carrying a `fid`). Lets the
/// global surveillance page flag apps/findings being resolved and gate the sweep.
/// Enriched with kind/title/severity from the findings table when still present.
#[instrument(skip(state))]
async fn get_resolving(State(state): State<ApiState>) -> impl IntoResponse {
    let pairs = state.open_tabs.resolving_pairs().await;
    let findings = state.surveillance.findings();
    let mut out = Vec::with_capacity(pairs.len());
    for (slug, fid) in pairs {
        let enriched = match findings {
            Some(store) => store.get(fid).await.ok().flatten(),
            None => None,
        };
        match enriched {
            Some(f) => out.push(json!({
                "slug": f.slug, "finding_id": f.id, "kind": f.kind,
                "title": f.title, "severity": f.severity, "status": f.status,
            })),
            None => out.push(json!({ "slug": slug, "finding_id": fid })),
        }
    }
    (StatusCode::OK, Json(json!({ "resolving": out }))).into_response()
}

// ── Automatic sweep (manual + scheduled) ──────────────────────────────────

/// Current sweep state for page-load hydration (Idle when no sweep ever ran).
#[instrument(skip(state))]
async fn get_sweep(State(state): State<ApiState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({ "sweep": state.surveillance.sweep_snapshot() })),
    )
        .into_response()
}

/// Start the automatic sweep (app-by-app, 3 scans each, forced). 409 if a sweep
/// is already running (single-flight).
#[instrument(skip(state))]
async fn start_sweep(State(state): State<ApiState>) -> impl IntoResponse {
    if state.surveillance.findings().is_none() {
        return err503();
    }
    match state.surveillance.start_sweep("manual") {
        Ok(snap) => (
            StatusCode::ACCEPTED,
            Json(json!({ "ok": true, "sweep": snap })),
        )
            .into_response(),
        Err(e) if e == "sweep already running" => {
            (StatusCode::CONFLICT, Json(json!({ "error": e }))).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[instrument(skip(state))]
async fn cancel_sweep(State(state): State<ApiState>) -> impl IntoResponse {
    if state.surveillance.cancel_sweep() {
        (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "no sweep running")
    }
}

#[instrument(skip(state))]
async fn get_sweep_schedule(State(state): State<ApiState>) -> impl IntoResponse {
    match state.surveillance.sweep_schedule_get().await {
        Some(Ok(cfg)) => (StatusCode::OK, Json(json!({ "schedule": cfg }))).into_response(),
        Some(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        None => err503(),
    }
}

#[derive(Debug, Deserialize)]
struct ScheduleBody {
    enabled: bool,
    hour: i32,
    #[serde(default)]
    cadence: Option<String>,
}

#[instrument(skip(state, body))]
async fn put_sweep_schedule(
    State(state): State<ApiState>,
    Json(body): Json<ScheduleBody>,
) -> impl IntoResponse {
    let cadence = body.cadence.as_deref().unwrap_or("daily");
    match state
        .surveillance
        .sweep_schedule_set(body.enabled, body.hour, cadence)
        .await
    {
        Ok(cfg) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "schedule": cfg })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
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
    // fingerprint, so the scan-agent évite de re-suggérer le même pattern.
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

/// HARD-delete a finding (human delete button in the Studio). Looks up the
/// finding to enforce slug ownership and to derive its `kind` for the scoped
/// delete. Irreversible — distinct from dismiss/resolve which keep the row.
#[instrument(skip(state))]
async fn delete_finding(
    State(state): State<ApiState>,
    Path((slug, id)): Path<(String, i64)>,
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
    match findings.delete(id, &slug, &item.kind).await {
        Ok(Some(_)) => {
            state.surveillance.emit("finding", &slug, "delete");
            (StatusCode::OK, Json(json!({"ok": true}))).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "finding not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize, Default)]
struct RunBody {
    /// Which scan to run: `security` | `code_review` | `business`.
    kind: Option<String>,
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
    let Some(kind) = body.kind.as_deref() else {
        return err(StatusCode::BAD_REQUEST, "kind is required".to_string());
    };
    if !atelier_watcher::is_valid_kind(kind) {
        return err(
            StatusCode::BAD_REQUEST,
            format!("kind must be security|code_review|business (got '{kind}')"),
        );
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
    // Only the data-gated business scan needs a freshness watermark; compute it by
    // running its gate_sql (the REST layer has dataverse access; the watcher does
    // not). security/code_review are git-diff-gated → no watermark.
    let data_watermark = if kind == atelier_watcher::BIZ_KIND {
        match state.surveillance.scan_get(&slug).await {
            Some(scan) if scan.gate == atelier_watcher::Gate::Data => match scan.gate_sql.as_deref()
            {
                Some(sql) => data_watermark(&state, &slug, sql).await,
                None => None,
            },
            _ => None,
        }
    } else {
        None
    };
    // Fire-and-forget: spawns the scan-agent async, returns the run id immediately.
    match state
        .surveillance
        .run_now(slug.clone(), kind, trigger, data_watermark)
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
