/// Surveillance IA per-app via le **Claude Agent SDK** (`claude.rs`). Chaque app
/// a TROIS scans (discriminés par `kind`) : `security` et `code_review`
/// (plateforme, fixes, prompts en code) + `business` (possédé par l'agent du
/// projet, défini en DONNÉES dans `app_scan`, vide par défaut).
///
/// Runs **manuels** (UI/MCP) **ou planifiés** : le *sweep* automatique
/// (`start_sweep`) passe app par app et lance les 3 scans simultanément, à la
/// demande ou via le scheduler interne (`sweep_scheduler`, config singleton
/// `sweep_schedule`). Chaque run passe par les gates cap (`MAX_OPEN_FINDINGS`,
/// par (app,kind)) + diff-aware (le sweep les **force**), puis le scan-agent. Le
/// git_watcher auto-résout les findings via les commits `fix(surveillance:N)`.
/// Le scan-agent écrit ses findings via le tool MCP `findings_upsert`
/// (`…/mcp?scope=surveillance`, whitelist read-only serveur).
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

pub mod claude;
pub mod findings;
pub mod git_watcher;
pub mod gitutil;
pub mod memory;
pub mod migration;
pub mod runner;
pub mod runs;
pub mod scandef;
pub mod service;
pub mod sweep_scheduler;

pub use claude::{ClaudeRunner, ClaudeScanConfig};
pub use findings::{Finding, FindingFilter, FindingsStore, NewFinding, OpenCountRow};
pub use runner::{ScanExec, build_prompt};
pub use memory::{Memory, MemoryStore};
pub use runs::{Run, RunsStore};
pub use scandef::{
    AppScanStore, Gate, ScanDef, BIZ_KIND, CODE_REVIEW_KIND, SECURITY_KIND, is_valid_kind, sha_key,
    watermark_key,
};
pub use service::{
    AppMeta, RunOutcome, ScanCell, SurveillanceConfig, SurveillanceService, SweepAppRow,
    SweepScanState, SweepSnapshot, SweepStatus,
};
pub use sweep_scheduler::{SweepSchedule, SweepScheduleStore};

/// Per-kind cap on OPEN findings. A new scan of a kind is skipped once the kind
/// already has this many open findings (the UI also disables the launch button),
/// and the prompt tells the scan-agent to report only the most important issues
/// within this budget. Single source of truth — no longer per-app configurable.
pub const MAX_OPEN_FINDINGS: i64 = 6;

/// Live event broadcast to the frontend over WebSocket whenever a finding or
/// run changes. Payload is intentionally minimal — the frontend reloads the
/// scope it's viewing on receipt (no per-field diffing needed).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SurveillanceEvent {
    /// "finding" | "run"
    pub kind: String,
    pub slug: String,
    /// e.g. "upsert" | "dismiss" | "resolve" | "delete" | "started" | "finished"
    pub action: String,
}

/// One line of scan-agent stdout, streamed live to the frontend while a run is
/// in progress. Ephemeral — never persisted; the UI shows it in a live console
/// that disappears once the run settles.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TranscriptLine {
    pub run_id: uuid::Uuid,
    pub slug: String,
    /// run kind: "security" | "code_review" | "business"
    pub kind: String,
    /// Monotonic line index within the run (lets the UI order/dedup).
    pub seq: u64,
    /// Wall-clock emit time (unix millis). Lets the UI compute per-step duration
    /// even when replaying a buffered transcript (where client arrival time is lost).
    pub ts: i64,
    pub line: String,
}

// No scan-kind enum: the three scans are modelled by `scandef::ScanDef` and
// discriminated by its `kind` field. `security`/`code_review` come from the
// hardcoded `ScanDef::security`/`ScanDef::code_review` constructors; `business`
// is loaded from the `app_scan` table (agent-owned, blank by default).
