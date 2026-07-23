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
//! — same no-op-without-pool contract as the other `atelier_meta` stores; the UI
//! then falls back to its localStorage cache.

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

    /// All `(slug, kind)` scan pairs that currently have an OPEN resolution
    /// conversation tab. A conversation tab carries `sk` (scan kind) when it was
    /// launched from the surveillance "Résoudre tout" button (cf.
    /// AgentConversationsContext canonTabs) — the conversation resolves ALL open
    /// findings of that scan at once. Lets the global surveillance page flag
    /// apps/scans being resolved across machines — without the per-app Studio
    /// React context. Degrades to empty on a down pool or query error.
    pub async fn resolving_pairs(&self) -> Vec<(String, String)> {
        let Some(pool) = self.pool.as_ref() else {
            return Vec::new();
        };
        let rows = match query(
            r#"
            SELECT slug, elem->>'sk' AS kind
              FROM agent_open_tabs, jsonb_array_elements(tabs) AS elem
             WHERE elem->>'t' = 'c' AND elem->>'sk' IS NOT NULL
            "#,
        )
        .fetch_all(pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "open_tabs resolving_pairs failed");
                return Vec::new();
            }
        };
        rows.iter()
            .filter_map(|r| {
                let slug: String = r.try_get("slug").ok()?;
                let kind: String = r.try_get("kind").ok()?;
                Some((slug, kind))
            })
            .collect()
    }

    /// Read the persisted Studio TOP-LEVEL tab `(tab, kind)` for an app
    /// (code/preview/…/surveillance + the surveillance sub-scan). `(None, None)`
    /// when the row is absent or the pool is down. Independent of `(tabs, active)`
    /// — those are the AgentWorkspace's conversation tabs.
    pub async fn get_studio_tab(&self, slug: &str) -> (Option<String>, Option<String>) {
        let Some(pool) = self.pool.as_ref() else {
            return (None, None);
        };
        match query("SELECT studio_tab, studio_kind FROM agent_open_tabs WHERE slug = $1")
            .bind(slug)
            .fetch_optional(pool)
            .await
        {
            Ok(Some(row)) => (
                row.try_get("studio_tab").ok().flatten(),
                row.try_get("studio_kind").ok().flatten(),
            ),
            Ok(None) => (None, None),
            Err(e) => {
                error!(slug, error = %e, "studio_tab get failed");
                (None, None)
            }
        }
    }

    /// Upsert ONLY the Studio top-level tab/kind, preserving the row's
    /// conversation `tabs`/`active` (a fresh row defaults `tabs` to `'[]'`).
    /// No-op when the pool is down.
    pub async fn set_studio_tab(
        &self,
        slug: &str,
        tab: &str,
        kind: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            return Ok(());
        };
        query(
            r#"
            INSERT INTO agent_open_tabs (slug, studio_tab, studio_kind, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (slug) DO UPDATE SET
                studio_tab  = EXCLUDED.studio_tab,
                studio_kind = EXCLUDED.studio_kind,
                updated_at  = now()
            "#,
        )
        .bind(slug)
        .bind(tab)
        .bind(kind)
        .execute(pool)
        .await?;
        Ok(())
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
