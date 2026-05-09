//! `http` connector — the generic HTTP request brick.
//!
//! Operations: `request`. Inputs:
//! - `url`     — required string
//! - `method`  — optional string (`GET`/`POST`/…), default `GET`
//! - `headers` — optional object of `name -> value`
//! - `body`    — optional JSON value (sent as `application/json` if present
//!               and no explicit `Content-Type` header)
//! - `timeout_ms` — optional number, default 30_000
//!
//! Output: `{ status, headers, body }` where `body` is parsed as JSON when
//! the response advertises a JSON content-type, otherwise returned as a
//! string.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
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

        let method = p.method.as_deref().unwrap_or("GET").to_uppercase();
        let timeout = Duration::from_millis(p.timeout_ms.unwrap_or(30_000));
        let mut req = self
            .client
            .request(
                method.parse().map_err(|e| FlowError::Connector(format!("invalid method: {e}")))?,
                &p.url,
            )
            .timeout(timeout);

        let mut has_ct = false;
        for (k, v) in &p.headers {
            if k.eq_ignore_ascii_case("content-type") { has_ct = true; }
            req = req.header(k, v);
        }
        if let Some(body) = p.body {
            if !has_ct {
                req = req.header("content-type", "application/json");
            }
            req = req.body(serde_json::to_vec(&body)
                .map_err(|e| FlowError::Connector(format!("serialize body: {e}")))?);
        }

        let resp = req.send().await
            .map_err(|e| FlowError::Connector(format!("http send: {e}")))?;

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
}
