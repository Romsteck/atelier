//! Read-only REST routes for the per-app flow engine — Phase 5.
//!
//! Copie 1:1 de homeroute hr-api flows.rs, avec les helpers `flows_dir` et
//! `runs_dir` paramétrés via `ApiState` (CloudMaster lit les TOML directement
//! sur les sources canoniques `/opt/homeroute/apps/{slug}/src/flows/`, et les
//! runs depuis `/var/lib/atelier/apps/{slug}/runs/` synchronisés depuis Medion
//! par `atelier-sync-runs.timer`).
//!
//! Mutations (trigger, replay) restent côté MCP/homeroute. Atelier est viewer.
//!
//! ⚠ Pas de couplage au runtime hr-flow ici, à dessein : à terme le moteur
//! deviendra un daemon `hr-flowd` séparé (cf. peaceful-spinning-mountain.md).
//! On consomme uniquement `hr_flow::parse_flow_toml` (fonction pure) — le reste
//! n'est que lecture de fichiers + agrégation in-memory.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use tracing::{info, instrument, warn};

use crate::clients::{flowd::FlowdError, FlowdClient};
use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/apps/{slug}/flows", get(list_definitions))
        .route("/apps/{slug}/flows/{name}", get(get_definition))
        .route("/apps/{slug}/flows/{name}/run", post(run_flow))
        .route("/apps/{slug}/flows/_stats", get(get_app_stats))
        .route("/apps/{slug}/flows/_runs", get(list_runs))
        .route("/apps/{slug}/flows/_runs/{run_id}", get(get_run))
        .route(
            "/apps/{slug}/flows/_runs/{run_id}/replay",
            post(replay_run),
        )
        .route(
            "/apps/{slug}/flows/_runs/{run_id}/steps/{record_id}",
            get(get_run_step),
        )
        .route("/flows/_stats", get(get_global_stats))
        .route("/flows/_admin/reload", post(reload_daemon_registry))
}

#[derive(Debug, Deserialize)]
struct RunBody {
    #[serde(default)]
    input: Value,
}

#[instrument(skip(_state, body), fields(slug = %slug, name = %name))]
async fn run_flow(
    State(_state): State<ApiState>,
    Path((slug, name)): Path<(String, String)>,
    Json(body): Json<RunBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !valid_slug(&slug) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid slug"));
    }
    let client = FlowdClient::from_env()
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &format!("flowd unavailable: {e}")))?;
    let wire = client
        .run(&slug, &name, body.input)
        .await
        .map_err(|e| flowd_error_to_http(e))?;
    Ok(Json(json!({ "success": true, "run": wire })))
}

#[instrument(skip(_state), fields(slug = %slug, run_id = %run_id))]
async fn replay_run(
    State(_state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !valid_slug(&slug) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid slug"));
    }
    if run_id.contains('/') || run_id.contains('.') {
        return Err(err(StatusCode::BAD_REQUEST, "invalid run_id"));
    }
    let client = FlowdClient::from_env()
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &format!("flowd unavailable: {e}")))?;
    let wire = client
        .replay(&slug, &run_id)
        .await
        .map_err(|e| flowd_error_to_http(e))?;
    Ok(Json(json!({ "success": true, "run": wire })))
}

#[derive(Debug, Deserialize)]
struct ReloadQuery {
    #[serde(default)]
    slug: Option<String>,
}

#[instrument(skip(_state))]
async fn reload_daemon_registry(
    State(_state): State<ApiState>,
    Query(q): Query<ReloadQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let client = FlowdClient::from_env()
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &format!("flowd unavailable: {e}")))?;
    let report = client
        .reload(q.slug.as_deref())
        .await
        .map_err(|e| flowd_error_to_http(e))?;
    Ok(Json(json!({
        "success": true,
        "apps_loaded": report.apps_loaded,
        "flows_loaded": report.flows_loaded,
    })))
}

fn flowd_error_to_http(e: FlowdError) -> (StatusCode, Json<serde_json::Value>) {
    let (status, msg) = match &e {
        FlowdError::MissingToken => (StatusCode::SERVICE_UNAVAILABLE, "flowd token missing"),
        FlowdError::Transport(_) => (StatusCode::BAD_GATEWAY, "flowd transport"),
        FlowdError::Upstream { status, .. } => {
            // Forward 4xx but cap 5xx as 502 (we don't want to leak internal status codes raw).
            if status.is_client_error() {
                (*status, "flowd client error")
            } else {
                (StatusCode::BAD_GATEWAY, "flowd upstream error")
            }
        }
        FlowdError::Parse(_) => (StatusCode::BAD_GATEWAY, "flowd response parse"),
    };
    (
        status,
        Json(json!({ "success": false, "error": msg, "detail": e.to_string() })),
    )
}

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(json!({ "success": false, "error": msg })))
}

fn flows_dir(state: &ApiState, slug: &str) -> PathBuf {
    state.apps_src_root.join(slug).join("src").join("flows")
}

fn runs_dir(state: &ApiState, slug: &str) -> PathBuf {
    state.apps_runtime_root.join(slug).join("runs")
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Blocking directory scan of `*.toml` flow definitions. Run inside
/// `spawn_blocking` — never call directly from an async handler.
fn scan_flow_definitions(dir: &FsPath) -> Vec<Value> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut flows = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = hr_flow::parse_flow_toml(&body) else {
            continue;
        };
        flows.push(json!({
            "name": parsed.name,
            "description": parsed.description,
            "step_count": parsed.steps.len(),
            "file": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        }));
    }
    flows.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    flows
}

async fn list_definitions(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !valid_slug(&slug) {
        return err(StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    let dir = flows_dir(&state, &slug);
    let flows = tokio::task::spawn_blocking(move || scan_flow_definitions(&dir))
        .await
        .unwrap_or_default();
    Json(json!({ "success": true, "flows": flows })).into_response()
}

async fn get_definition(
    State(state): State<ApiState>,
    Path((slug, name)): Path<(String, String)>,
) -> impl IntoResponse {
    if !valid_slug(&slug) {
        return err(StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    if name.contains('/') || name.contains('.') {
        return err(StatusCode::BAD_REQUEST, "Invalid name").into_response();
    }
    let path = flows_dir(&state, &slug).join(format!("{name}.toml"));
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(b) => b,
        Err(_) => return err(StatusCode::NOT_FOUND, "Flow not found").into_response(),
    };
    let parsed = match hr_flow::parse_flow_toml(&body) {
        Ok(p) => p,
        Err(e) => {
            warn!(slug = %slug, name = %name, error = %e, "flow definition is malformed");
            return err(StatusCode::UNPROCESSABLE_ENTITY, "Invalid flow definition")
                .into_response();
        }
    };
    Json(json!({
        "success": true,
        "definition": parsed,
        "source": body,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct RunsQuery {
    #[serde(default)]
    flow_name: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

/// Blocking scan of `runs/*.json`, newest first, capped at `limit`. Run
/// inside `spawn_blocking`.
fn scan_runs(dir: &FsPath, limit: usize, flow_name: Option<&str>) -> Vec<Value> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((modified, path));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let mut out = Vec::new();
    for (_, path) in entries {
        if out.len() >= limit {
            break;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut doc: Value = match serde_json::from_str(&body) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if let Some(filter) = flow_name {
            if doc.get("flow_name").and_then(|v| v.as_str()) != Some(filter) {
                continue;
            }
        }
        if let Some(obj) = doc.as_object_mut() {
            obj.remove("steps");
        }
        out.push(doc);
    }
    out
}

async fn list_runs(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<RunsQuery>,
) -> impl IntoResponse {
    if !valid_slug(&slug) {
        return err(StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    let dir = runs_dir(&state, &slug);
    // Clamp the caller-supplied limit so a huge value can't force an
    // unbounded scan.
    let limit = (q.limit.unwrap_or(50) as usize).min(1000);
    let flow_name = q.flow_name.clone();
    let runs = tokio::task::spawn_blocking(move || {
        scan_runs(&dir, limit, flow_name.as_deref())
    })
    .await
    .unwrap_or_default();
    Json(json!({ "success": true, "runs": runs })).into_response()
}

async fn get_run(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !valid_slug(&slug) {
        return err(StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    if run_id.contains('/') || run_id.contains('.') {
        return err(StatusCode::BAD_REQUEST, "Invalid run_id").into_response();
    }
    let path = runs_dir(&state, &slug).join(format!("{run_id}.json"));
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(b) => b,
        Err(_) => return err(StatusCode::NOT_FOUND, "Run not found").into_response(),
    };
    let mut doc: serde_json::Value = match serde_json::from_str(&body) {
        Ok(d) => d,
        Err(e) => {
            warn!(slug = %slug, run_id = %run_id, error = %e, "run file is corrupt");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "Run file is unreadable")
                .into_response();
        }
    };
    if let Some(steps) = doc.get_mut("steps").and_then(|v| v.as_array_mut()) {
        for step in steps {
            if let Some(obj) = step.as_object_mut() {
                let has_input = obj.get("input").is_some_and(|v| !v.is_null());
                let has_output = obj.get("output").is_some_and(|v| !v.is_null());
                let has_error = obj.get("error").is_some_and(|v| !v.is_null());
                obj.insert("has_input".into(), json!(has_input));
                obj.insert("has_output".into(), json!(has_output));
                obj.insert("has_error".into(), json!(has_error));
                obj.remove("input");
                obj.remove("output");
                obj.remove("error");
            }
        }
    }
    Json(json!({ "success": true, "run": doc })).into_response()
}

async fn get_run_step(
    State(state): State<ApiState>,
    Path((slug, run_id, record_id)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if !valid_slug(&slug) {
        return err(StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    if run_id.contains('/') || run_id.contains('.') {
        return err(StatusCode::BAD_REQUEST, "Invalid run_id").into_response();
    }
    if record_id.contains('/') || record_id.contains('.') {
        return err(StatusCode::BAD_REQUEST, "Invalid record_id").into_response();
    }
    let path = runs_dir(&state, &slug).join(format!("{run_id}.json"));
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(b) => b,
        Err(_) => return err(StatusCode::NOT_FOUND, "Run not found").into_response(),
    };
    let doc: serde_json::Value = match serde_json::from_str(&body) {
        Ok(d) => d,
        Err(e) => {
            warn!(slug = %slug, run_id = %run_id, error = %e, "run file is corrupt");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "Run file is unreadable")
                .into_response();
        }
    };
    let Some(steps) = doc.get("steps").and_then(|v| v.as_array()) else {
        return err(StatusCode::NOT_FOUND, "Run has no steps").into_response();
    };
    let Some(step) = steps
        .iter()
        .find(|s| s.get("record_id").and_then(|v| v.as_str()) == Some(record_id.as_str()))
    else {
        return err(StatusCode::NOT_FOUND, "Step not found").into_response();
    };
    Json(json!({ "success": true, "step": step })).into_response()
}

// ── Stats ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Last24h,
    Last7d,
    Last30d,
    All,
}

impl Period {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "24h" => Some(Self::Last24h),
            "7d" => Some(Self::Last7d),
            "30d" => Some(Self::Last30d),
            "all" => Some(Self::All),
            _ => None,
        }
    }
    fn cutoff(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::Last24h => Some(now - Duration::hours(24)),
            Self::Last7d => Some(now - Duration::days(7)),
            Self::Last30d => Some(now - Duration::days(30)),
            Self::All => None,
        }
    }
    fn bucket_count(&self) -> u32 {
        match self {
            Self::Last24h => 24,
            Self::Last7d => 7,
            Self::Last30d => 30,
            Self::All => 30,
        }
    }
    fn bucket_unit_seconds(&self) -> i64 {
        match self {
            Self::Last24h => 3600,
            _ => 86400,
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Last24h => "24h",
            Self::Last7d => "7d",
            Self::Last30d => "30d",
            Self::All => "all",
        }
    }
}

#[derive(Deserialize)]
struct StatsQuery {
    #[serde(default)]
    period: Option<String>,
}

#[derive(Default, Serialize)]
struct StatsDoc {
    period: String,
    kpi: KpiHeader,
    top_by_count: Vec<TopFlow>,
    top_by_avg_duration: Vec<TopFlow>,
    top_by_total_time: Vec<TopFlow>,
    top_by_bytes: Vec<TopFlow>,
    recent_failures: Vec<RecentFailure>,
    step_hotspots: Vec<StepHotspot>,
    activity: Vec<ActivityBucket>,
    per_flow: Vec<PerFlowRow>,
    per_app: Vec<PerAppRow>,
    per_connector: Vec<PerConnectorRow>,
}

#[derive(Default, Serialize)]
struct KpiHeader {
    total_runs: u64,
    success_count: u64,
    failed_count: u64,
    success_rate: f64,
    total_duration_ms: u64,
    total_bytes: u64,
}

#[derive(Serialize)]
struct TopFlow {
    flow_name: String,
    app_slug: Option<String>,
    value: f64,
    count: u64,
}

#[derive(Serialize)]
struct RecentFailure {
    run_id: String,
    flow_name: String,
    app_slug: Option<String>,
    started_at: String,
    failed_step_id: Option<String>,
    error_message: Option<String>,
}

#[derive(Serialize)]
struct StepHotspot {
    step_id: String,
    flow_name: String,
    app_slug: Option<String>,
    failure_count: u64,
}

#[derive(Serialize)]
struct ActivityBucket {
    bucket_start: String,
    success_count: u64,
    failed_count: u64,
}

#[derive(Clone, Serialize)]
struct PerFlowRow {
    flow_name: String,
    app_slug: Option<String>,
    count: u64,
    avg_ms: f64,
    p50_ms: u64,
    p95_ms: u64,
    p99_ms: u64,
    success_count: u64,
    failed_count: u64,
    success_rate: f64,
    total_bytes: u64,
}

#[derive(Serialize)]
struct PerAppRow {
    slug: String,
    run_count: u64,
    success_count: u64,
    failed_count: u64,
    success_rate: f64,
    total_duration_ms: u64,
    total_bytes: u64,
}

#[derive(Serialize)]
struct PerConnectorRow {
    connector: String,
    op_count: u64,
    total_duration_ms: u64,
    total_bytes: u64,
}

fn step_bytes(step: &serde_json::Value) -> u64 {
    let mut total: u64 = 0;
    for k in ["input", "output"] {
        if let Some(v) = step.get(k) {
            if !v.is_null() {
                if let Ok(buf) = serde_json::to_vec(v) {
                    total = total.saturating_add(buf.len() as u64);
                }
            }
        }
    }
    total
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn bucket_start(dt: DateTime<Utc>, unit_seconds: i64) -> DateTime<Utc> {
    let ts = dt.timestamp();
    let floored = ts - (ts.rem_euclid(unit_seconds));
    DateTime::from_timestamp(floored, 0).unwrap_or(dt)
}

fn collect_global_runs(state: &ApiState) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    let apps_root = &state.apps_runtime_root;
    let Ok(read) = std::fs::read_dir(apps_root) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(slug) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_slug(slug) {
            continue;
        }
        let runs = path.join("runs");
        let Ok(rd) = std::fs::read_dir(&runs) else {
            continue;
        };
        for f in rd.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
                continue;
            };
            out.push((slug.to_string(), v));
        }
    }
    out
}

fn collect_app_runs(state: &ApiState, slug: &str) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    let dir = runs_dir(state, slug);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for f in rd.flatten() {
        let p = f.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        out.push((slug.to_string(), v));
    }
    out
}

fn compute_stats(
    runs: Vec<(String, serde_json::Value)>,
    period: Period,
    include_app_in_rows: bool,
) -> StatsDoc {
    let now = Utc::now();
    let cutoff = period.cutoff(now);

    let mut per_flow: HashMap<(String, String), PerFlowAcc> = HashMap::new();
    let mut per_app: HashMap<String, PerAppAcc> = HashMap::new();
    let mut per_connector: HashMap<String, PerConnectorAcc> = HashMap::new();
    let mut step_hotspots: HashMap<(String, String, String), u64> = HashMap::new();
    let mut activity: HashMap<i64, (u64, u64)> = HashMap::new();
    let mut recent_failures: Vec<RecentFailure> = Vec::new();
    let mut kpi = KpiHeader::default();

    let bucket_unit = period.bucket_unit_seconds();

    for (slug, run) in runs.iter() {
        let started_at_str = run
            .get("started_at")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Ok(started_at) = DateTime::parse_from_rfc3339(started_at_str) else {
            continue;
        };
        let started_at = started_at.with_timezone(&Utc);
        if let Some(c) = cutoff {
            if started_at < c {
                continue;
            }
        }
        let flow_name = run
            .get("flow_name")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let status = run.get("status").and_then(|v| v.as_str()).unwrap_or("?");
        let duration_ms = run
            .get("duration_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0) as u64;

        let mut run_bytes: u64 = 0;
        if let Some(steps) = run.get("steps").and_then(|v| v.as_array()) {
            for step in steps {
                run_bytes = run_bytes.saturating_add(step_bytes(step));

                let kind = step.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                if kind == "connector" {
                    let detail = step.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                    let connector = detail.split('.').next().unwrap_or("").to_string();
                    if !connector.is_empty() {
                        let entry = per_connector.entry(connector).or_default();
                        entry.op_count += 1;
                        entry.total_duration_ms += step
                            .get("duration_ms")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0)
                            .max(0) as u64;
                        entry.total_bytes += step_bytes(step);
                    }
                }

                if step.get("status").and_then(|v| v.as_str()) == Some("failed") {
                    let step_id = step
                        .get("step_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let key_app = if include_app_in_rows {
                        slug.clone()
                    } else {
                        String::new()
                    };
                    *step_hotspots
                        .entry((key_app, flow_name.clone(), step_id))
                        .or_default() += 1;
                }
            }
        }

        kpi.total_runs += 1;
        kpi.total_duration_ms = kpi.total_duration_ms.saturating_add(duration_ms);
        kpi.total_bytes = kpi.total_bytes.saturating_add(run_bytes);
        let is_success = status == "success";
        let is_failed = status == "failed";
        if is_success {
            kpi.success_count += 1;
        }
        if is_failed {
            kpi.failed_count += 1;
        }

        let key_app = if include_app_in_rows {
            slug.clone()
        } else {
            String::new()
        };
        let pf = per_flow.entry((key_app.clone(), flow_name.clone())).or_default();
        pf.count += 1;
        pf.total_ms = pf.total_ms.saturating_add(duration_ms);
        pf.durations_ms.push(duration_ms);
        pf.total_bytes = pf.total_bytes.saturating_add(run_bytes);
        if is_success {
            pf.success_count += 1;
        }
        if is_failed {
            pf.failed_count += 1;
        }

        let pa = per_app.entry(slug.clone()).or_default();
        pa.run_count += 1;
        pa.total_duration_ms = pa.total_duration_ms.saturating_add(duration_ms);
        pa.total_bytes = pa.total_bytes.saturating_add(run_bytes);
        if is_success {
            pa.success_count += 1;
        }
        if is_failed {
            pa.failed_count += 1;
        }

        let bs = bucket_start(started_at, bucket_unit).timestamp();
        let act = activity.entry(bs).or_insert((0, 0));
        if is_success {
            act.0 += 1;
        }
        if is_failed {
            act.1 += 1;
        }

        if is_failed {
            let err_obj = run.get("error");
            let failed_step_id = err_obj
                .and_then(|e| e.get("step_id"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let error_message = err_obj
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .map(String::from);
            recent_failures.push(RecentFailure {
                run_id: run
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                flow_name: flow_name.clone(),
                app_slug: if include_app_in_rows {
                    Some(slug.clone())
                } else {
                    None
                },
                started_at: started_at_str.to_string(),
                failed_step_id,
                error_message,
            });
        }
    }

    kpi.success_rate = if kpi.total_runs > 0 {
        (kpi.success_count as f64) / (kpi.total_runs as f64)
    } else {
        0.0
    };

    let mut per_flow_rows: Vec<PerFlowRow> = per_flow
        .into_iter()
        .map(|((app, flow_name), mut acc)| {
            acc.durations_ms.sort_unstable();
            let avg = if acc.count > 0 {
                (acc.total_ms as f64) / (acc.count as f64)
            } else {
                0.0
            };
            PerFlowRow {
                flow_name,
                app_slug: if include_app_in_rows { Some(app) } else { None },
                count: acc.count,
                avg_ms: avg,
                p50_ms: percentile(&acc.durations_ms, 50.0),
                p95_ms: percentile(&acc.durations_ms, 95.0),
                p99_ms: percentile(&acc.durations_ms, 99.0),
                success_count: acc.success_count,
                failed_count: acc.failed_count,
                success_rate: if acc.count > 0 {
                    (acc.success_count as f64) / (acc.count as f64)
                } else {
                    0.0
                },
                total_bytes: acc.total_bytes,
            }
        })
        .collect();
    per_flow_rows.sort_by(|a, b| b.count.cmp(&a.count));

    let top_by_count: Vec<TopFlow> = per_flow_rows
        .iter()
        .take(5)
        .map(|r| TopFlow {
            flow_name: r.flow_name.clone(),
            app_slug: r.app_slug.clone(),
            value: r.count as f64,
            count: r.count,
        })
        .collect();
    let mut by_avg = per_flow_rows.clone();
    by_avg.sort_by(|a, b| {
        b.avg_ms
            .partial_cmp(&a.avg_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_by_avg_duration: Vec<TopFlow> = by_avg
        .iter()
        .take(5)
        .map(|r| TopFlow {
            flow_name: r.flow_name.clone(),
            app_slug: r.app_slug.clone(),
            value: r.avg_ms,
            count: r.count,
        })
        .collect();
    let mut by_total = per_flow_rows.clone();
    by_total.sort_by(|a, b| {
        ((b.avg_ms * b.count as f64))
            .partial_cmp(&(a.avg_ms * a.count as f64))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_by_total_time: Vec<TopFlow> = by_total
        .iter()
        .take(5)
        .map(|r| TopFlow {
            flow_name: r.flow_name.clone(),
            app_slug: r.app_slug.clone(),
            value: r.avg_ms * r.count as f64,
            count: r.count,
        })
        .collect();
    let mut by_bytes = per_flow_rows.clone();
    by_bytes.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));
    let top_by_bytes: Vec<TopFlow> = by_bytes
        .iter()
        .take(5)
        .map(|r| TopFlow {
            flow_name: r.flow_name.clone(),
            app_slug: r.app_slug.clone(),
            value: r.total_bytes as f64,
            count: r.count,
        })
        .collect();

    recent_failures.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    recent_failures.truncate(10);

    let mut hotspot_rows: Vec<StepHotspot> = step_hotspots
        .into_iter()
        .map(|((app, flow_name, step_id), failure_count)| StepHotspot {
            step_id,
            flow_name,
            app_slug: if include_app_in_rows && !app.is_empty() {
                Some(app)
            } else {
                None
            },
            failure_count,
        })
        .collect();
    hotspot_rows.sort_by(|a, b| b.failure_count.cmp(&a.failure_count));
    hotspot_rows.truncate(10);

    let bucket_count = period.bucket_count();
    let now_bucket = bucket_start(now, bucket_unit).timestamp();
    let activity_rows: Vec<ActivityBucket> = (0..bucket_count)
        .rev()
        .map(|i| {
            let ts = now_bucket - (i as i64) * bucket_unit;
            let (s, f) = activity.get(&ts).copied().unwrap_or((0, 0));
            ActivityBucket {
                bucket_start: DateTime::from_timestamp(ts, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                success_count: s,
                failed_count: f,
            }
        })
        .collect();

    let mut per_app_rows: Vec<PerAppRow> = if include_app_in_rows {
        per_app
            .into_iter()
            .map(|(slug, acc)| PerAppRow {
                slug,
                run_count: acc.run_count,
                success_count: acc.success_count,
                failed_count: acc.failed_count,
                success_rate: if acc.run_count > 0 {
                    (acc.success_count as f64) / (acc.run_count as f64)
                } else {
                    0.0
                },
                total_duration_ms: acc.total_duration_ms,
                total_bytes: acc.total_bytes,
            })
            .collect()
    } else {
        Vec::new()
    };
    per_app_rows.sort_by(|a, b| b.run_count.cmp(&a.run_count));

    let mut per_connector_rows: Vec<PerConnectorRow> = if include_app_in_rows {
        per_connector
            .into_iter()
            .map(|(connector, acc)| PerConnectorRow {
                connector,
                op_count: acc.op_count,
                total_duration_ms: acc.total_duration_ms,
                total_bytes: acc.total_bytes,
            })
            .collect()
    } else {
        Vec::new()
    };
    per_connector_rows.sort_by(|a, b| b.op_count.cmp(&a.op_count));

    StatsDoc {
        period: period.label().to_string(),
        kpi,
        top_by_count,
        top_by_avg_duration,
        top_by_total_time,
        top_by_bytes,
        recent_failures,
        step_hotspots: hotspot_rows,
        activity: activity_rows,
        per_flow: per_flow_rows,
        per_app: per_app_rows,
        per_connector: per_connector_rows,
    }
}

#[derive(Default)]
struct PerFlowAcc {
    count: u64,
    total_ms: u64,
    durations_ms: Vec<u64>,
    success_count: u64,
    failed_count: u64,
    total_bytes: u64,
}

#[derive(Default)]
struct PerAppAcc {
    run_count: u64,
    success_count: u64,
    failed_count: u64,
    total_duration_ms: u64,
    total_bytes: u64,
}

#[derive(Default)]
struct PerConnectorAcc {
    op_count: u64,
    total_duration_ms: u64,
    total_bytes: u64,
}

async fn get_app_stats(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<StatsQuery>,
) -> impl IntoResponse {
    if !valid_slug(&slug) {
        return err(StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    let period_str = q.period.as_deref().unwrap_or("7d");
    let Some(period) = Period::parse(period_str) else {
        return err(
            StatusCode::BAD_REQUEST,
            "Invalid period (use 24h|7d|30d|all)",
        )
        .into_response();
    };
    // Reading + aggregating every run file is blocking + CPU-bound — keep it
    // off the async worker threads.
    let slug_for_log = slug.clone();
    let computed = tokio::task::spawn_blocking(move || {
        let runs = collect_app_runs(&state, &slug);
        let n = runs.len();
        (compute_stats(runs, period, false), n)
    })
    .await;
    match computed {
        Ok((stats, run_count)) => {
            info!(slug = %slug_for_log, period = period.label(), run_count,
                "flow stats aggregated (app)");
            Json(json!({ "success": true, "stats": stats })).into_response()
        }
        Err(e) => {
            warn!(error = %e, "app stats task failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "stats aggregation failed").into_response()
        }
    }
}

async fn get_global_stats(
    State(state): State<ApiState>,
    Query(q): Query<StatsQuery>,
) -> impl IntoResponse {
    let period_str = q.period.as_deref().unwrap_or("7d");
    let Some(period) = Period::parse(period_str) else {
        return err(
            StatusCode::BAD_REQUEST,
            "Invalid period (use 24h|7d|30d|all)",
        )
        .into_response();
    };
    let computed = tokio::task::spawn_blocking(move || {
        let runs = collect_global_runs(&state);
        let run_count = runs.len();
        let app_count = runs
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len();
        (compute_stats(runs, period, true), run_count, app_count)
    })
    .await;
    match computed {
        Ok((stats, run_count, app_count)) => {
            info!(period = period.label(), app_count, run_count,
                "flow stats aggregated (global)");
            Json(json!({ "success": true, "stats": stats })).into_response()
        }
        Err(e) => {
            warn!(error = %e, "global stats task failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "stats aggregation failed").into_response()
        }
    }
}
