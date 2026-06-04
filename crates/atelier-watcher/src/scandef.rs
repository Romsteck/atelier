//! Per-app single scan definition. Each app has exactly ONE scan, defined in the
//! `app_scan` table and owned by the project's agent (created/maintained via the
//! `scan_set` MCP tool — no human approval). Replaces the old hardcoded `RunKind`
//! (code_review/security/suggestions) + the post_mortem prototype: the scan's
//! prompt, cadence, gate and categories are now DATA, not code.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::sqlx::{PgPool, Row, query};

/// `findings.kind` / `surveillance_runs.kind` literal for every app's scan. The
/// `slug` is the real discriminator everywhere; a constant kind keeps the
/// `UNIQUE(slug,kind,fingerprint)` index meaningful and git_watcher kind-agnostic.
pub const SCAN_KIND: &str = "scan";

/// Memory keys (per-app, kind `last_run`) for the freshness gate.
pub const SHA_KEY: &str = "scan_sha";
pub const WATERMARK_KEY: &str = "scan_watermark";

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

/// The app's scan definition (one row in `app_scan`).
#[derive(Debug, Clone, Serialize)]
pub struct ScanDef {
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

/// Store for the per-app scan definition (`app_scan` table, DB `atelier_meta`).
#[derive(Clone)]
pub struct AppScanStore {
    pool: PgPool,
}

impl AppScanStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read an app's scan definition (None if no row yet).
    pub async fn get(&self, slug: &str) -> anyhow::Result<Option<ScanDef>> {
        let sql = "SELECT label, prompt, cadence, gate, gate_sql, categories, updated_by, updated_at \
                   FROM app_scan WHERE slug = $1";
        let Some(row) = query(sql).bind(slug).fetch_optional(&self.pool).await? else {
            return Ok(None);
        };
        let categories: sqlx_core::types::Json<Vec<String>> =
            row.try_get("categories").unwrap_or(sqlx_core::types::Json(Vec::new()));
        Ok(Some(ScanDef {
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

    /// Create/replace an app's scan definition (agent-owned, no approval).
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
