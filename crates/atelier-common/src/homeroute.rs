//! Control-plane store for the Homeroute reverse-proxy integration.
//!
//! Two tables in `atelier_meta`:
//! - `homeroute_settings` (singleton id=1) — the link config (base URL of
//!   Homeroute's hr-api, identity, bearer token). The link is "active" iff a
//!   bearer token is configured — there is no separate enable flag.
//! - `homeroute_routes` — a slug → Homeroute-host-uuid mapping. This is a CACHE:
//!   Homeroute's live config is the source of truth, the uuid is re-resolved by
//!   `subdomain` before any mutation (never trusted blindly).
//!
//! Mirrors the singleton/COALESCE pattern of `atelier-backup::target::TargetStore`
//! and the `Option<PgPool>` no-op-when-absent pattern of `TaskStore`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::control_db::sqlx::{PgPool, PgRow, Pool, Postgres, Row, query};

/// API view of the link settings — the bearer token is REDACTED to a boolean.
#[derive(Debug, Clone, Serialize)]
pub struct HomerouteSettings {
    pub base_url: String,
    pub has_bearer_token: bool,
    /// Label of this Atelier environment (filled with the hostname fallback by
    /// the service so the UI always shows a concrete name).
    pub environment_name: Option<String>,
    /// Public URL advertised to Homeroute at registration (filled with the
    /// default by the service for display).
    pub public_url: Option<String>,
    /// Timestamp of the last successful registration (None ⇒ never registered).
    pub registered_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Internal view (token included) — used to build authenticated HTTP requests.
#[derive(Debug, Clone)]
pub struct FullSettings {
    pub base_url: String,
    pub bearer_token: Option<String>,
    /// Stored environment label (None ⇒ never set; resolve via [`effective_env_name`]).
    pub environment_name: Option<String>,
    /// Stored public URL (None ⇒ never set; resolve via [`effective_public_url`]).
    pub public_url: Option<String>,
}

/// Body of `PUT /api/homeroute/settings`. `bearer_token` absent ⇒ kept.
#[derive(Debug, Clone, Deserialize)]
pub struct NewSettings {
    pub base_url: String,
    /// None ⇒ keep the existing token (COALESCE); empty string ⇒ keep too.
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// None/empty ⇒ keep the existing environment name (COALESCE).
    #[serde(default)]
    pub environment_name: Option<String>,
    /// None/empty ⇒ keep the existing public URL (COALESCE).
    #[serde(default)]
    pub public_url: Option<String>,
}

/// Resolve the effective environment label: the stored one if non-empty, else
/// the machine hostname (`/etc/hostname`), else `"atelier"`. Used both to stamp
/// created hosts and to fill the redacted settings for display.
pub fn effective_env_name(stored: Option<&str>) -> String {
    if let Some(s) = stored.map(str::trim).filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "atelier".to_string())
}

/// Resolve the effective public URL advertised to Homeroute: the stored one if
/// non-empty, else the conventional Atelier edge hostname.
pub fn effective_public_url(stored: Option<&str>) -> String {
    stored
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("https://atelier.mynetwk.biz")
        .to_string()
}

impl NewSettings {
    pub fn validate(&self) -> Result<(), String> {
        let url = self.base_url.trim();
        if url.is_empty() {
            return Err("base_url is required".into());
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("base_url must start with http:// or https://".into());
        }
        Ok(())
    }
}

/// A persisted slug → Homeroute-host mapping row.
#[derive(Debug, Clone, Serialize)]
pub struct RouteRow {
    pub slug: String,
    pub host_id: String,
    pub subdomain: String,
    pub hostname: String,
    pub target_port: i32,
    pub require_auth: bool,
    pub updated_at: DateTime<Utc>,
}

/// Postgres-backed store. Every op degrades to a best-effort no-op / `None`
/// when `pool` is `None` (Postgres unreachable at boot) — same contract as
/// [`crate::task_store::TaskStore`].
#[derive(Clone)]
pub struct HomerouteStore {
    pool: Option<Pool<Postgres>>,
}

impl HomerouteStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Whether the control-plane pool is available (gates the `/api/homeroute/*`
    /// endpoints — 503 when absent, since we cannot persist settings/mapping).
    pub fn is_available(&self) -> bool {
        self.pool.is_some()
    }

    fn pool(&self) -> Option<&Pool<Postgres>> {
        self.pool.as_ref()
    }

    /// Redacted settings view (token → boolean). Returns `None` only when the
    /// pool is absent; the singleton row is seeded by the migration.
    pub async fn get_settings_redacted(&self) -> anyhow::Result<Option<HomerouteSettings>> {
        let Some(p) = self.pool() else { return Ok(None) };
        let row: Option<PgRow> = query(
            r#"
            SELECT base_url,
                   (bearer_token IS NOT NULL AND bearer_token <> '') AS has_bearer_token,
                   environment_name, public_url, registered_at, updated_at
              FROM homeroute_settings WHERE id = 1
            "#,
        )
        .fetch_optional(p)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(HomerouteSettings {
            base_url: row.try_get("base_url")?,
            has_bearer_token: row.try_get("has_bearer_token")?,
            environment_name: row.try_get("environment_name").ok().flatten(),
            public_url: row.try_get("public_url").ok().flatten(),
            registered_at: row.try_get("registered_at").ok().flatten(),
            updated_at: row.try_get("updated_at")?,
        }))
    }

    /// Internal settings view (token included) — for the HTTP client.
    pub async fn get_settings_full(&self) -> anyhow::Result<Option<FullSettings>> {
        let Some(p) = self.pool() else { return Ok(None) };
        let row: Option<PgRow> = query(
            "SELECT base_url, bearer_token, environment_name, public_url \
             FROM homeroute_settings WHERE id = 1",
        )
        .fetch_optional(p)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(FullSettings {
            base_url: row.try_get("base_url")?,
            bearer_token: row.try_get("bearer_token").ok().flatten(),
            environment_name: row.try_get("environment_name").ok().flatten(),
            public_url: row.try_get("public_url").ok().flatten(),
        }))
    }

    /// Update the singleton. `bearer_token` NULL/empty ⇒ preserved (COALESCE).
    pub async fn upsert_settings(&self, s: &NewSettings) -> anyhow::Result<()> {
        let Some(p) = self.pool() else { return Ok(()) };
        query(
            r#"
            UPDATE homeroute_settings
               SET base_url = $1,
                   bearer_token = COALESCE($2, bearer_token),
                   environment_name = COALESCE($3, environment_name),
                   public_url = COALESCE($4, public_url),
                   updated_at = now()
             WHERE id = 1
            "#,
        )
        .bind(s.base_url.trim())
        .bind(s.bearer_token.as_deref().filter(|v| !v.is_empty()))
        .bind(s.environment_name.as_deref().map(str::trim).filter(|v| !v.is_empty()))
        .bind(s.public_url.as_deref().map(str::trim).filter(|v| !v.is_empty()))
        .execute(p)
        .await?;
        Ok(())
    }

    /// Stamp `registered_at = now()` after a successful registration with Homeroute.
    pub async fn touch_registered(&self) -> anyhow::Result<()> {
        let Some(p) = self.pool() else { return Ok(()) };
        query("UPDATE homeroute_settings SET registered_at = now() WHERE id = 1")
            .execute(p)
            .await?;
        Ok(())
    }

    pub async fn list_routes(&self) -> anyhow::Result<Vec<RouteRow>> {
        let Some(p) = self.pool() else { return Ok(Vec::new()) };
        let rows: Vec<PgRow> = query(
            "SELECT slug, host_id, subdomain, hostname, target_port, require_auth, updated_at \
             FROM homeroute_routes ORDER BY slug",
        )
        .fetch_all(p)
        .await?;
        rows.iter().map(row_to_route).collect()
    }

    pub async fn get_route(&self, slug: &str) -> anyhow::Result<Option<RouteRow>> {
        let Some(p) = self.pool() else { return Ok(None) };
        let row: Option<PgRow> = query(
            "SELECT slug, host_id, subdomain, hostname, target_port, require_auth, updated_at \
             FROM homeroute_routes WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(p)
        .await?;
        row.as_ref().map(row_to_route).transpose()
    }

    pub async fn upsert_route(
        &self,
        slug: &str,
        host_id: &str,
        subdomain: &str,
        hostname: &str,
        target_port: u16,
        require_auth: bool,
    ) -> anyhow::Result<()> {
        let Some(p) = self.pool() else { return Ok(()) };
        query(
            r#"
            INSERT INTO homeroute_routes
                (slug, host_id, subdomain, hostname, target_port, require_auth, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, now())
            ON CONFLICT (slug) DO UPDATE SET
                host_id      = EXCLUDED.host_id,
                subdomain    = EXCLUDED.subdomain,
                hostname     = EXCLUDED.hostname,
                target_port  = EXCLUDED.target_port,
                require_auth = EXCLUDED.require_auth,
                updated_at   = now()
            "#,
        )
        .bind(slug)
        .bind(host_id)
        .bind(subdomain)
        .bind(hostname)
        .bind(target_port as i32)
        .bind(require_auth)
        .execute(p)
        .await?;
        Ok(())
    }

    /// Remove the mapping for `slug`. Returns true if a row was deleted.
    pub async fn delete_route(&self, slug: &str) -> anyhow::Result<bool> {
        let Some(p) = self.pool() else { return Ok(false) };
        let res = query("DELETE FROM homeroute_routes WHERE slug = $1")
            .bind(slug)
            .execute(p)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn row_to_route(row: &PgRow) -> anyhow::Result<RouteRow> {
    Ok(RouteRow {
        slug: row.try_get("slug")?,
        host_id: row.try_get("host_id")?,
        subdomain: row.try_get("subdomain")?,
        hostname: row.try_get("hostname")?,
        target_port: row.try_get("target_port")?,
        require_auth: row.try_get("require_auth")?,
        updated_at: row.try_get("updated_at")?,
    })
}
