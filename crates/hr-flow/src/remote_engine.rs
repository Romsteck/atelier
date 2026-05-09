//! HTTP client for `hr-flowd`. Implements the `Engine` trait by POSTing
//! `/v1/runs`, `/v1/runs/{id}/replay` to the daemon. All actions and
//! connectors live in the daemon registry — apps using `RemoteEngine` only
//! need to expose their custom code over the callback router.
//!
//! Configuration via env vars (read at construction time):
//! * `HR_FLOWD_URL`      — base URL (default `http://127.0.0.1:4002`)
//! * `HR_FLOWD_TOKEN`    — shared bearer token, mirrored from the daemon's
//!                          `ATELIER_FLOW_TOKEN` (loopback only)
//! * `HR_FLOWD_TIMEOUT_MS` — total request timeout (default 600_000)

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use crate::engine::{RunError as RunErrorOut, RunResult, RunStatus};
use crate::engine_trait::Engine;
use crate::error::{FlowError, FlowResult};

pub struct RemoteEngine {
    base_url: String,
    bearer: String,
    slug: String,
    http: Client,
    timeout: Duration,
}

impl RemoteEngine {
    pub fn new(base_url: impl Into<String>, bearer: impl Into<String>, slug: impl Into<String>) -> Self {
        Self::new_with_client(base_url, bearer, slug, Client::new())
    }

    pub fn new_with_client(
        base_url: impl Into<String>,
        bearer: impl Into<String>,
        slug: impl Into<String>,
        http: Client,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            bearer: bearer.into(),
            slug: slug.into(),
            http,
            timeout: Duration::from_millis(600_000),
        }
    }

    pub fn from_env(slug: impl Into<String>) -> FlowResult<Self> {
        let base_url =
            std::env::var("HR_FLOWD_URL").unwrap_or_else(|_| "http://127.0.0.1:4002".to_string());
        let bearer = std::env::var("HR_FLOWD_TOKEN").map_err(|_| {
            FlowError::Internal("HR_FLOWD_TOKEN must be set when HR_FLOW_BACKEND=remote".into())
        })?;
        let timeout_ms = std::env::var("HR_FLOWD_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(600_000);
        Ok(Self {
            base_url,
            bearer,
            slug: slug.into(),
            http: Client::new(),
            timeout: Duration::from_millis(timeout_ms),
        })
    }
}

#[derive(Debug, Deserialize)]
struct RunWire {
    run_id: String,
    flow_name: String,
    status: String,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    error: Option<RunErrorWire>,
    duration_ms: i64,
}

#[derive(Debug, Deserialize)]
struct RunErrorWire {
    step_id: String,
    message: String,
}

impl RunWire {
    fn into_result(self, input: Value) -> RunResult {
        let status = if self.status == "success" {
            RunStatus::Success
        } else {
            RunStatus::Failed
        };
        RunResult {
            run_id: self.run_id,
            flow_name: self.flow_name,
            status,
            output: self.output,
            error: self.error.map(|e| RunErrorOut {
                step_id: e.step_id,
                message: e.message,
                input: Some(input),
            }),
            duration_ms: self.duration_ms,
        }
    }
}

#[async_trait]
impl Engine for RemoteEngine {
    async fn run(&self, name: &str, input: Value) -> FlowResult<RunResult> {
        self.run_with_trigger(name, input, "manual").await
    }

    async fn run_with_trigger(
        &self,
        name: &str,
        input: Value,
        trigger: &str,
    ) -> FlowResult<RunResult> {
        let url = format!("{}/v1/runs", self.base_url.trim_end_matches('/'));
        let body = json!({
            "slug": self.slug,
            "flow_name": name,
            "input": input,
            "trigger": trigger,
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| FlowError::Internal(format!("hr-flowd POST {url}: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(FlowError::Internal(format!(
                "hr-flowd run failed: {status} {text}"
            )));
        }
        let wire: RunWire = resp
            .json()
            .await
            .map_err(|e| FlowError::Internal(format!("hr-flowd response parse: {e}")))?;
        Ok(wire.into_result(input))
    }

    async fn replay(&self, run_id: &str) -> FlowResult<RunResult> {
        let url = format!(
            "{}/v1/runs/{}/replay?slug={}",
            self.base_url.trim_end_matches('/'),
            run_id,
            self.slug
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| FlowError::Internal(format!("hr-flowd POST {url}: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(FlowError::Internal(format!(
                "hr-flowd replay failed: {status} {text}"
            )));
        }
        let wire: RunWire = resp
            .json()
            .await
            .map_err(|e| FlowError::Internal(format!("hr-flowd response parse: {e}")))?;
        Ok(wire.into_result(Value::Null))
    }
}
