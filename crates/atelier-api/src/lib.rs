#![recursion_limit = "512"]

pub mod clients;
pub mod host_gate;
pub mod mcp;
pub mod pm_prompts;
pub mod routes;
pub mod state;

use std::path::PathBuf;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use state::ApiState;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: ApiState, web_dist: Option<PathBuf>) -> Router {
    // The host-gate needs the state but `state` is consumed by `.with_state`.
    let gate_state = state.clone();
    let mut app = Router::new()
        .nest("/api", api_router())
        .nest("/apps", routes::apps_proxy::router())
        // MCP (Model Context Protocol) JSON-RPC endpoint — POST /mcp[?project=<slug>].
        // Mounted at the top level (not under /api) to match the legacy
        // homeroute layout the apps' `.mcp.json` files still target.
        .nest("/mcp", routes::mcp::router())
        // Forward `/_next/*` and `/static/*` to the NextJS fallback app (default `www`).
        // See `routes::apps_proxy::next_fallback_handler` doc for the why.
        .route(
            "/_next/{*rest}",
            axum::routing::any(routes::apps_proxy::next_fallback_handler),
        )
        .with_state(state);

    if let Some(dir) = web_dist {
        if dir.is_dir() {
            // The Studio is a SECOND, separately-built Vite SPA (base `/studio/`,
            // outDir `web/dist/studio/`) served by the same API — see the frontend
            // split (2026-06-21). It must be nested BEFORE the homepage fallback so
            // `/studio/*` (incl. its `/studio/assets/*`) routes to the studio bundle
            // and is never swallowed by the homepage SPA fallback. `nest_service`
            // strips `/studio` → ServeDir resolves `web/dist/studio/...`; a client
            // route like `/studio/<slug>` 404s in ServeDir → falls back to the
            // studio `index.html` (200) for SPA routing.
            let studio_dir = dir.join("studio");
            // The studio Vite build's entry is `studio.html` (Vite keeps the input
            // filename), so that — not `index.html` — is the SPA fallback document.
            let studio_index = studio_dir.join("studio.html");
            if studio_index.is_file() {
                let studio_serve =
                    ServeDir::new(&studio_dir).fallback(ServeFile::new(studio_index));
                app = app.nest_service("/studio", studio_serve);
            } else {
                tracing::warn!(path = %studio_dir.display(), "studio dist missing — /studio not served");
            }

            // Serve every file in `web/dist/` (manifest.json, sw.js, favicon.svg,
            // icons, /assets/*, etc.) with the right Content-Type, and fall back
            // to `index.html` for any 404 — that's standard SPA semantics.
            let index_path = dir.join("index.html");
            // `not_found_service` wraps the fallback response in SetStatus(404)
            // (cf. tower-http-0.6 serve_dir/mod.rs:241). Use `fallback` so
            // index.html is served with 200 for SPA routes.
            let serve_dir = ServeDir::new(&dir).fallback(ServeFile::new(index_path));
            app = app.fallback_service(serve_dir);
        } else {
            tracing::warn!(path = %dir.display(), "web dist not a directory — skipping SPA serve");
        }
    }

    // Host-gate : les hostnames publics assignés (Homeroute) ciblent CE port ;
    // sur ces hosts on ne sert QUE /apps/{slug}/ (307 sinon, 404 autres apps).
    // `Router::layer` (et pas route_layer) pour couvrir aussi fallback_service
    // SPA, /studio, /mcp et /_next. CatchPanic reste le plus externe (filet).
    app = app.layer(axum::middleware::from_fn_with_state(
        gate_state,
        host_gate::host_gate,
    ));
    // Filet global : sans lui, un panic de handler avorte la tâche hyper et le
    // client reçoit une connexion coupée (pas de statut) au lieu d'un 500 JSON.
    app.layer(CatchPanicLayer::custom(handle_panic))
}

fn handle_panic(err: Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response {
    let detail = if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "unknown panic".to_string()
    };
    tracing::error!(panic = %detail, "handler panicked — returning 500");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"success": false, "error": "internal server error"})),
    )
        .into_response()
}

async fn api_404() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"success": false, "error": "endpoint not found"})),
    )
}

fn api_router() -> Router<ApiState> {
    Router::new()
        .merge(routes::health::router())
        .nest("/docs", routes::docs::router())
        .nest("/git", routes::git::router())
        .nest("/hooks", routes::hooks::router())
        .nest("/apps", routes::apps::router())
        .nest("/apps", routes::apps_db::router())
        .nest("/dv", routes::dv::router())
        .nest("/tasks", routes::tasks::router())
        .nest("/logs", routes::logs::router())
        .nest("/findings", routes::surveillance::global_router())
        .nest("/surveillance", routes::surveillance::overview_router())
        .nest("/apps", routes::surveillance::app_router())
        .nest("/apps", routes::agent::app_router())
        .nest("/apps", routes::source::app_router())
        .nest("/apps", routes::issues::app_router())
        .nest("/notifications", routes::notifications::router())
        .nest("/agent", routes::agent::global_router())
        .nest("/backup", routes::backup::router())
        .nest("/pilot", routes::pilot::router())
        .nest("/homeroute", routes::homeroute::router())
        .nest("/stats", routes::stats::router())
        .merge(routes::ws::router())
        .fallback(api_404)
}
