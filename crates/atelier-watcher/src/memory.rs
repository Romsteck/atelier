use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sqlx::{PgRow, Pool, Postgres, Row, query};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub slug: String,
    pub kind: String,
    pub key: String,
    pub value: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub ttl_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct MemoryStore {
    pool: Pool<Postgres>,
}

impl MemoryStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    pub async fn upsert(
        &self,
        slug: &str,
        kind: &str,
        key: &str,
        value: &serde_json::Value,
        ttl_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        query(
            r#"
            INSERT INTO agent_memory (slug, kind, key, value, ttl_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (slug, kind, key) DO UPDATE SET
                value      = EXCLUDED.value,
                ttl_at     = EXCLUDED.ttl_at,
                updated_at = now()
            "#,
        )
        .bind(slug)
        .bind(kind)
        .bind(key)
        .bind(value)
        .bind(ttl_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(
        &self,
        slug: &str,
        kind: Option<&str>,
        key: Option<&str>,
    ) -> anyhow::Result<Vec<Memory>> {
        let sql = r#"
            SELECT id, slug, kind, key, value, created_at, updated_at,
                   last_used_at, ttl_at
              FROM agent_memory
             WHERE slug = $1
               AND ($2::text IS NULL OR kind = $2)
               AND ($3::text IS NULL OR key  = $3)
               AND (ttl_at IS NULL OR ttl_at > now())
             ORDER BY last_used_at DESC
        "#;
        let rows: Vec<PgRow> = query(sql)
            .bind(slug)
            .bind(kind)
            .bind(key)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_memory).collect()
    }

    /// Bump `last_used_at` on entries returned to Codex. Pure LRU bookkeeping.
    pub async fn touch(&self, ids: &[i64]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        query(
            r#"
            UPDATE agent_memory
               SET last_used_at = now()
             WHERE id = ANY($1::bigint[])
            "#,
        )
        .bind(ids)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, slug: &str, kind: &str, key: &str) -> anyhow::Result<bool> {
        let res = query(
            r#"
            DELETE FROM agent_memory
             WHERE slug = $1 AND kind = $2 AND key = $3
            "#,
        )
        .bind(slug)
        .bind(kind)
        .bind(key)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Purge entries whose TTL is past. Called periodically by the service.
    pub async fn purge_expired(&self) -> anyhow::Result<u64> {
        let res = query("DELETE FROM agent_memory WHERE ttl_at IS NOT NULL AND ttl_at <= now()")
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
}

fn row_to_memory(row: &PgRow) -> anyhow::Result<Memory> {
    Ok(Memory {
        id: row.try_get("id")?,
        slug: row.try_get("slug")?,
        kind: row.try_get("kind")?,
        key: row.try_get("key")?,
        value: row.try_get("value")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        last_used_at: row.try_get("last_used_at")?,
        ttl_at: row.try_get("ttl_at").ok(),
    })
}
