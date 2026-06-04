/// Surveillance IA per-app (code review + suggestions + sécurité via Codex CLI).
///
/// Runs **manuels uniquement** (déclenchés depuis l'UI ou MCP) — pas de
/// scheduler interne : un cron consommerait trop l'abonnement GPT+. Chaque run
/// passe par les gates cap (`MAX_OPEN_FINDINGS`) + diff-aware, puis Codex. Le
/// git_watcher auto-résout les findings via les commits `fix(surveillance:N)`.
/// Inert tant que le binaire `codex` n'est pas installé — un run renvoie alors
/// une erreur propre.
#[allow(unused_imports)]
pub(crate) mod sqlx {
    pub use sqlx_core::Error;
    pub use sqlx_core::executor::Executor;
    pub use sqlx_core::pool::Pool;
    pub use sqlx_core::query::query;
    pub use sqlx_core::query_as::query_as;
    pub use sqlx_core::query_scalar::query_scalar;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::row::Row;
    pub use sqlx_core::sql_str::AssertSqlSafe;
    pub use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
}

pub mod codex;
pub mod findings;
pub mod git_watcher;
pub mod gitutil;
pub mod memory;
pub mod migration;
pub mod runs;
pub mod scandef;
pub mod service;

pub use codex::{CodexConfig, CodexRunner};
pub use findings::{Finding, FindingFilter, FindingsStore, NewFinding};
pub use memory::{Memory, MemoryStore};
pub use runs::{Run, RunsStore};
pub use scandef::{AppScanStore, Gate, ScanDef, SCAN_KIND};
pub use service::{AppMeta, SurveillanceConfig, SurveillanceService};

/// Per-kind cap on OPEN findings. A new scan of a kind is skipped once the kind
/// already has this many open findings (the UI also disables the launch button),
/// and the prompt tells Codex to report only the most important issues within
/// this budget. Single source of truth — no longer per-app configurable.
pub const MAX_OPEN_FINDINGS: i64 = 6;

/// Live event broadcast to the frontend over WebSocket whenever a finding or
/// run changes. Payload is intentionally minimal — the frontend reloads the
/// scope it's viewing on receipt (no per-field diffing needed).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SurveillanceEvent {
    /// "finding" | "run"
    pub kind: String,
    pub slug: String,
    /// e.g. "upsert" | "dismiss" | "resolve" | "started" | "finished"
    pub action: String,
}

/// One line of Codex stdout, streamed live to the frontend while a run is in
/// progress. Ephemeral — never persisted; the UI shows it in a live console
/// that disappears once the run settles.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TranscriptLine {
    pub run_id: uuid::Uuid,
    pub slug: String,
    /// run_kind: "code_review" | "suggestions" | "security"
    pub kind: String,
    /// Monotonic line index within the run (lets the UI order/dedup).
    pub seq: u64,
    pub line: String,
}

// The scan kind enum was removed: every app now has exactly ONE scan, defined as
// DATA in the `app_scan` table (label/prompt/cadence/gate/categories), owned by
// the project's agent. See `scandef::ScanDef` + `scandef::SCAN_KIND`.
