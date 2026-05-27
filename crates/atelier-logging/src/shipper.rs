use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::types::{LogCategory, LogLevel, LogSource, RawIngestEntry};

/// Tracing layer used by external apps to ship logs to Atelier via HTTP batch.
pub struct HttpShipperLayer {
    tx: mpsc::UnboundedSender<RawIngestEntry>,
    service: String,
    app_slug: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HttpShipperConfig {
    pub ingest_url: String,           // e.g. "http://127.0.0.1:4100"
    pub bearer_token: String,
    pub service: String,              // e.g. "app-wallet"
    pub app_slug: Option<String>,     // e.g. Some("wallet")
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

impl HttpShipperLayer {
    /// Spawn the background batch task. Returns a Layer that channels events
    /// to it via an unbounded mpsc (best-effort; drop on send failure).
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

        Self { tx, service, app_slug }
    }
}

async fn ship(client: &reqwest::Client, url: &str, auth: &str, batch: Vec<RawIngestEntry>) {
    let res = client
        .post(url)
        .header("Authorization", auth)
        .json(&batch)
        .send()
        .await;
    if let Err(e) = res {
        // Best-effort: drop, never crash the app for logging.
        eprintln!("atelier-logging shipper: send failed: {e}");
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
            Some(serde_json::Value::Object(visitor.extra))
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
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let v = format!("{:?}", value);
        match field.name() {
            "message" => self.message = Some(v.trim_matches('"').to_string()),
            "category" => self.category = Some(v.trim_matches('"').to_string()),
            "request_id" => self.request_id = Some(v.trim_matches('"').to_string()),
            "user_id" => self.user_id = Some(v.trim_matches('"').to_string()),
            other => { self.extra.insert(other.to_string(), serde_json::Value::String(v)); }
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = Some(value.to_string()),
            "category" => self.category = Some(value.to_string()),
            "request_id" => self.request_id = Some(value.to_string()),
            "user_id" => self.user_id = Some(value.to_string()),
            other => { self.extra.insert(other.to_string(), serde_json::Value::String(value.to_string())); }
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.extra.insert(field.name().to_string(), serde_json::Value::Number(value.into()));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.extra.insert(field.name().to_string(), serde_json::Value::Number(value.into()));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.extra.insert(field.name().to_string(), serde_json::Value::Bool(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(n) = serde_json::Number::from_f64(value) {
            self.extra.insert(field.name().to_string(), serde_json::Value::Number(n));
        }
    }
}
