use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sqlx::{PgRow, Pool, Postgres, Row, query};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: Uuid,
    pub slug: String,
    pub kind: String,
    pub trigger: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub skip_reason: Option<String>,
    pub findings_count: i32,
    pub tokens_in: Option<i32>,
    pub tokens_out: Option<i32>,
    pub git_sha_before: Option<String>,
    pub git_sha_reviewed: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct RunsStore {
    pool: Pool<Postgres>,
}

impl RunsStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    /// Open a new run. Caller must `finish_*` it before returning.
    pub async fn start(
        &self,
        slug: &str,
        kind: &str,
        trigger: &str,
        git_sha_before: Option<&str>,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();
        query(
            r#"
            INSERT INTO surveillance_runs (id, slug, kind, trigger, git_sha_before)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(id)
        .bind(slug)
        .bind(kind)
        .bind(trigger)
        .bind(git_sha_before)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_success(
        &self,
        id: Uuid,
        findings_count: i32,
        tokens_in: Option<i32>,
        tokens_out: Option<i32>,
        git_sha_reviewed: Option<&str>,
        empty: bool,
    ) -> anyhow::Result<()> {
        let status = if empty { "success_empty" } else { "success" };
        query(
            r#"
            UPDATE surveillance_runs
               SET status = $2,
                   finished_at = now(),
                   findings_count = $3,
                   tokens_in = $4,
                   tokens_out = $5,
                   git_sha_reviewed = $6
             WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(status)
        .bind(findings_count)
        .bind(tokens_in)
        .bind(tokens_out)
        .bind(git_sha_reviewed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_skipped(&self, id: Uuid, reason: &str) -> anyhow::Result<()> {
        query(
            r#"
            UPDATE surveillance_runs
               SET status = 'skipped',
                   finished_at = now(),
                   skip_reason = $2
             WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a run as cancelled by the user (kill of an in-progress run).
    pub async fn finish_cancelled(&self, id: Uuid) -> anyhow::Result<()> {
        query(
            r#"
            UPDATE surveillance_runs
               SET status = 'cancelled',
                   finished_at = now(),
                   error = 'cancelled by user'
             WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_failed(&self, id: Uuid, error: &str) -> anyhow::Result<()> {
        // Cap error to a reasonable length to keep the row small.
        let truncated: String = error.chars().take(2000).collect();
        query(
            r#"
            UPDATE surveillance_runs
               SET status = 'failed',
                   finished_at = now(),
                   error = $2
             WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(truncated)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self, slug: Option<&str>, limit: i64) -> anyhow::Result<Vec<Run>> {
        let limit = limit.clamp(1, 1000);
        let sql = r#"
            SELECT id, slug, kind, trigger, started_at, finished_at, status,
                   skip_reason, findings_count, tokens_in, tokens_out,
                   git_sha_before, git_sha_reviewed, error
              FROM surveillance_runs
             WHERE ($1::text IS NULL OR slug = $1)
             ORDER BY started_at DESC
             LIMIT $2
        "#;
        let rows: Vec<PgRow> = query(sql)
            .bind(slug)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_run).collect()
    }

    /// Latest run per (slug, kind), all apps at once. An in-flight (`running`)
    /// run always wins the DISTINCT ON tiebreak — a concurrent run of the same
    /// pair that settles fast (skipped/failed) must not hide the dashboard's
    /// "in progress" indicator. Backs `GET /api/surveillance/overview`.
    pub async fn latest_per_app_kind(&self) -> anyhow::Result<Vec<Run>> {
        let sql = r#"
            SELECT DISTINCT ON (slug, kind)
                   id, slug, kind, trigger, started_at, finished_at, status,
                   skip_reason, findings_count, tokens_in, tokens_out,
                   git_sha_before, git_sha_reviewed, error
              FROM surveillance_runs
             ORDER BY slug, kind, (status = 'running') DESC, started_at DESC
        "#;
        let rows: Vec<PgRow> = query(sql).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_run).collect()
    }

    /// Boot reconciliation: any row still 'running' belongs to a previous
    /// process (its tokio task died with it) — mark it failed so the dashboard
    /// "running" counter cannot stay stuck.
    pub async fn reconcile_interrupted(&self) -> anyhow::Result<u64> {
        let res = query(
            r#"
            UPDATE surveillance_runs
               SET status = 'failed', finished_at = now(),
                   error = 'interrupted by restart'
             WHERE status = 'running'
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
}

fn row_to_run(row: &PgRow) -> anyhow::Result<Run> {
    Ok(Run {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        kind: row.try_get("kind")?,
        trigger: row.try_get("trigger")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at").ok(),
        status: row.try_get("status")?,
        skip_reason: row.try_get("skip_reason").ok(),
        findings_count: row.try_get("findings_count")?,
        tokens_in: row.try_get("tokens_in").ok(),
        tokens_out: row.try_get("tokens_out").ok(),
        git_sha_before: row.try_get("git_sha_before").ok(),
        git_sha_reviewed: row.try_get("git_sha_reviewed").ok(),
        error: row.try_get("error").ok(),
    })
}
