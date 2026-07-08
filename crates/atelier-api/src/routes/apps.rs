//! Apps routes — Phase 9 (cutover en cours).
//!
//! Lecture **et** écriture sur `state.app_registry` + `state.supervisor`.
//! Atelier devient le canonical writer post-cutover. Les routes appellent
//! directement la lib `atelier-apps` (pas d'IPC orchestrator séparé).
//!
//! Endpoints exposés :
//! - GET    /api/apps            (list)
//! - POST   /api/apps            (create — même mutator que MCP `app.create`)
//! - GET    /api/apps/{slug}     (single)
//! - PATCH  /api/apps/{slug}     (update) · DELETE /api/apps/{slug}
//! - GET    /api/apps/{slug}/env
//! - POST   /api/apps/{slug}/control  body {action: start|stop|restart}
//! - GET    /api/apps/{slug}/status   process state (pid, uptime, port)
//! - POST   /api/apps/{slug}/ship     body {timeout_secs?: u64} (wrapper MCP `app.ship`)

use std::collections::BTreeMap;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::mcp::apps_ops::AppsContext;
use crate::state::ApiState;
use atelier_apps::types::EnvScope;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_apps).post(create_app))
        .route("/{slug}", get(get_app).patch(update_app).delete(delete_app))
        // Env management (structured view + per-variable user CRUD). The `.env`
        // file is a generated projection — these routes mutate the model and
        // re-render it; platform vars (PORT/HR_DV_*/ATELIER_*) are read-only.
        .route("/{slug}/env", get(get_app_env))
        .route(
            "/{slug}/env/{key}",
            get(get_app_env_var).put(set_app_env_var).delete(delete_app_env_var),
        )
        .route("/{slug}/reconcile-env", post(reconcile_env))
        .route("/{slug}/build-env", get(get_app_build_env))
        .route("/{slug}/control", post(control_app))
        .route("/{slug}/status", get(app_status))
        .route("/{slug}/ship", post(ship_app))
        .route("/{slug}/build-event", post(build_event))
}

/// Map an `anyhow` error from the env layer onto an HTTP status by inspecting
/// its message (the env ops return human-readable, classifiable errors).
fn env_err(e: anyhow::Error) -> axum::response::Response {
    let msg = e.to_string();
    let status = if msg.contains("not found") {
        StatusCode::NOT_FOUND
    } else if msg.contains("platform-managed")
        || msg.contains("invalid env key")
        || msg.contains("cannot contain")
        || msg.contains("géré par la plateforme") // clé interdite (CLAUDE_CONFIG_DIR)
        || msg.contains("valeur interdite") // valeur pointant une zone plateforme
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(json!({"success": false, "error": msg}))).into_response()
}

fn parse_scope(s: Option<&str>) -> EnvScope {
    match s {
        Some("build") => EnvScope::Build,
        Some("both") => EnvScope::Both,
        _ => EnvScope::Runtime,
    }
}

fn validate_slug(slug: &str) -> Result<(), axum::response::Response> {
    if atelier_apps::valid_slug(slug) {
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

#[derive(Deserialize, Default)]
struct EnvViewQuery {
    /// Return secret values in clear instead of masking them. Loopback/LAN
    /// trust model, same as the sibling `/api/apps/*` routes.
    #[serde(default)]
    reveal: bool,
}

/// `GET /api/apps/{slug}/env` — structured, ownership-aware view of the app's
/// full environment (platform tier + user tier). Secret values are masked
/// (omitted) unless `?reveal=1`. Replaces the old flat-map dump.
async fn get_app_env(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<EnvViewQuery>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let ctx = AppsContext::from_api_state(&state);
    match ctx.env_view(&slug, q.reveal).await {
        Ok(vars) => Json(json!({"success": true, "data": vars})).into_response(),
        Err(e) => env_err(e),
    }
}

/// `GET /api/apps/{slug}/env/{key}` — reveal a single variable's plaintext
/// value (platform or user). Keeps secrets out of the bulk view payload until
/// explicitly requested (per-row "eye" in the UI).
async fn get_app_env_var(
    State(state): State<ApiState>,
    Path((slug, key)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let ctx = AppsContext::from_api_state(&state);
    match ctx.env_var_value(&slug, &key).await {
        Ok(Some(value)) => {
            Json(json!({"success": true, "data": {"key": key, "value": value}})).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": format!("env var not found: {key}")})),
        )
            .into_response(),
        Err(e) => env_err(e),
    }
}

#[derive(Deserialize)]
struct SetEnvBody {
    value: String,
    #[serde(default)]
    secret: bool,
    #[serde(default)]
    scope: Option<String>,
}

/// `PUT /api/apps/{slug}/env/{key}` — insert or replace a USER variable. Rejects
/// platform-managed keys and malformed names. Re-renders the `.env`; the change
/// applies on the app's next restart (`restart_required: true`).
async fn set_app_env_var(
    State(state): State<ApiState>,
    Path((slug, key)): Path<(String, String)>,
    Json(body): Json<SetEnvBody>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let scope = parse_scope(body.scope.as_deref());
    info!(slug = %slug, key = %key, secret = body.secret, scope = scope.as_str(), "AppEnvSet");
    let ctx = AppsContext::from_api_state(&state);
    match ctx.env_set_var(&slug, &key, &body.value, body.secret, scope).await {
        Ok(()) => Json(json!({
            "success": true,
            "data": {"key": key, "restart_required": true}
        }))
        .into_response(),
        Err(e) => env_err(e),
    }
}

/// `DELETE /api/apps/{slug}/env/{key}` — remove a USER variable.
async fn delete_app_env_var(
    State(state): State<ApiState>,
    Path((slug, key)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    info!(slug = %slug, key = %key, "AppEnvDelete");
    let ctx = AppsContext::from_api_state(&state);
    match ctx.env_delete_var(&slug, &key).await {
        Ok(true) => Json(json!({
            "success": true,
            "data": {"key": key, "restart_required": true}
        }))
        .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": format!("env var not found: {key}")})),
        )
            .into_response(),
        Err(e) => env_err(e),
    }
}

#[derive(Deserialize, Default)]
struct ReconcileQuery {
    /// Default true: only report the plan, do not write. Set `dry_run=false` to
    /// actually re-render the `.env`.
    #[serde(default = "default_true")]
    dry_run: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /api/apps/{slug}/reconcile-env` — admin/debug. Recompute the `.env`
/// projection (import residual hand-seeded vars, GC dead vars). Dry-run by
/// default; returns the diff report.
async fn reconcile_env(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<ReconcileQuery>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let ctx = AppsContext::from_api_state(&state);
    match ctx.reconcile_app_env(&slug, q.dry_run).await {
        Ok(report) => {
            Json(json!({"success": true, "data": serde_json::to_value(&report).unwrap_or(Value::Null)}))
                .into_response()
        }
        Err(e) => env_err(e),
    }
}

/// `GET /api/apps/{slug}/build-env` — `eval`-able `export K='v'` lines for the
/// app's build-scoped vars. Sourced over loopback by the generated `build.sh`
/// and `deploy-app.sh` so framework-baked public vars (`VITE_*`/`NEXT_PUBLIC_*`)
/// reach the build. Empty for apps without build-scoped vars. text/plain.
async fn get_app_build_env(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let ctx = AppsContext::from_api_state(&state);
    match ctx.build_env_script(&slug).await {
        Ok(script) => script.into_response(),
        Err(e) => env_err(e),
    }
}

#[derive(Deserialize)]
struct CreateAppBody {
    name: String,
    slug: String,
    /// Label technologique libre, purement informatif (peut rester vide —
    /// l'agent le posera via app.update quand il bootstrappe le projet).
    #[serde(default)]
    stack: String,
    visibility: Option<String>,
    run_command: Option<String>,
    build_command: Option<String>,
    health_path: Option<String>,
    build_artefact: Option<String>,
}

/// `POST /api/apps` — create an app. Delegates to the same `AppsContext::create`
/// the MCP `app.create` tool uses (port assign, workspace init, registry, git
/// repo, context regen, initial `.env`). WHY: the homepage "Nouvelle application"
/// modal called this route, which did not exist — creation 405'd from the UI
/// (only the MCP path worked). `has_db` mirrors the MCP handler's hardcoded
/// `true`: every app gets its dataverse `app_{slug}` provisioned.
async fn create_app(
    State(state): State<ApiState>,
    Json(b): Json<CreateAppBody>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&b.slug) {
        return r;
    }
    info!(slug = %b.slug, stack = %b.stack, "AppCreate (HTTP)");
    let ctx = AppsContext::from_api_state(&state);
    let resp = ctx
        .create(
            b.slug.clone(),
            b.name,
            b.stack,
            true,
            b.visibility.unwrap_or_else(|| "private".to_string()),
            b.run_command,
            b.build_command,
            b.health_path,
            b.build_artefact,
        )
        .await;
    ipc_to_http(resp)
}

#[derive(Deserialize, Default)]
struct UpdateAppBody {
    name: Option<String>,
    stack: Option<String>,
    visibility: Option<String>,
    run_command: Option<String>,
    build_command: Option<String>,
    health_path: Option<String>,
    env_vars: Option<BTreeMap<String, String>>,
    has_db: Option<bool>,
    build_artefact: Option<String>,
    /// Réglage plateforme (page Paramètres) : injecter `CLAUDE_CODE_OAUTH_TOKEN`
    /// à cette app. Non exposé aux agents (les tools MCP `app.update` passent None).
    claude_access: Option<bool>,
}

/// `PATCH /api/apps/{slug}` — update app settings (name/visibility/commands/
/// health/env/has_db). Delegates to the same `AppsContext::update` the MCP
/// `app.update` tool uses, so HTTP and the agent converge on one mutator.
/// WHY: the Studio Settings save button previously called this route, which did
/// not exist — the save silently no-op'd.
async fn update_app(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    body: Option<Json<UpdateAppBody>>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let Json(b) = body.unwrap_or_default();
    info!(slug = %slug, "AppUpdate (HTTP)");
    let ctx = AppsContext::from_api_state(&state);
    let resp = ctx
        .update(
            slug.clone(),
            b.name,
            b.stack,
            b.visibility,
            b.run_command,
            b.build_command,
            b.health_path,
            b.env_vars,
            b.has_db,
            b.build_artefact,
            b.claude_access,
        )
        .await;
    ipc_to_http(resp)
}

#[derive(Deserialize, Default)]
struct DeleteAppQuery {
    #[serde(default)]
    keep_data: bool,
}

/// `DELETE /api/apps/{slug}` — delete an app (stop, remove route/registry/port,
/// and rm the data dir unless `?keep_data=1`).
async fn delete_app(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<DeleteAppQuery>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    info!(slug = %slug, keep_data = q.keep_data, "AppDelete (HTTP)");
    let ctx = AppsContext::from_api_state(&state);
    let resp = ctx.delete(slug.clone(), q.keep_data).await;
    // Best-effort: drop the Homeroute hostname route + local mapping once the app
    // is gone (its port may be reassigned, so a stale route would misroute).
    // Non-fatal — never affects the delete result.
    if resp.ok {
        state.homeroute.cleanup_on_delete(&slug).await;
    }
    ipc_to_http(resp)
}

/// Map an `IpcResponse` (the MCP-layer result type the `AppsContext` methods
/// return) onto an HTTP JSON envelope.
fn ipc_to_http(resp: atelier_ipc::types::IpcResponse) -> axum::response::Response {
    if resp.ok {
        Json(json!({"success": true, "data": resp.data.unwrap_or(json!({"ok": true}))}))
            .into_response()
    } else {
        let err = resp.error.unwrap_or_else(|| "operation failed".to_string());
        let status = if err.contains("not found") {
            StatusCode::NOT_FOUND
        } else if err.contains("already exists") {
            StatusCode::CONFLICT
        } else if err.starts_with("invalid") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, Json(json!({"success": false, "error": err}))).into_response()
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
    // Same per-slug lock as build/ship: a restart racing a ship window (or a
    // second restart whose stop kills the first one's fresh start) must be
    // refused, not interleaved. try_lock → 409, never queue.
    let lock = {
        let mut map = state.build_locks.lock().await;
        map.entry(slug.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let Ok(_guard) = lock.try_lock() else {
        warn!(slug = %slug, action = %body.action, "AppControl: another operation in progress");
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "success": false,
                "error": format!("another build/deploy or lifecycle action is already running for '{slug}' — retry once it finishes")
            })),
        )
            .into_response();
    };
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
                    "exe_path": status.exe_path,
                    "exe_mtime": status.exe_mtime,
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
/// Thin HTTP wrapper around `AppsContext::ship` (aussi exposé comme tool MCP
/// `ship` en scope projet — même lock, même canal build). Stops the supervised
/// process and restarts it. Build artefacts are expected to already be in
/// `/var/lib/atelier/apps/<slug>/src/`. If `ATELIER_BUILD_HOST` is set, an
/// optional SSH+rsync step pulls pre-built artefacts from that host first.
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

    // `from_api_state` câble le canal de build PARTAGÉ (state.events.app_build)
    // relayé par le WebSocket — pas un canal jetable, sinon les étapes du ship
    // (stop/restart) ne s'afficheraient pas dans le badge Studio.
    let ctx = AppsContext::from_api_state(&state);

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

#[derive(Deserialize, Default)]
struct BuildEventBody {
    /// "started" | "step" | "finished" | "error"
    status: String,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    step: Option<u32>,
    #[serde(default)]
    total_steps: Option<u32>,
}

/// `POST /api/apps/{slug}/build-event`
///
/// Relais des étapes de build émises par la skill `0-build` (qui tourne en local
/// sur Medion) vers le canal `app:build` du WebSocket → le badge per-app du Studio.
/// Sans secret, payload purement cosmétique (allume/éteint un badge) ; non
/// authentifié comme les routes `/api/apps/*` sœurs (confiance LAN, bind 0.0.0.0).
async fn build_event(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    body: Option<Json<BuildEventBody>>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let Json(b) = body.unwrap_or_default();
    info!(slug = %slug, status = %b.status, phase = ?b.phase, "AppBuildEvent (external)");

    let ctx = AppsContext::from_api_state(&state);
    let resp = ctx
        .emit_external_build_event(
            slug.clone(),
            b.status,
            b.phase,
            b.message,
            b.duration_ms,
            b.error,
            b.step,
            b.total_steps,
        )
        .await;
    if resp.ok {
        Json(json!({"success": true, "data": {"ok": true}})).into_response()
    } else {
        let err_msg = resp.error.unwrap_or_else(|| "build-event failed".to_string());
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": err_msg})),
        )
            .into_response()
    }
}
