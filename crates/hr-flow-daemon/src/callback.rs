//! Outbound callback bridge — daemon → app.
//!
//! When a step has `kind = "action"` referencing a name not implemented by the
//! daemon, or `kind = "connector"` with a connector outside the managed list,
//! the executor calls a closure / connector defined here that POSTs the work
//! over HTTP to the owning app.
//!
//! Contract (cf. plan-hr-flowd.md §D):
//!   POST  {callback_url}/_flow/action/{name}
//!   POST  {callback_url}/_flow/connector/{name}/{op}
//!   Authorization: Bearer <flow_callback_token>
//!   X-HomeRoute-Flow: 1
//!   X-Flow-Deadline-Ms: <timeout>
//!   { "run_id", "step_id", "input" | "params", "params"? }
//!
//! ⚠ `run_id` / `step_id` are carried in the body for forward-compat but are
//! currently **empty** — the executor does not yet thread run context down to
//! connector / action calls. The `X-Flow-Run-Id` / `X-Flow-Step-Id` headers
//! are **not** sent. Apps must not rely on either for correlation yet.
//!
//! Response:
//!   200 { "output": <Value> }                     — success
//!   any { "error": "<msg>", "kind": "<opt>" }    — business error
//!   timeout / 5xx / refused                       — synthesised StepFailed
//!
//! Panics in the app handler are caught by `hr-flow-callback` (apps side) and
//! turned into a `{ "error": "panic: ...", "kind": "panic" }` body — the
//! daemon does not distinguish them from other business errors at this layer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hr_flow::{Connector, FlowError, FlowResult};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::registry::AppEntry;

/// Daemon-side connector that forwards every `op` to the app's
/// `/_flow/connector/{name}/{op}` endpoint.
pub struct RemoteConnector {
    name: String,
    slug: String,
    callback_url: String,
    bearer: String,
    http: Client,
    timeout_ms: u64,
}

impl RemoteConnector {
    pub fn new(
        name: impl Into<String>,
        app: &AppEntry,
        http: Client,
        timeout_ms: u64,
    ) -> FlowResult<Arc<dyn Connector>> {
        let callback_url = app
            .callback_url
            .clone()
            .ok_or_else(|| FlowError::Internal(format!("app {} has no flow_callback_url", app.slug)))?;
        let bearer = app
            .callback_token
            .clone()
            .ok_or_else(|| FlowError::Internal(format!("app {} has no flow_callback_token", app.slug)))?;
        Ok(Arc::new(Self {
            name: name.into(),
            slug: app.slug.clone(),
            callback_url,
            bearer,
            http,
            timeout_ms,
        }))
    }
}

#[async_trait]
impl Connector for RemoteConnector {
    fn name(&self) -> &str {
        &self.name
    }

    async fn call(&self, op: &str, params: Value) -> FlowResult<Value> {
        let url = format!(
            "{}/_flow/connector/{}/{}",
            self.callback_url.trim_end_matches('/'),
            self.name,
            op
        );
        let body = serde_json::json!({
            "run_id": "",
            "step_id": "",
            "input": Value::Null,
            "params": params,
        });
        post_callback(
            &self.http,
            &self.slug,
            &url,
            &self.bearer,
            self.timeout_ms,
            body,
        )
        .await
    }
}

/// Build a closure usable as a `register_action` handler that forwards the
/// step `input` to the app's `/_flow/action/{name}` endpoint.
pub fn remote_action(
    name: String,
    app: &AppEntry,
    http: Client,
    timeout_ms: u64,
) -> FlowResult<
    impl Fn(Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = FlowResult<Value>> + Send>>
        + Clone
        + Send
        + Sync
        + 'static,
> {
    let callback_url = app
        .callback_url
        .clone()
        .ok_or_else(|| FlowError::Internal(format!("app {} has no flow_callback_url", app.slug)))?;
    let bearer = app
        .callback_token
        .clone()
        .ok_or_else(|| FlowError::Internal(format!("app {} has no flow_callback_token", app.slug)))?;
    let slug = app.slug.clone();
    let url = format!(
        "{}/_flow/action/{}",
        callback_url.trim_end_matches('/'),
        name
    );
    Ok(move |input: Value| {
        let http = http.clone();
        let url = url.clone();
        let bearer = bearer.clone();
        let slug = slug.clone();
        Box::pin(async move {
            let body = serde_json::json!({
                "run_id": "",
                "step_id": "",
                "input": input,
                "params": Value::Null,
            });
            post_callback(&http, &slug, &url, &bearer, timeout_ms, body).await
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = FlowResult<Value>> + Send>>
    })
}

#[derive(Deserialize)]
struct CallbackResponse {
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

async fn post_callback(
    http: &Client,
    slug: &str,
    url: &str,
    bearer: &str,
    timeout_ms: u64,
    body: Value,
) -> FlowResult<Value> {
    let req = http
        .post(url)
        .bearer_auth(bearer)
        .header("X-HomeRoute-Flow", "1")
        .header("X-Flow-Deadline-Ms", timeout_ms.to_string())
        .timeout(Duration::from_millis(timeout_ms))
        .json(&body);

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            warn!(slug, url, "callback timeout");
            return Err(FlowError::StepFailed {
                step_id: String::new(),
                message: format!("callback_timeout: {url}"),
            });
        }
        Err(e) if e.is_connect() => {
            warn!(slug, url, "callback unreachable");
            return Err(FlowError::StepFailed {
                step_id: String::new(),
                message: format!("callback_unreachable: {e}"),
            });
        }
        Err(e) => {
            warn!(slug, url, ?e, "callback http error");
            return Err(FlowError::StepFailed {
                step_id: String::new(),
                message: format!("callback_http: {e}"),
            });
        }
    };

    let status = resp.status();
    if status.is_server_error() {
        return Err(FlowError::StepFailed {
            step_id: String::new(),
            message: format!("callback_5xx: {} {url}", status.as_u16()),
        });
    }

    let parsed: CallbackResponse = resp.json().await.map_err(|e| FlowError::StepFailed {
        step_id: String::new(),
        message: format!("callback_response_parse: {e}"),
    })?;

    if let Some(err) = parsed.error {
        return Err(FlowError::StepFailed {
            step_id: String::new(),
            message: match parsed.kind {
                Some(kind) => format!("{kind}: {err}"),
                None => err,
            },
        });
    }
    Ok(parsed.output.unwrap_or(Value::Null))
}
