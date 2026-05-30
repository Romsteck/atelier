use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub service: LogService,
    pub level: LogLevel,
    pub category: LogCategory,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub source: LogSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSource {
    pub crate_name: String,
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogService {
    Atelier,
    Edge,
    Orchestrator,
    Netcore,
}

impl LogService {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Atelier => "atelier",
            Self::Edge => "edge",
            Self::Orchestrator => "orchestrator",
            Self::Netcore => "netcore",
        }
    }
}

impl fmt::Display for LogService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogService {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "atelier" => Ok(Self::Atelier),
            "edge" => Ok(Self::Edge),
            "orchestrator" => Ok(Self::Orchestrator),
            "netcore" => Ok(Self::Netcore),
            _ => Err(format!("unknown log service: {s}")),
        }
    }
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
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(format!("unknown log level: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogCategory {
    HttpRequest,
    IpcCall,
    System,
    Audit,
    Error,
    Task,
}

impl LogCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HttpRequest => "http_request",
            Self::IpcCall => "ipc_call",
            Self::System => "system",
            Self::Audit => "audit",
            Self::Error => "error",
            Self::Task => "task",
        }
    }
}

impl fmt::Display for LogCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogCategory {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http_request" => Ok(Self::HttpRequest),
            "ipc_call" => Ok(Self::IpcCall),
            "system" => Ok(Self::System),
            "audit" => Ok(Self::Audit),
            "error" => Ok(Self::Error),
            "task" => Ok(Self::Task),
            _ => Err(format!("unknown log category: {s}")),
        }
    }
}
