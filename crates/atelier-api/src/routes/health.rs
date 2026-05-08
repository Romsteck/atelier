use axum::{Json, Router, routing::get};
use serde_json::json;
use tracing::instrument;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new().route("/health", get(health))
}

#[instrument]
async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "atelier",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
