//! App-domain DTOs returned by IPC/MCP handlers.
//!
//! Previously lived under `hr_ipc::types` (deleted there 2026-05-27 once
//! the apps subsystem moved to Atelier). The shapes mirror
//! `hr_apps::types::Application` & co but stay in atelier-api to avoid a
//! crate-dep cycle (`hr-apps` -> `hr-ipc` -> `hr-apps`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Application summary returned by `app.list` / `app.get`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationDto {
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub stack: String,
    #[serde(default)]
    pub has_db: bool,
    #[serde(default)]
    pub visibility: String,
    pub domain: String,
    pub port: u16,
    pub run_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_artefact: Option<String>,
    pub health_path: String,
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,
    #[serde(default)]
    pub state: String,
    /// `"postgres-dataverse"` — the only supported backend post-migration.
    /// Kept as `String` (not enum) so a future backend addition stays
    /// payload-compatible.
    #[serde(default = "default_db_backend")]
    pub db_backend: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_db_backend() -> String {
    "postgres-dataverse".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppListData {
    pub apps: Vec<ApplicationDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatusData {
    pub slug: String,
    pub pid: Option<u32>,
    pub state: String,
    pub port: u16,
    pub uptime_secs: u64,
    pub restart_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppLogsData {
    pub slug: String,
    pub logs: Vec<AppLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDbTableColumn {
    pub name: String,
    pub field_type: String,
    pub required: bool,
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula_expression: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDbRelation {
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub display_column: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDbTableSchema {
    pub name: String,
    pub columns: Vec<AppDbTableColumn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<AppDbRelation>,
    pub row_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDbTablesData {
    pub tables: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDbQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Value>,
    pub total: u64,
}
