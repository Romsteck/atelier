//! Studio UI state, backed by the shared `atelier_meta` control-plane pool. WHY
//! server-side: the Studio is used from several PCs against the same Atelier
//! backend, so this state must follow the user across machines/browsers (a
//! per-browser `localStorage` cache cannot). One scope lives here:
//!   - per-app **open tabs** (conversations + files + diffs + commits) + active
//!     tab, keyed by slug (`agent_open_tabs`, paired with the `agent:open-tabs`
//!     WS broadcast).
//!
//! (The former globally-selected-app singleton `studio_state` was removed on
//! 2026-06-21 when the Studio became a separate per-app tab — the open app now
//! comes from the URL `/studio/{slug}`, so there is no global selection to sync.)
//!
//! Degrades to a no-op / "empty" when the pool is absent (Postgres down at boot)
//! — mirrors [`crate::task_store::TaskStore`]; the UI then falls back to its
//! localStorage cache.

use serde_json::{Value, json};
use tracing::error;

use crate::control_db::sqlx::{PgPool, Pool, Postgres, Row, query};

#[derive(Clone)]
pub struct OpenTabsStore {
    pool: Option<Pool<Postgres>>,
}

impl OpenTabsStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Read the persisted `(tabs, active)` for an app. `(json!([]), None)` when
    /// the row is absent or the pool is down (caller treats both as "empty").
    pub async fn get(&self, slug: &str) -> (Value, Option<String>) {
        let Some(pool) = self.pool.as_ref() else {
            return (json!([]), None);
        };
        match query("SELECT tabs, active FROM agent_open_tabs WHERE slug = $1")
            .bind(slug)
            .fetch_optional(pool)
            .await
        {
            Ok(Some(row)) => {
                let tabs: Value = row.try_get("tabs").unwrap_or_else(|_| json!([]));
                let active: Option<String> = row.try_get("active").ok().flatten();
                (tabs, active)
            }
            Ok(None) => (json!([]), None),
            Err(e) => {
                error!(slug, error = %e, "open_tabs get failed");
                (json!([]), None)
            }
        }
    }

    /// Upsert the per-app `{tabs, active}` state (full replacement — last write
    /// wins). No-op when the pool is down.
    pub async fn set(&self, slug: &str, tabs: &Value, active: Option<&str>) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            return Ok(());
        };
        query(
            r#"
            INSERT INTO agent_open_tabs (slug, tabs, active, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (slug) DO UPDATE SET
                tabs       = EXCLUDED.tabs,
                active     = EXCLUDED.active,
                updated_at = now()
            "#,
        )
        .bind(slug)
        .bind(tabs)
        .bind(active)
        .execute(pool)
        .await?;
        Ok(())
    }
}
