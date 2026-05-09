//! `hr-flow-callback` — helper for HomeRoute apps written in Rust that host
//! custom flow actions / connectors and want to expose them to `hr-flowd` via
//! HTTP.
//!
//! Mount the returned `axum::Router` under the app's main router; it exposes:
//!   POST /_flow/action/{name}
//!   POST /_flow/connector/{name}/{op}
//!
//! Bearer auth (`HR_FLOW_TOKEN` env var) is enforced before any handler runs.
//! Each handler call is wrapped in `AssertUnwindSafe(_).catch_unwind()` so a
//! panic in user code becomes a structured `{ error, kind: "panic" }`
//! response — never a 500 from axum, never an aborted connection.
//!
//! Typical usage in an app's main.rs :
//!
//! ```ignore
//! let flow_router = hr_flow_callback::router(state.clone())
//!     .with_action_fn("compute_risk_score", actions::compute_risk_score)
//!     .with_connector("openrouter", Arc::new(OpenRouterConnector::from_env()?))
//!     .into_router();
//!
//! let app = Router::new()
//!     .merge(flow_router)
//!     .merge(business_router);
//! ```

mod auth;
mod handler;
mod router;

pub use router::CallbackRouter;

use std::env;

/// Build a callback router using the bearer token from `HR_FLOW_TOKEN`.
/// Panics if the env var is missing — apps with flows are expected to have
/// this provisioned at scaffold time.
pub fn router() -> CallbackRouter {
    let bearer = env::var("HR_FLOW_TOKEN")
        .expect("HR_FLOW_TOKEN must be set for hr-flow-callback to enforce bearer auth");
    CallbackRouter::with_bearer(bearer)
}

/// Build a callback router with an explicit token (useful in tests).
pub fn router_with_token(bearer: impl Into<String>) -> CallbackRouter {
    CallbackRouter::with_bearer(bearer.into())
}
