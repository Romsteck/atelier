use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};

use crate::{auth, state::DaemonState};

pub mod admin;
pub mod definitions;
pub mod health;
pub mod runs;

pub fn router(state: Arc<DaemonState>) -> Router {
    let public = Router::new().route("/v1/health", get(health::handler));

    let protected = Router::new()
        .route("/v1/runs", post(runs::start).get(runs::list))
        .route("/v1/runs/{run_id}", get(runs::get))
        .route("/v1/runs/{run_id}/replay", post(runs::replay))
        .route("/v1/runs/{run_id}/cancel", post(runs::cancel))
        .route("/v1/definitions", get(definitions::list))
        .route("/v1/_admin/reload", post(admin::reload))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));

    public.merge(protected).with_state(state)
}
