pub mod clients;
pub mod routes;
pub mod state;

use std::path::PathBuf;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use state::ApiState;
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: ApiState, web_dist: Option<PathBuf>) -> Router {
    let mut app = Router::new()
        .nest("/api", api_router())
        .nest("/apps", routes::apps_proxy::router())
        // Forward `/_next/*` and `/static/*` to the NextJS fallback app (default `www`).
        // See `routes::apps_proxy::next_fallback_handler` doc for the why.
        .route(
            "/_next/{*rest}",
            axum::routing::any(routes::apps_proxy::next_fallback_handler),
        )
        .with_state(state);

    if let Some(dir) = web_dist {
        if dir.is_dir() {
            // Serve every file in `web/dist/` (manifest.json, sw.js, favicon.svg,
            // icons, /assets/*, etc.) with the right Content-Type, and fall back
            // to `index.html` for any 404 — that's standard SPA semantics.
            let index_path = dir.join("index.html");
            let serve_dir = ServeDir::new(&dir).not_found_service(ServeFile::new(index_path));
            app = app.fallback_service(serve_dir);
        } else {
            tracing::warn!(path = %dir.display(), "web dist not a directory — skipping SPA serve");
        }
    }

    app
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
        .nest("/store", routes::store::router())
        .nest("/git", routes::git::router())
        .nest("/apps", routes::apps::router())
        .nest("/apps", routes::apps_db::router())
        .nest("/dv", routes::dv::router())
        .nest("/tasks", routes::tasks::router())
        .merge(routes::flows::router())
        .fallback(api_404)
}
