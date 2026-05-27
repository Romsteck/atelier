use std::collections::BTreeMap;

use chrono::Utc;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::ingest::LogIngestService;
use crate::types::{LogCategory, LogEntryBuilder, LogLevel, LogSource};

/// Tracing layer that pushes events into the in-process LogIngestService.
/// Use this in Atelier itself; apps go through `HttpShipperLayer` instead.
pub struct LoggingLayer {
    ingest: LogIngestService,
    service: String,
    app_slug: Option<String>,
}

impl LoggingLayer {
    pub fn new(ingest: LogIngestService, service: impl Into<String>) -> Self {
        Self {
            ingest,
            service: service.into(),
            app_slug: None,
        }
    }

    pub fn with_app_slug(mut self, slug: impl Into<String>) -> Self {
        self.app_slug = Some(slug.into());
        self
    }
}

impl<S> tracing_subscriber::Layer<S> for LoggingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = LogLevel::from_tracing(meta.level());

        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        // Default message is the "message" field, else fall back to target.
        let message = visitor.message.unwrap_or_else(|| meta.target().to_string());

        let category = visitor
            .category
            .as_deref()
            .and_then(LogCategory::from_str)
            .unwrap_or(LogCategory::System);

        let request_id = visitor.request_id;
        let user_id = visitor.user_id;

        let mut fields_map = visitor.extra;
        if !fields_map.is_empty() {
            // serde_json::Map -> Value
        }
        let fields = if fields_map.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(std::mem::take(&mut fields_map)))
        };

        let source = LogSource {
            crate_name: Some(crate_from_target(meta.target())),
            module: Some(meta.target().to_string()),
            function: None,
            file: meta.file().map(|s| s.to_string()),
            line: meta.line(),
        };

        let builder = LogEntryBuilder {
            timestamp: Utc::now(),
            service: self.service.clone(),
            app_slug: self.app_slug.clone(),
            level,
            category,
            message,
            fields,
            request_id,
            user_id,
            source,
            app_version: option_env!("CARGO_PKG_VERSION").map(|s| s.to_string()),
            deploy_id: None,
        };

        self.ingest.push(builder);
    }
}

fn crate_from_target(target: &str) -> String {
    target.split("::").next().unwrap_or(target).to_string()
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
            other => {
                self.extra.insert(other.to_string(), serde_json::Value::String(v));
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
                self.extra.insert(other.to_string(), serde_json::Value::String(value.to_string()));
            }
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

// Touch unused import (BTreeMap is reserved for future structured spans).
#[allow(dead_code)]
fn _touch(_: BTreeMap<String, String>) {}
