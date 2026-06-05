//! Scan definitions. Each app has THREE scans, discriminated by `kind`:
//! - `security` and `code_review` are FIXED platform scans (prompt/categories/gate
//!   are code, via `ScanDef::security` / `ScanDef::code_review`); they run for every
//!   app and are not editable by the project's agent.
//! - `business` is the AGENT-OWNED scan: its prompt, cadence, gate, gate_sql and
//!   categories are DATA in the `app_scan` table, created/maintained by the project's
//!   agent via the `scan_set` MCP tool (no human approval). Blank by default.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::sqlx::{PgPool, Row, query};

/// `findings.kind` / `surveillance_runs.kind` literals. The `(slug, kind)` pair is
/// the discriminator everywhere (cap, freshness memory keys, UI tab).
pub const SECURITY_KIND: &str = "security";
pub const CODE_REVIEW_KIND: &str = "code_review";
pub const BIZ_KIND: &str = "business";

/// Validate a kind coming from MCP/REST.
pub fn is_valid_kind(k: &str) -> bool {
    k == SECURITY_KIND || k == CODE_REVIEW_KIND || k == BIZ_KIND
}

/// Per-(app,kind) memory keys (kind `last_run`) for the freshness gate. Keyed by
/// scan kind so the three scans of one app don't share a watermark.
pub fn sha_key(kind: &str) -> String {
    format!("{kind}_sha")
}
pub fn watermark_key(kind: &str) -> String {
    format!("{kind}_watermark")
}

const PROMPT_SECURITY: &str = include_str!("prompts/security.md");
const PROMPT_CODE_REVIEW: &str = include_str!("prompts/code_review.md");

/// How a run decides whether there's anything new to scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gate {
    /// Re-run only when the app's git HEAD changed since last run.
    Code,
    /// Re-run only when new data appeared (watermark from `gate_sql`).
    Data,
    /// Always run on demand (no freshness skip).
    Manual,
}

impl Gate {
    pub fn parse(s: &str) -> Gate {
        match s {
            "data" => Gate::Data,
            "manual" => Gate::Manual,
            _ => Gate::Code,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Gate::Code => "code",
            Gate::Data => "data",
            Gate::Manual => "manual",
        }
    }
}

/// A resolved scan definition. For `security`/`code_review` it comes from the
/// hardcoded constructors below; for `business` it is loaded from `app_scan`.
#[derive(Debug, Clone, Serialize)]
pub struct ScanDef {
    pub kind: String,
    pub slug: String,
    pub label: String,
    pub prompt: String,
    pub cadence: String,
    pub gate: Gate,
    pub gate_sql: Option<String>,
    pub categories: Vec<String>,
    pub updated_by: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl ScanDef {
    /// The fixed `security` platform scan for an app.
    pub fn security(slug: &str) -> ScanDef {
        ScanDef {
            kind: SECURITY_KIND.to_string(),
            slug: slug.to_string(),
            label: "Sécurité".to_string(),
            prompt: PROMPT_SECURITY.to_string(),
            cadence: "manual".to_string(),
            gate: Gate::Code,
            gate_sql: None,
            categories: ["auth", "injection", "secrets", "exposition", "autres"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            updated_by: Some("system".to_string()),
            updated_at: None,
        }
    }

    /// The fixed `code_review` ("Qualité": bugs/quality/perf) platform scan.
    pub fn code_review(slug: &str) -> ScanDef {
        ScanDef {
            kind: CODE_REVIEW_KIND.to_string(),
            slug: slug.to_string(),
            label: "Qualité".to_string(),
            prompt: PROMPT_CODE_REVIEW.to_string(),
            cadence: "manual".to_string(),
            gate: Gate::Code,
            gate_sql: None,
            categories: [
                "bug",
                "architecture",
                "performance",
                "composants",
                "gestion_erreurs",
                "autres",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            updated_by: Some("system".to_string()),
            updated_at: None,
        }
    }

    /// Resolve a FIXED platform scan by kind (None for `business`, which is in DB).
    pub fn fixed(kind: &str, slug: &str) -> Option<ScanDef> {
        match kind {
            SECURITY_KIND => Some(Self::security(slug)),
            CODE_REVIEW_KIND => Some(Self::code_review(slug)),
            _ => None,
        }
    }

    /// A blank scan (no prompt) is "en veille": a run is a no-op skip.
    pub fn is_blank(&self) -> bool {
        self.prompt.trim().is_empty()
    }

    /// Coerce a category to the scan's declared set, falling back to `autres`.
    pub fn normalize_category(&self, raw: Option<&str>) -> String {
        match raw {
            Some(c) if self.categories.iter().any(|x| x == c) => c.to_string(),
            _ => "autres".to_string(),
        }
    }
}

/// Store for the per-app `business` scan definition (`app_scan` table, DB
/// `atelier_meta`). The table only ever holds the agent-owned business scan.
#[derive(Clone)]
pub struct AppScanStore {
    pool: PgPool,
}

impl AppScanStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read an app's business scan definition (None if no row yet).
    pub async fn get(&self, slug: &str) -> anyhow::Result<Option<ScanDef>> {
        let sql = "SELECT label, prompt, cadence, gate, gate_sql, categories, updated_by, updated_at \
                   FROM app_scan WHERE slug = $1";
        let Some(row) = query(sql).bind(slug).fetch_optional(&self.pool).await? else {
            return Ok(None);
        };
        let categories: sqlx_core::types::Json<Vec<String>> =
            row.try_get("categories").unwrap_or(sqlx_core::types::Json(Vec::new()));
        Ok(Some(ScanDef {
            kind: BIZ_KIND.to_string(),
            slug: slug.to_string(),
            label: row.try_get("label").unwrap_or_default(),
            prompt: row.try_get("prompt").unwrap_or_default(),
            cadence: row.try_get("cadence").unwrap_or_else(|_| "manual".into()),
            gate: Gate::parse(&row.try_get::<String, _>("gate").unwrap_or_else(|_| "code".into())),
            gate_sql: row.try_get("gate_sql").ok().flatten(),
            categories: categories.0,
            updated_by: row.try_get("updated_by").ok().flatten(),
            updated_at: row.try_get("updated_at").ok(),
        }))
    }

    /// Create the blank scan row for an app if absent (idempotent provisioning).
    pub async fn ensure(&self, slug: &str) -> anyhow::Result<()> {
        query("INSERT INTO app_scan (slug) VALUES ($1) ON CONFLICT (slug) DO NOTHING")
            .bind(slug)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Create/replace an app's business scan definition (agent-owned, no approval).
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert(
        &self,
        slug: &str,
        label: &str,
        prompt: &str,
        cadence: &str,
        gate: Gate,
        gate_sql: Option<&str>,
        categories: &[String],
        updated_by: &str,
    ) -> anyhow::Result<()> {
        query(
            "INSERT INTO app_scan (slug, label, prompt, cadence, gate, gate_sql, categories, updated_by, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8, now()) \
             ON CONFLICT (slug) DO UPDATE SET \
               label=EXCLUDED.label, prompt=EXCLUDED.prompt, cadence=EXCLUDED.cadence, \
               gate=EXCLUDED.gate, gate_sql=EXCLUDED.gate_sql, categories=EXCLUDED.categories, \
               updated_by=EXCLUDED.updated_by, updated_at=now()",
        )
        .bind(slug)
        .bind(label)
        .bind(prompt)
        .bind(cadence)
        .bind(gate.as_str())
        .bind(gate_sql)
        .bind(sqlx_core::types::Json(categories))
        .bind(updated_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
