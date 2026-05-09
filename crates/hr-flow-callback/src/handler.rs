//! Per-action / per-connector axum handler.
//!
//! Wraps the user-provided closure with `catch_unwind` and turns every outcome
//! into the `{ output }` / `{ error, kind }` body the daemon expects.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use axum::{extract::Path, Json};
use futures::FutureExt;
use hr_flow::{Connector, FlowError, FlowResult};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

#[derive(Debug, Deserialize)]
pub(crate) struct CallbackBody {
    /// Daemon-supplied run UUID. Phase 2 keeps it captured but does not yet
    /// thread it through to user code; Phase 5+ exposes it via a context arg.
    #[serde(default)]
    #[allow(dead_code)]
    pub run_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub step_id: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub params: Value,
}

pub(crate) type ActionHandler =
    Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = FlowResult<Value>> + Send>> + Send + Sync>;

/// `POST /_flow/action/{name}`. The handler is selected by name from the
/// router's action registry (see `CallbackRouter::with_action_fn`).
pub(crate) async fn action_handler(
    Path(name): Path<String>,
    handler: ActionHandler,
    body: CallbackBody,
) -> Json<Value> {
    let fut = AssertUnwindSafe(async move { handler(body.input).await }).catch_unwind();
    match fut.await {
        Ok(Ok(output)) => Json(json!({ "output": output })),
        Ok(Err(err)) => {
            let (kind, message) = classify(&err);
            warn!(action = %name, kind, %message, "callback action failed");
            Json(json!({ "error": message, "kind": kind }))
        }
        Err(payload) => {
            let msg = panic_message(payload);
            warn!(action = %name, panic = %msg, "callback action panicked");
            Json(json!({ "error": format!("panic: {msg}"), "kind": "panic" }))
        }
    }
}

/// `POST /_flow/connector/{name}/{op}`. The connector is selected by name
/// from the router's connector registry, then `call(op, params)` is invoked.
pub(crate) async fn connector_handler(
    Path((name, op)): Path<(String, String)>,
    connector: Arc<dyn Connector>,
    body: CallbackBody,
) -> Json<Value> {
    let fut = AssertUnwindSafe(async move { connector.call(&op, body.params).await }).catch_unwind();
    match fut.await {
        Ok(Ok(output)) => Json(json!({ "output": output })),
        Ok(Err(err)) => {
            let (kind, message) = classify(&err);
            warn!(connector = %name, kind, %message, "callback connector failed");
            Json(json!({ "error": message, "kind": kind }))
        }
        Err(payload) => {
            let msg = panic_message(payload);
            warn!(connector = %name, panic = %msg, "callback connector panicked");
            Json(json!({ "error": format!("panic: {msg}"), "kind": "panic" }))
        }
    }
}

fn classify(err: &FlowError) -> (&'static str, String) {
    match err {
        FlowError::UnknownConnector(_) => ("unknown_connector", err.to_string()),
        FlowError::UnknownOperation { .. } => ("unknown_op", err.to_string()),
        FlowError::UnknownAction(_) => ("unknown_action", err.to_string()),
        FlowError::Connector(_) => ("connector_error", err.to_string()),
        FlowError::Persistence(_) => ("persistence", err.to_string()),
        FlowError::Internal(_) => ("internal", err.to_string()),
        _ => ("error", err.to_string()),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
