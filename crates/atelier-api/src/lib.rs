pub mod routes;
pub mod state;

use axum::Router;
use state::ApiState;

pub fn router(state: ApiState) -> Router {
    Router::new()
        .nest("/api", api_router())
        .with_state(state)
}

fn api_router() -> Router<ApiState> {
    Router::new()
        .merge(routes::health::router())
        .nest("/docs", routes::docs::router())
}
