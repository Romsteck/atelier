/// Surveillance IA per-app (code review + suggestions + sécurité via Codex CLI).
///
/// Runs **manuels uniquement** (déclenchés depuis l'UI ou MCP) — pas de
/// scheduler interne : un cron consommerait trop l'abonnement GPT+. Chaque run
/// passe par les gates throttle + budget + diff-aware, puis Codex. Le
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
    pub use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
}

pub mod codex;
pub mod config;
pub mod findings;
pub mod git_watcher;
pub mod gitutil;
pub mod memory;
pub mod migration;
pub mod runs;
pub mod service;

pub use codex::{CodexConfig, CodexRunner};
pub use config::{AppSurveillanceConfig, ConfigStore, ConfigUpdate};
pub use findings::{Finding, FindingFilter, FindingsStore, NewFinding};
pub use memory::{Memory, MemoryStore};
pub use runs::{Run, RunsStore};
pub use service::{AppMeta, SurveillanceConfig, SurveillanceService};

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

/// The three kinds of surveillance run. Note the naming asymmetry kept for
/// backward-compat with the DB: a run is `code_review`/`suggestions`/`security`
/// (plural for suggestions) while a finding is `code_review`/`suggestion`/`security`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    CodeReview,
    Suggestions,
    Security,
}

impl RunKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "code_review" => Some(Self::CodeReview),
            "suggestions" => Some(Self::Suggestions),
            "security" => Some(Self::Security),
            _ => None,
        }
    }

    /// Value stored in `surveillance_runs.kind`.
    pub fn run_kind(&self) -> &'static str {
        match self {
            Self::CodeReview => "code_review",
            Self::Suggestions => "suggestions",
            Self::Security => "security",
        }
    }

    /// Value stored in `findings.kind` (singular for suggestions).
    pub fn finding_kind(&self) -> &'static str {
        match self {
            Self::CodeReview => "code_review",
            Self::Suggestions => "suggestion",
            Self::Security => "security",
        }
    }

    /// Memory key tracking the last reviewed git SHA (diff-aware).
    pub fn sha_memory_key(&self) -> &'static str {
        match self {
            Self::CodeReview => "code_review_sha",
            Self::Suggestions => "suggestions_sha",
            Self::Security => "security_sha",
        }
    }

    /// Allowed finding categories for this kind. Codex is told to classify
    /// each finding into one of these; the server coerces anything else to
    /// `autres`. Keep in sync with the frontend `CATEGORIES` map.
    pub fn categories(&self) -> &'static [&'static str] {
        match self {
            Self::CodeReview => &[
                "bug",
                "architecture",
                "performance",
                "composants",
                "gestion_erreurs",
                "autres",
            ],
            Self::Suggestions => &["performance", "ux", "autres"],
            Self::Security => &["auth", "injection", "secrets", "exposition", "autres"],
        }
    }

    /// Coerce an arbitrary category string to a valid one for this kind,
    /// falling back to `autres`.
    pub fn normalize_category(&self, raw: Option<&str>) -> String {
        match raw {
            Some(c) if self.categories().contains(&c) => c.to_string(),
            _ => "autres".to_string(),
        }
    }

    /// Whether a `findings.kind` string belongs to this run kind. Used to
    /// resolve categories from a stored finding kind.
    pub fn from_finding_kind(s: &str) -> Option<Self> {
        match s {
            "code_review" => Some(Self::CodeReview),
            "suggestion" => Some(Self::Suggestions),
            "security" => Some(Self::Security),
            _ => None,
        }
    }
}
