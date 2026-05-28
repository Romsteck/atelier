use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sqlx::{PgRow, Pool, Postgres, Row, query};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSurveillanceConfig {
    pub slug: String,
    pub throttle_threshold: i32,
    pub max_tokens_per_day: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigUpdate {
    pub throttle_threshold: Option<i32>,
    pub max_tokens_per_day: Option<i32>,
}

#[derive(Clone)]
pub struct ConfigStore {
    pool: Pool<Postgres>,
}

impl ConfigStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    /// Insert default row for `slug` if absent. Idempotent — used at boot to
    /// seed every app of the registry. Existing rows are untouched.
    pub async fn seed(&self, slug: &str) -> anyhow::Result<()> {
        query(
            r#"
            INSERT INTO surveillance_config (slug)
            VALUES ($1)
            ON CONFLICT (slug) DO NOTHING
            "#,
        )
        .bind(slug)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, slug: &str) -> anyhow::Result<Option<AppSurveillanceConfig>> {
        let sql = r#"
            SELECT slug, throttle_threshold, max_tokens_per_day, updated_at
              FROM surveillance_config
             WHERE slug = $1
        "#;
        let row: Option<PgRow> = query(sql).bind(slug).fetch_optional(&self.pool).await?;
        row.as_ref().map(row_to_config).transpose()
    }

    pub async fn list(&self) -> anyhow::Result<Vec<AppSurveillanceConfig>> {
        let sql = r#"
            SELECT slug, throttle_threshold, max_tokens_per_day, updated_at
              FROM surveillance_config
             ORDER BY slug
        "#;
        let rows: Vec<PgRow> = query(sql).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_config).collect()
    }

    /// Partial update. Fields set to `None` keep their existing value.
    /// Uses COALESCE so a single SQL handles any subset.
    pub async fn update(
        &self,
        slug: &str,
        upd: ConfigUpdate,
    ) -> anyhow::Result<Option<AppSurveillanceConfig>> {
        let sql = r#"
            UPDATE surveillance_config SET
                throttle_threshold = COALESCE($2, throttle_threshold),
                max_tokens_per_day = COALESCE($3, max_tokens_per_day),
                updated_at         = now()
             WHERE slug = $1
            RETURNING slug, throttle_threshold, max_tokens_per_day, updated_at
        "#;
        let row: Option<PgRow> = query(sql)
            .bind(slug)
            .bind(upd.throttle_threshold)
            .bind(upd.max_tokens_per_day)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_config).transpose()
    }
}

fn row_to_config(row: &PgRow) -> anyhow::Result<AppSurveillanceConfig> {
    Ok(AppSurveillanceConfig {
        slug: row.try_get("slug")?,
        throttle_threshold: row.try_get("throttle_threshold")?,
        max_tokens_per_day: row.try_get("max_tokens_per_day")?,
        updated_at: row.try_get("updated_at")?,
    })
}
