use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sqlx::{PgRow, Pool, Postgres, Row, query};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: i64,
    pub slug: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub evidence: Option<serde_json::Value>,
    pub plan: String,
    pub fingerprint: String,
    pub category: String,
    pub status: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FindingFilter {
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewFinding {
    pub slug: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub plan: String,
    pub fingerprint: String,
    pub category: String,
    pub evidence: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct FindingsStore {
    pool: Pool<Postgres>,
}

impl FindingsStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    /// Upsert by `(slug, kind, fingerprint)`. On conflict, bumps `last_seen`
    /// and `updated_at` + refreshes severity/title/summary/plan/evidence.
    /// Status is preserved (so dismiss/resolve survive re-detection).
    pub async fn upsert(&self, draft: NewFinding) -> anyhow::Result<Finding> {
        let sql = r#"
            INSERT INTO findings (
                slug, kind, severity, title, summary, evidence, plan, fingerprint, category
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (slug, kind, fingerprint) DO UPDATE SET
                severity   = EXCLUDED.severity,
                title      = EXCLUDED.title,
                summary    = EXCLUDED.summary,
                evidence   = EXCLUDED.evidence,
                plan       = EXCLUDED.plan,
                category   = EXCLUDED.category,
                last_seen  = now(),
                updated_at = now()
            RETURNING id, slug, kind, severity, title, summary, evidence, plan,
                      fingerprint, category, status, first_seen,
                      last_seen, updated_at
        "#;
        let row = query(sql)
            .bind(&draft.slug)
            .bind(&draft.kind)
            .bind(&draft.severity)
            .bind(&draft.title)
            .bind(&draft.summary)
            .bind(&draft.evidence)
            .bind(&draft.plan)
            .bind(&draft.fingerprint)
            .bind(&draft.category)
            .fetch_one(&self.pool)
            .await?;
        row_to_finding(&row)
    }

    pub async fn list(&self, filter: FindingFilter) -> anyhow::Result<Vec<Finding>> {
        let limit = filter.limit.unwrap_or(200).min(1000).max(1);
        let sql = r#"
            SELECT id, slug, kind, severity, title, summary, evidence, plan,
                   fingerprint, category, status, first_seen,
                   last_seen, updated_at
              FROM findings
             WHERE ($1::text IS NULL OR slug     = $1)
               AND ($2::text IS NULL OR kind     = $2)
               AND ($3::text IS NULL OR severity = $3)
               AND ($4::text IS NULL OR status   = $4)
               AND ($5::text IS NULL OR category = $5)
             ORDER BY last_seen DESC
             LIMIT $6
        "#;
        let rows: Vec<PgRow> = query(sql)
            .bind(filter.slug.as_deref())
            .bind(filter.kind.as_deref())
            .bind(filter.severity.as_deref())
            .bind(filter.status.as_deref())
            .bind(filter.category.as_deref())
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_finding).collect()
    }

    pub async fn get(&self, id: i64) -> anyhow::Result<Option<Finding>> {
        let sql = r#"
            SELECT id, slug, kind, severity, title, summary, evidence, plan,
                   fingerprint, category, status, first_seen,
                   last_seen, updated_at
              FROM findings
             WHERE id = $1
        "#;
        let row: Option<PgRow> = query(sql).bind(id).fetch_optional(&self.pool).await?;
        row.as_ref().map(row_to_finding).transpose()
    }

    pub async fn dismiss(&self, id: i64) -> anyhow::Result<bool> {
        let res = query(
            r#"
            UPDATE findings
               SET status = 'dismissed', updated_at = now()
             WHERE id = $1 AND status <> 'dismissed'
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn resolve(&self, id: i64, commit_sha: Option<&str>) -> anyhow::Result<bool> {
        let res = query(
            r#"
            UPDATE findings
               SET status = 'resolved',
                   evidence = COALESCE(evidence, '{}'::jsonb)
                            || jsonb_build_object('resolved_commit', $2::text),
                   updated_at = now()
             WHERE id = $1 AND status <> 'resolved'
            "#,
        )
        .bind(id)
        .bind(commit_sha)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Count all OPEN findings for a (slug, kind). Backs the per-kind cap gate
    /// (`MAX_OPEN_FINDINGS`): a new scan is skipped once the cap is reached, and
    /// the count is injected into the prompt so Codex self-limits to the most
    /// important issues.
    pub async fn count_open(&self, slug: &str, kind: &str) -> anyhow::Result<i64> {
        let sql = r#"
            SELECT COUNT(*)::bigint AS c
              FROM findings
             WHERE slug = $1
               AND kind = $2
               AND status = 'open'
        "#;
        let row = query(sql)
            .bind(slug)
            .bind(kind)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("c")?)
    }

    /// Count findings touched (created or re-seen) at/after `since` for a
    /// given (slug, kind). Used to measure how many findings a run produced.
    pub async fn count_touched_since(
        &self,
        slug: &str,
        kind: &str,
        since: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<i64> {
        let sql = r#"
            SELECT COUNT(*)::bigint AS c
              FROM findings
             WHERE slug = $1
               AND kind = $2
               AND last_seen >= $3
        "#;
        let row = query(sql)
            .bind(slug)
            .bind(kind)
            .bind(since)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("c")?)
    }
}

fn row_to_finding(row: &PgRow) -> anyhow::Result<Finding> {
    Ok(Finding {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        kind: row.try_get("kind")?,
        severity: row.try_get("severity")?,
        title: row.try_get("title")?,
        summary: row.try_get("summary")?,
        evidence: row.try_get("evidence").ok(),
        plan: row.try_get("plan")?,
        fingerprint: row.try_get("fingerprint")?,
        category: row.try_get("category").unwrap_or_else(|_| "autres".to_string()),
        status: row.try_get("status")?,
        first_seen: row.try_get("first_seen")?,
        last_seen: row.try_get("last_seen")?,
        updated_at: row.try_get("updated_at")?,
    })
}
