//! `FlowdClient` — Atelier API → `hr-flowd` (loopback, port 4002).
//!
//! Used by the write-side endpoints (`/api/apps/{slug}/flows/{name}/run`,
//! `/api/apps/{slug}/flows/_runs/{id}/replay`) and by the future MCP tools
//! `flow.run` / `flow.replay` once they migrate from `hr-orchestrator` legacy
//! to Atelier (Phase 4 plan §E).
//!
//! Configuration (read at construction time):
//! * `HR_FLOWD_URL`             default `http://127.0.0.1:4002`
//! * `ATELIER_FLOW_TOKEN`       shared bearer with the daemon (required)
//! * `HR_FLOWD_TIMEOUT_MS`      default 600_000
//!
//! Returns a structured error rather than panicking when the env is missing,
//! so apps without the daemon provisioned (dev sandbox) keep booting and only
//! the flow routes return 503.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct FlowdClient {
    base_url: String,
    bearer: String,
    http: Client,
    timeout: Duration,
}

#[derive(Debug, Error)]
pub enum FlowdError {
    #[error("ATELIER_FLOW_TOKEN must be set")]
    MissingToken,
    #[error("hr-flowd unreachable: {0}")]
    Transport(reqwest::Error),
    #[error("hr-flowd returned {status}: {body}")]
    Upstream {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("hr-flowd response parse: {0}")]
    Parse(reqwest::Error),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunWire {
    pub run_id: String,
    pub flow_name: String,
    pub status: String,
    #[serde(default)]
    pub output: Option<Value>,
    #[serde(default)]
    pub error: Option<RunErrorWire>,
    pub duration_ms: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RunErrorWire {
    pub step_id: String,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReloadReport {
    pub apps_loaded: usize,
    pub flows_loaded: usize,
}

impl FlowdClient {
    pub fn from_env() -> Result<Self, FlowdError> {
        let base_url =
            std::env::var("HR_FLOWD_URL").unwrap_or_else(|_| "http://127.0.0.1:4002".to_string());
        let bearer = std::env::var("ATELIER_FLOW_TOKEN").map_err(|_| FlowdError::MissingToken)?;
        let timeout_ms = std::env::var("HR_FLOWD_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(600_000);
        Ok(Self {
            base_url,
            bearer,
            http: Client::new(),
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    pub async fn run(
        &self,
        slug: &str,
        flow_name: &str,
        input: Value,
    ) -> Result<RunWire, FlowdError> {
        let url = format!("{}/v1/runs", self.base_url.trim_end_matches('/'));
        let body = json!({
            "slug": slug,
            "flow_name": flow_name,
            "input": input,
            "trigger": "manual",
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .timeout(self.timeout)
            .json(&body)
            .send()
            .await
            .map_err(FlowdError::Transport)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(FlowdError::Upstream { status, body });
        }
        resp.json::<RunWire>().await.map_err(FlowdError::Parse)
    }

    pub async fn replay(&self, slug: &str, run_id: &str) -> Result<RunWire, FlowdError> {
        let url = format!(
            "{}/v1/runs/{}/replay?slug={}",
            self.base_url.trim_end_matches('/'),
            run_id,
            slug
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(FlowdError::Transport)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(FlowdError::Upstream { status, body });
        }
        resp.json::<RunWire>().await.map_err(FlowdError::Parse)
    }

    pub async fn reload(&self, slug: Option<&str>) -> Result<ReloadReport, FlowdError> {
        let mut url = format!("{}/v1/_admin/reload", self.base_url.trim_end_matches('/'));
        if let Some(slug) = slug {
            url.push_str(&format!("?slug={slug}"));
        }
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.bearer)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(FlowdError::Transport)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(FlowdError::Upstream { status, body });
        }
        resp.json::<ReloadReport>().await.map_err(FlowdError::Parse)
    }
}
