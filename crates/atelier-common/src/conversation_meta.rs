//! Per-conversation agent settings (model / effort / mode), backed by the shared
//! `atelier_meta` control-plane pool (`agent_conversation_meta`). WHY server-side:
//! the Studio is used from several PCs against the same Atelier backend — these
//! settings must follow the conversation across machines (a per-browser
//! `localStorage` cannot), otherwise reopening a conversation elsewhere silently
//! restarted it on the default model/effort.
//!
//! `model = NULL` means "subscription default" (Opus [1m]) — an explicit state,
//! distinct from "no row" (legacy conversation, frontend keeps its local prefs).
//!
//! Written by the agent relay (session binding + live set_model/set_mode), read
//! by the conversation snapshot. Mutations are best-effort (logged, never
//! propagated) and the whole store degrades to a no-op when the pool is absent —
//! mirrors [`crate::agent_ui_state::OpenTabsStore`].

use serde_json::{Value, json};
use tracing::error;

use crate::control_db::sqlx::{PgPool, Pool, Postgres, Row, query};

#[derive(Clone)]
pub struct ConversationMetaStore {
    pool: Option<Pool<Postgres>>,
}

impl ConversationMetaStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Full upsert at session binding (query/resume) — the requested settings of
    /// the run that (re)opened the conversation.
    pub async fn upsert(
        &self,
        slug: &str,
        sid: &str,
        model: Option<&str>,
        effort: Option<&str>,
        mode: &str,
    ) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            r#"
            INSERT INTO agent_conversation_meta (slug, session_id, model, effort, mode, updated_at)
            VALUES ($1, $2, $3, $4, $5, now())
            ON CONFLICT (slug, session_id) DO UPDATE SET
                model      = EXCLUDED.model,
                effort     = EXCLUDED.effort,
                mode       = EXCLUDED.mode,
                updated_at = now()
            "#,
        )
        .bind(slug)
        .bind(sid)
        .bind(model)
        .bind(effort)
        .bind(mode)
        .execute(pool)
        .await
        {
            error!(slug, sid, error = %e, "conversation_meta upsert failed");
        }
    }

    /// Update ONLY the model (live `set_model`). `None` = back to the
    /// subscription default — stored as an explicit NULL.
    pub async fn set_model(&self, slug: &str, sid: &str, model: Option<&str>) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            r#"
            INSERT INTO agent_conversation_meta (slug, session_id, model, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (slug, session_id) DO UPDATE SET
                model      = EXCLUDED.model,
                updated_at = now()
            "#,
        )
        .bind(slug)
        .bind(sid)
        .bind(model)
        .execute(pool)
        .await
        {
            error!(slug, sid, error = %e, "conversation_meta set_model failed");
        }
    }

    /// Update ONLY the effort. WHY a direct setter: effort has no live SDK API (fixed
    /// at session start) — changing it recycles the session (cancel → resume at next
    /// send). Persisting the INTENT here at click time keeps snapshots/other PCs from
    /// reverting the selector to the old effort before that resume happens.
    pub async fn set_effort(&self, slug: &str, sid: &str, effort: &str) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            r#"
            INSERT INTO agent_conversation_meta (slug, session_id, effort, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (slug, session_id) DO UPDATE SET
                effort     = EXCLUDED.effort,
                updated_at = now()
            "#,
        )
        .bind(slug)
        .bind(sid)
        .bind(effort)
        .execute(pool)
        .await
        {
            error!(slug, sid, error = %e, "conversation_meta set_effort failed");
        }
    }

    /// Update ONLY the mode (`permission_mode` event: /set_mode, plan approval).
    pub async fn set_mode(&self, slug: &str, sid: &str, mode: &str) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            r#"
            INSERT INTO agent_conversation_meta (slug, session_id, mode, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (slug, session_id) DO UPDATE SET
                mode       = EXCLUDED.mode,
                updated_at = now()
            "#,
        )
        .bind(slug)
        .bind(sid)
        .bind(mode)
        .execute(pool)
        .await
        {
            error!(slug, sid, error = %e, "conversation_meta set_mode failed");
        }
    }

    /// `Some({model, effort, mode})` when a row exists, `None` otherwise (legacy
    /// conversation, pool down, or query error) — the caller surfaces `None` as
    /// `settings: null` and the frontend keeps its local defaults.
    pub async fn get(&self, slug: &str, sid: &str) -> Option<Value> {
        let pool = self.pool.as_ref()?;
        match query("SELECT model, effort, mode FROM agent_conversation_meta WHERE slug = $1 AND session_id = $2")
            .bind(slug)
            .bind(sid)
            .fetch_optional(pool)
            .await
        {
            Ok(Some(row)) => {
                let model: Option<String> = row.try_get("model").ok().flatten();
                let effort: Option<String> = row.try_get("effort").ok().flatten();
                let mode: Option<String> = row.try_get("mode").ok().flatten();
                Some(json!({ "model": model, "effort": effort, "mode": mode }))
            }
            Ok(None) => None,
            Err(e) => {
                error!(slug, sid, error = %e, "conversation_meta get failed");
                None
            }
        }
    }

    /// Purge on conversation delete.
    pub async fn delete(&self, slug: &str, sid: &str) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query("DELETE FROM agent_conversation_meta WHERE slug = $1 AND session_id = $2")
            .bind(slug)
            .bind(sid)
            .execute(pool)
            .await
        {
            error!(slug, sid, error = %e, "conversation_meta delete failed");
        }
    }

    /// Purge on app delete (AppDelete hook, mirrors the issues/notifications stores).
    pub async fn delete_by_slug(&self, slug: &str) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query("DELETE FROM agent_conversation_meta WHERE slug = $1")
            .bind(slug)
            .execute(pool)
            .await
        {
            error!(slug, error = %e, "conversation_meta delete_by_slug failed");
        }
    }
}
