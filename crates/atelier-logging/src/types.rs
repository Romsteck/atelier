use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub service: String,
    pub app_slug: Option<String>,
    pub level: LogLevel,
    pub category: LogCategory,
    pub message: String,
    pub fields: Option<Value>,
    pub request_id: Option<String>,
    pub user_id: Option<String>,
    pub source: LogSource,
    pub app_version: Option<String>,
    pub deploy_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogSource {
    pub crate_name: Option<String>,
    pub module: Option<String>,
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "trace" => Some(LogLevel::Trace),
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" | "warning" => Some(LogLevel::Warn),
            "error" | "err" => Some(LogLevel::Error),
            _ => None,
        }
    }

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
    pub fn as_str(&self) -> &'static str {
        match self {
            LogCategory::HttpRequest => "http_request",
            LogCategory::DvMutation => "dv_mutation",
            LogCategory::Business => "business",
            LogCategory::ExternalCall => "external_call",
            LogCategory::System => "system",
            LogCategory::IpcCall => "ipc_call",
            LogCategory::Audit => "audit",
            LogCategory::Task => "task",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "http_request" => Some(LogCategory::HttpRequest),
            "dv_mutation" => Some(LogCategory::DvMutation),
            "business" => Some(LogCategory::Business),
            "external_call" => Some(LogCategory::ExternalCall),
            "system" => Some(LogCategory::System),
            "ipc_call" => Some(LogCategory::IpcCall),
            "audit" => Some(LogCategory::Audit),
            "task" => Some(LogCategory::Task),
            _ => None,
        }
    }
}

/// In-process push (used by the Layer). No id yet — the ingest service assigns it.
#[derive(Debug, Clone)]
pub struct LogEntryBuilder {
    pub timestamp: DateTime<Utc>,
    pub service: String,
    pub app_slug: Option<String>,
    pub level: LogLevel,
    pub category: LogCategory,
    pub message: String,
    pub fields: Option<Value>,
    pub request_id: Option<String>,
    pub user_id: Option<String>,
    pub source: LogSource,
    pub app_version: Option<String>,
    pub deploy_id: Option<String>,
}

/// Payload for the HTTP ingest endpoint. Apps (Rust shipper or NextJS) POST
/// `Vec<RawIngestEntry>` to `/api/logs/ingest`.
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

impl RawIngestEntry {
    pub fn into_builder(self, default_service: &str, server_ts: DateTime<Utc>) -> LogEntryBuilder {
        LogEntryBuilder {
            timestamp: self.timestamp.unwrap_or(server_ts),
            service: if self.service.is_empty() { default_service.to_string() } else { self.service },
            app_slug: self.app_slug,
            level: self.level,
            category: self.category.unwrap_or(LogCategory::System),
            message: truncate(self.message, 8192),
            fields: self.fields.map(truncate_json),
            request_id: self.request_id,
            user_id: self.user_id,
            source: self.source.unwrap_or_default(),
            app_version: self.app_version,
            deploy_id: self.deploy_id,
        }
    }
}

fn truncate(s: String, max: usize) -> String {
    if s.len() <= max { s } else { let mut t = s; t.truncate(max); t }
}

fn truncate_json(v: Value) -> Value {
    // Cap serialized size at 32 KB to prevent runaway JSONB payloads.
    const MAX: usize = 32 * 1024;
    match serde_json::to_string(&v) {
        Ok(s) if s.len() <= MAX => v,
        Ok(_) => Value::String(format!("<truncated payload exceeded {MAX} bytes>")),
        Err(_) => Value::Null,
    }
}
