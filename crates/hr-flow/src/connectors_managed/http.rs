//! `http` connector — the generic HTTP request brick.
//!
//! Operations: `request`. Inputs:
//! - `url`     — required string
//! - `method`  — optional string (`GET`/`POST`/…), default `GET`
//! - `headers` — optional object of `name -> value`
//! - `body`    — optional JSON value (sent as `application/json` if present
//!               and no explicit `Content-Type` header)
//! - `timeout_ms`       — optional number, default 30_000
//! - `max_retries`      — optional number, default 0. When > 0 the request is
//!   retried on 429 / 5xx responses and on network errors.
//! - `retry_backoff_ms` — optional number, default 1000. Base for the
//!   exponential backoff between attempts; a numeric `Retry-After` response
//!   header takes precedence when present.
//!
//! Output: `{ status, headers, body }` where `body` is parsed as JSON when
//! the response advertises a JSON content-type, otherwise returned as a
//! string. A non-2xx response that exhausts the retry budget is still
//! returned as a normal output (with its `status`) — the connector only
//! errors on a transport failure.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Method, Response};
use serde::Deserialize;
use serde_json::Value;

use crate::connector::Connector;
use crate::error::{FlowError, FlowResult};

pub struct HttpConnector {
    client: Client,
}

impl HttpConnector {
    pub fn new() -> Self {
        let client = Client::builder()
            .build()
            .expect("reqwest client");
        Self { client }
    }
}

impl Default for HttpConnector {
    fn default() -> Self { Self::new() }
}

#[derive(Deserialize)]
struct RequestParams {
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Retry budget for transient failures (429 / 5xx / network). 0 disables.
    #[serde(default)]
    max_retries: u32,
    /// Exponential-backoff base in ms (doubles each attempt). A numeric
    /// `Retry-After` response header overrides it when present.
    #[serde(default = "default_retry_backoff")]
    retry_backoff_ms: u64,
}

fn default_retry_backoff() -> u64 { 1000 }

/// Statuses worth retrying: provider rate limit + transient gateway errors.
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Delay before the next attempt. Honors a numeric `Retry-After`
/// (delta-seconds) header when the provider sends one, otherwise applies
/// exponential backoff with light jitter. Capped at 60s either way.
fn next_delay(resp: Option<&Response>, attempt: u32, base_ms: u64) -> Duration {
    const CAP_MS: u64 = 60_000;
    if let Some(r) = resp {
        if let Some(secs) = r
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
        {
            return Duration::from_millis(secs.saturating_mul(1000).min(CAP_MS));
        }
    }
    let backoff = base_ms.saturating_mul(1u64 << attempt.min(16)).min(CAP_MS);
    // Cheap jitter (~±12%) without pulling in `rand` — derive it from the clock.
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
        % (backoff / 8 + 1);
    Duration::from_millis((backoff + jitter).min(CAP_MS))
}

#[async_trait]
impl Connector for HttpConnector {
    fn name(&self) -> &str { "http" }

    async fn call(&self, op: &str, params: Value) -> FlowResult<Value> {
        if op != "request" {
            return Err(FlowError::UnknownOperation {
                connector: "http".into(),
                op: op.to_string(),
            });
        }
        let p: RequestParams = serde_json::from_value(params)
            .map_err(|e| FlowError::Connector(format!("invalid http params: {e}")))?;

        let method: Method = p.method.as_deref().unwrap_or("GET").to_uppercase()
            .parse()
            .map_err(|e| FlowError::Connector(format!("invalid method: {e}")))?;
        let timeout = Duration::from_millis(p.timeout_ms.unwrap_or(30_000));

        let mut attempt: u32 = 0;
        loop {
            // Rebuild the request each attempt — a RequestBuilder is consumed
            // by `send()` and not reusable.
            let mut req = self
                .client
                .request(method.clone(), &p.url)
                .timeout(timeout);

            let mut has_ct = false;
            for (k, v) in &p.headers {
                if k.eq_ignore_ascii_case("content-type") { has_ct = true; }
                req = req.header(k, v);
            }
            if let Some(body) = &p.body {
                if !has_ct {
                    req = req.header("content-type", "application/json");
                }
                req = req.body(serde_json::to_vec(body)
                    .map_err(|e| FlowError::Connector(format!("serialize body: {e}")))?);
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if attempt < p.max_retries && is_retryable_status(status) {
                        let delay = next_delay(Some(&resp), attempt, p.retry_backoff_ms);
                        tracing::warn!(
                            url = %p.url, status, attempt,
                            delay_ms = delay.as_millis() as u64,
                            "http connector retrying after transient status",
                        );
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return build_output(resp).await;
                }
                Err(e) => {
                    if attempt < p.max_retries {
                        let delay = next_delay(None, attempt, p.retry_backoff_ms);
                        tracing::warn!(
                            url = %p.url, error = %e, attempt,
                            delay_ms = delay.as_millis() as u64,
                            "http connector retrying after network error",
                        );
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(FlowError::Connector(format!("http send: {e}")));
                }
            }
        }
    }
}

/// Materialize a `reqwest::Response` into the connector's `{ status, headers,
/// body }` output shape.
async fn build_output(resp: Response) -> FlowResult<Value> {
    let status = resp.status().as_u16();
    let mut headers_out = serde_json::Map::new();
    for (k, v) in resp.headers().iter() {
        if let Ok(s) = v.to_str() {
            headers_out.insert(k.as_str().to_string(), Value::String(s.to_string()));
        }
    }

    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.bytes().await
        .map_err(|e| FlowError::Connector(format!("http body: {e}")))?;
    let body_value: Value = if ct.contains("application/json") || ct.contains("+json") {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
    } else {
        Value::String(String::from_utf8_lossy(&bytes).into())
    };

    Ok(serde_json::json!({
        "status": status,
        "headers": Value::Object(headers_out),
        "body": body_value,
    }))
}
