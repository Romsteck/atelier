pub mod routes;
pub mod state;

use std::path::PathBuf;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use serde_json::json;
use state::ApiState;
use tower_http::services::ServeDir;

pub fn router(state: ApiState, web_dist: Option<PathBuf>) -> Router {
    let mut app = Router::new().nest("/api", api_router()).with_state(state);

    if let Some(dir) = web_dist {
        if dir.is_dir() {
            let assets_dir = dir.join("assets");
            let index_path = dir.join("index.html");
            // /assets/* via ServeDir (returns proper 404 for missing assets — never SPA-fallback).
            // Everything else falls back to index.html with 200 (SPA semantics).
            app = app
                .nest_service("/assets", ServeDir::new(assets_dir))
                .fallback(move || spa_fallback(index_path.clone()));
        } else {
            tracing::warn!(path = %dir.display(), "web dist not a directory — skipping SPA serve");
        }
    }

    app
}

async fn spa_fallback(index_path: PathBuf) -> impl IntoResponse {
    match tokio::fs::read_to_string(&index_path).await {
        Ok(html) => (StatusCode::OK, Html(html)).into_response(),
        Err(err) => {
            tracing::error!(?err, path = %index_path.display(), "failed to read SPA index.html");
            (StatusCode::INTERNAL_SERVER_ERROR, "atelier: spa index unavailable")
                .into_response()
        }
    }
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
        .fallback(api_404)
}
