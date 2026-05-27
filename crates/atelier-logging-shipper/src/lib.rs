//! Thin shipper that batches `tracing` events to Atelier's `/api/logs/ingest`.
//!
//! Stand-alone crate intended for the 6 HomeRoute apps — it does NOT pull
//! sqlx or any DB-side dep (that lives in `atelier-logging` for the core
//! ingest service). Apps add this crate as a path-dep and call
//! `HttpShipperLayer::from_env(service, app_slug)` at startup.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

// ────────────────────────────────────────────────────────────────────
// Wire types (must match crates/atelier-logging/src/types.rs)
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogSource {
    #[serde(default)]
    pub crate_name: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub function: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn from_tracing(level: &tracing::Level) -> Self {
        match *level {
            tracing::Level::TRACE => LogLevel::Trace,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::ERROR => LogLevel::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogCategory {
    HttpRequest,
    DvMutation,
    Business,
    ExternalCall,
    System,
    IpcCall,
    Audit,
    Task,
}

impl LogCategory {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "http_request" => Some(Self::HttpRequest),
            "dv_mutation" => Some(Self::DvMutation),
            "business" => Some(Self::Business),
            "external_call" => Some(Self::ExternalCall),
            "system" => Some(Self::System),
            "ipc_call" => Some(Self::IpcCall),
            "audit" => Some(Self::Audit),
            "task" => Some(Self::Task),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawIngestEntry {
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
    pub service: String,
    #[serde(default)]
    pub app_slug: Option<String>,
    pub level: LogLevel,
    #[serde(default)]
    pub category: Option<LogCategory>,
    pub message: String,
    #[serde(default)]
    pub fields: Option<Value>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub source: Option<LogSource>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub deploy_id: Option<String>,
}

// ────────────────────────────────────────────────────────────────────
// HttpShipperLayer
// ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HttpShipperConfig {
    pub ingest_url: String,
    pub bearer_token: String,
    pub service: String,
    pub app_slug: Option<String>,
    pub batch_size: usize,
    pub batch_interval: Duration,
}

impl Default for HttpShipperConfig {
    fn default() -> Self {
        Self {
            ingest_url: String::new(),
            bearer_token: String::new(),
            service: String::new(),
            app_slug: None,
            batch_size: 200,
            batch_interval: Duration::from_secs(5),
        }
    }
}

pub struct HttpShipperLayer {
    tx: mpsc::UnboundedSender<RawIngestEntry>,
    service: String,
    app_slug: Option<String>,
}

impl HttpShipperLayer {
    /// Spawn the background batch task. Returns a Layer that channels events
    /// to it via an unbounded mpsc (best-effort; drop on send failure).
    /// Requires an active Tokio runtime when called.
    pub fn start(cfg: HttpShipperConfig) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<RawIngestEntry>();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        let url = format!("{}/api/logs/ingest", cfg.ingest_url.trim_end_matches('/'));
        let auth = format!("Bearer {}", cfg.bearer_token);

        let batch_size = cfg.batch_size;
        let batch_interval = cfg.batch_interval;
        let service = cfg.service.clone();
        let app_slug = cfg.app_slug.clone();

        tokio::spawn(async move {
            let mut buf: Vec<RawIngestEntry> = Vec::with_capacity(batch_size);
            let mut tick = tokio::time::interval(batch_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(m) => buf.push(m),
                            None => break,
                        }
                        if buf.len() >= batch_size {
                            let batch = std::mem::take(&mut buf);
                            ship(&client, &url, &auth, batch).await;
                        }
                    }
                    _ = tick.tick() => {
                        if !buf.is_empty() {
                            let batch = std::mem::take(&mut buf);
                            ship(&client, &url, &auth, batch).await;
                        }
                    }
                }
            }
            if !buf.is_empty() {
                ship(&client, &url, &auth, buf).await;
            }
        });

        Self {
            tx,
            service,
            app_slug,
        }
    }

    /// Construct from environment. Returns `None` when `ATELIER_INGEST_URL`
    /// or `ATELIER_LOGS_TOKEN` is missing — the app should fall back to
    /// stdout-only logging in that case.
    pub fn from_env(service: impl Into<String>, app_slug: Option<String>) -> Option<Self> {
        let ingest_url = std::env::var("ATELIER_INGEST_URL").ok()?;
        let bearer_token = std::env::var("ATELIER_LOGS_TOKEN").ok()?;
        if ingest_url.is_empty() || bearer_token.is_empty() {
            return None;
        }
        Some(Self::start(HttpShipperConfig {
            ingest_url,
            bearer_token,
            service: service.into(),
            app_slug,
            ..Default::default()
        }))
    }
}

async fn ship(client: &reqwest::Client, url: &str, auth: &str, batch: Vec<RawIngestEntry>) {
    if let Err(e) = client
        .post(url)
        .header("Authorization", auth)
        .json(&batch)
        .send()
        .await
    {
        eprintln!("atelier-logging-shipper: send failed: {e}");
    }
}

impl<S> tracing_subscriber::Layer<S> for HttpShipperLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = LogLevel::from_tracing(meta.level());
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let message = visitor.message.unwrap_or_else(|| meta.target().to_string());
        let category = visitor.category.and_then(|s| LogCategory::from_str(&s));
        let extra = if visitor.extra.is_empty() {
            None
        } else {
            Some(Value::Object(visitor.extra))
        };
        let entry = RawIngestEntry {
            timestamp: Some(Utc::now()),
            service: self.service.clone(),
            app_slug: self.app_slug.clone(),
            level,
            category,
            message,
            fields: extra,
            request_id: visitor.request_id,
            user_id: visitor.user_id,
            source: Some(LogSource {
                crate_name: Some(meta.target().split("::").next().unwrap_or("").to_string()),
                module: Some(meta.target().to_string()),
                function: None,
                file: meta.file().map(|s| s.to_string()),
                line: meta.line(),
            }),
            app_version: option_env!("CARGO_PKG_VERSION").map(|s| s.to_string()),
            deploy_id: None,
        };
        let _ = self.tx.send(entry);
    }
}

#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    category: Option<String>,
    request_id: Option<String>,
    user_id: Option<String>,
    extra: serde_json::Map<String, Value>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let v = format!("{:?}", value);
        match field.name() {
            "message" => self.message = Some(v.trim_matches('"').to_string()),
            "category" => self.category = Some(v.trim_matches('"').to_string()),
            "request_id" => self.request_id = Some(v.trim_matches('"').to_string()),
            "user_id" => self.user_id = Some(v.trim_matches('"').to_string()),
            other => {
                self.extra
                    .insert(other.to_string(), Value::String(v));
            }
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = Some(value.to_string()),
            "category" => self.category = Some(value.to_string()),
            "request_id" => self.request_id = Some(value.to_string()),
            "user_id" => self.user_id = Some(value.to_string()),
            other => {
                self.extra
                    .insert(other.to_string(), Value::String(value.to_string()));
            }
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.extra
            .insert(field.name().to_string(), Value::Number(value.into()));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.extra
            .insert(field.name().to_string(), Value::Number(value.into()));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.extra
            .insert(field.name().to_string(), Value::Bool(value));
    }
}
