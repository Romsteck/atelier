//! Manager that owns the admin connection pool and a cache of per-app
//! [`DataverseEngine`] instances.
//!
//! Lifetime model:
//! - The admin pool stays open for the lifetime of `hr-orchestrator` and is
//!   used only for `CREATE DATABASE`/`CREATE ROLE`/`DROP …` operations.
//! - Per-app pools are opened lazily on first request and cached behind an
//!   `Arc<DataverseEngine>` keyed by slug. The first request pays the
//!   connection-establishment cost; subsequent requests are zero-overhead.
//!
//! DSN resolution: the per-app DATABASE_URL is read from a secrets JSON
//! file (`/opt/homeroute/data/dataverse-secrets.json` in production), or
//! from in-memory overrides supplied by tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::sqlx::{PgPool, PgPoolOptions};
use tokio::sync::RwLock;

use crate::engine::DataverseEngine;
use crate::error::{DataverseError, Result};
use crate::provisioning::{
    self, ProvisioningConfig, ProvisioningResult, app_exists,
};

/// On-disk format of `dataverse-secrets.json`.
///
/// One entry per provisioned app. The file is mode `600` and is the only
/// place where the per-app passwords live in cleartext.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SecretsFile {
    /// Map slug → app secret.
    #[serde(default)]
    pub apps: HashMap<String, AppSecret>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSecret {
    pub db_name: String,
    pub role_name: String,
    pub password: String,
    pub dsn: String,
    /// Stable per-app identity uuid. Used by the dataverse gateway to
    /// populate `created_by` / `updated_by` when the app itself (not a
    /// human user) is the actor. Persistent — never rotated.
    #[serde(default)]
    pub app_uuid: Uuid,
    /// Opaque bearer token the app supplies in `Authorization: Bearer …`
    /// when calling the dataverse gateway. The value is a 32-byte random
    /// blob, base64url-encoded. Rotated by [`DataverseManager::rotate_token`];
    /// the previous value is invalidated immediately.
    #[serde(default)]
    pub gateway_token: String,
    /// Wall-clock timestamp of the last token rotation (or first mint).
    #[serde(default)]
    pub token_rotated_at: Option<DateTime<Utc>>,
}

impl AppSecret {
    /// Generate a fresh `AppSecret::gateway_token` (32 random bytes,
    /// base64url-encoded without padding). 256 bits of entropy.
    pub fn fresh_token() -> String {
        let mut buf = [0u8; 32];
        rand::rng().fill_bytes(&mut buf);
        base64url_no_pad(&buf)
    }

    /// True iff the secret has both an `app_uuid` and a `gateway_token`.
    /// Pre-base-model entries were written before these fields existed
    /// (`app_uuid` deserialises to `Uuid::nil()`, `gateway_token` to "").
    pub fn has_gateway_credentials(&self) -> bool {
        !self.app_uuid.is_nil() && !self.gateway_token.is_empty()
    }
}

fn base64url_no_pad(input: &[u8]) -> String {
    use base64::engine::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

impl From<&ProvisioningResult> for AppSecret {
    fn from(r: &ProvisioningResult) -> Self {
        Self {
            db_name: r.db_name.clone(),
            role_name: r.role_name.clone(),
            password: r.password.clone(),
            dsn: r.dsn.clone(),
            app_uuid: Uuid::new_v4(),
            gateway_token: AppSecret::fresh_token(),
            token_rotated_at: Some(Utc::now()),
        }
    }
}

pub struct DataverseManager {
    admin_pool: PgPool,
    admin_dsn: String,
    config: ProvisioningConfig,
    secrets_path: Option<PathBuf>,
    /// Overrides keyed by slug → DSN. Checked before the secrets file.
    /// Production code typically leaves this empty; tests inject ephemeral DSNs.
    dsn_overrides: RwLock<HashMap<String, String>>,
    engines: RwLock<HashMap<String, Arc<DataverseEngine>>>,
}

impl DataverseManager {
    pub fn new(
        admin_pool: PgPool,
        admin_dsn: String,
        config: ProvisioningConfig,
        secrets_path: Option<PathBuf>,
    ) -> Self {
        Self {
            admin_pool,
            admin_dsn,
            config,
            secrets_path,
            dsn_overrides: RwLock::new(HashMap::new()),
            engines: RwLock::new(HashMap::new()),
        }
    }

    /// Convenience constructor: open the admin pool from a DSN, then
    /// build the manager. Used by `hr-orchestrator::main` so it doesn't
    /// have to depend on `sqlx_postgres` directly.
    pub async fn connect_admin(
        admin_dsn: String,
        config: ProvisioningConfig,
        secrets_path: Option<PathBuf>,
    ) -> Result<Self> {
        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_dsn)
            .await
            .map_err(|e| DataverseError::internal(format!("connect admin: {}", e)))?;
        Ok(Self::new(admin_pool, admin_dsn, config, secrets_path))
    }

    pub fn admin_pool(&self) -> &PgPool { &self.admin_pool }
    pub fn config(&self) -> &ProvisioningConfig { &self.config }
    pub fn secrets_path(&self) -> Option<&Path> { self.secrets_path.as_deref() }

    /// Manually register a DSN for a slug (useful in tests, or to load from
    /// a non-default secret store at boot).
    pub async fn set_dsn_override(&self, slug: impl Into<String>, dsn: impl Into<String>) {
        self.dsn_overrides
            .write()
            .await
            .insert(slug.into(), dsn.into());
    }

    /// Resolve the DSN for `slug` from overrides → secrets file. Returns
    /// `NotProvisioned` if neither yields a result.
    pub async fn resolve_dsn(&self, slug: &str) -> Result<String> {
        if let Some(dsn) = self.dsn_overrides.read().await.get(slug).cloned() {
            return Ok(dsn);
        }
        if let Some(path) = &self.secrets_path {
            let secrets = read_secrets_file(path)?;
            if let Some(s) = secrets.apps.get(slug) {
                return Ok(s.dsn.clone());
            }
        }
        Err(DataverseError::NotProvisioned(slug.to_string()))
    }

    /// Get or open the engine for `slug`. Opens a new connection pool on
    /// first call and primes the `_dv_*` metadata defensively (idempotent).
    pub async fn engine_for(&self, slug: &str) -> Result<Arc<DataverseEngine>> {
        if let Some(eng) = self.engines.read().await.get(slug).cloned() {
            return Ok(eng);
        }

        let dsn = self.resolve_dsn(slug).await?;
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&dsn)
            .await
            .map_err(|e| DataverseError::provisioning(slug, format!("connect: {}", e)))?;

        let engine = Arc::new(DataverseEngine::new(pool, slug));
        engine.init_metadata().await?;

        let mut guard = self.engines.write().await;
        if let Some(existing) = guard.get(slug).cloned() {
            // Lost race; close the pool we just opened.
            engine.pool().clone().close().await;
            return Ok(existing);
        }
        guard.insert(slug.to_string(), engine.clone());
        Ok(engine)
    }

    /// Drop a cached engine (closes its pool). Used after `drop_app`.
    pub async fn evict(&self, slug: &str) {
        if let Some(eng) = self.engines.write().await.remove(slug) {
            eng.pool().clone().close().await;
        }
        self.dsn_overrides.write().await.remove(slug);
    }

    /// Provision a new app and persist its secret to the configured
    /// secrets file (if any). Returns the `ProvisioningResult` so the
    /// caller can inject the DATABASE_URL into the app's env.
    pub async fn provision(&self, slug: &str) -> Result<ProvisioningResult> {
        let result =
            provisioning::provision_app(&self.admin_pool, &self.config, &self.admin_dsn, slug).await?;

        if let Some(path) = &self.secrets_path {
            let mut secrets = read_secrets_file(path).unwrap_or_default();
            secrets.apps.insert(slug.to_string(), AppSecret::from(&result));
            write_secrets_file(path, &secrets)?;
        }
        Ok(result)
    }

    pub async fn exists(&self, slug: &str) -> Result<bool> {
        app_exists(&self.admin_pool, slug).await
    }

    /// Adopt an existing Postgres database for `slug`: assume the DB
    /// and role were provisioned earlier (possibly by a now-lost
    /// secret), reset the role's password to a fresh value, and
    /// persist the new secret as if it were a brand new provisioning.
    ///
    /// Used by the migration tool to recover from "the secrets file
    /// was lost / never written" scenarios — common during the
    /// transitional rollout. Caller is expected to validate that the
    /// schema in the existing DB matches what they expect.
    pub async fn adopt_existing(&self, slug: &str) -> Result<ProvisioningResult> {
        if !self.exists(slug).await? {
            return Err(DataverseError::provisioning(
                slug,
                "no postgres database to adopt",
            ));
        }
        let result = crate::provisioning::adopt_app(
            &self.admin_pool,
            &self.config,
            slug,
        )
        .await?;

        if let Some(path) = &self.secrets_path {
            let mut secrets = read_secrets_file(path).unwrap_or_default();
            secrets.apps.insert(slug.to_string(), AppSecret::from(&result));
            write_secrets_file(path, &secrets)?;
        }
        Ok(result)
    }

    /// Execute an arbitrary GraphQL request (query or mutation) against
    /// the app's dynamic schema. The schema is built lazily and cached
    /// per `_dv_meta.schema_version`.
    ///
    /// Returns the full async-graphql response serialised as JSON, with
    /// the canonical `{ "data": …, "errors": [...] }` shape.
    pub async fn graphql_execute(
        &self,
        slug: &str,
        query: &str,
        variables: Option<serde_json::Value>,
        operation_name: Option<&str>,
    ) -> Result<serde_json::Value> {
        let engine = self.engine_for(slug).await?;
        let schema = engine.graphql_schema().await?;

        let mut req = async_graphql::Request::new(query);
        if let Some(vars) = variables {
            req = req.variables(async_graphql::Variables::from_json(vars));
        }
        if let Some(op) = operation_name {
            req = req.operation_name(op);
        }

        let response = schema.execute(req).await;
        serde_json::to_value(response)
            .map_err(|e| DataverseError::internal(format!("response serialise: {}", e)))
    }

    /// Return the SDL representation of the app's GraphQL schema. Useful
    /// for the `db.introspect` MCP tool: an agent coding inside an app
    /// can fetch the SDL to discover the data model in a single call.
    pub async fn introspect_sdl(&self, slug: &str) -> Result<String> {
        let engine = self.engine_for(slug).await?;
        let schema = engine.graphql_schema().await?;
        Ok(schema.sdl())
    }

    /// Read the current `AppSecret` for `slug`.
    pub fn read_secret(&self, slug: &str) -> Result<Option<AppSecret>> {
        let Some(path) = &self.secrets_path else {
            return Ok(None);
        };
        let secrets = read_secrets_file(path)?;
        Ok(secrets.apps.get(slug).cloned())
    }

    /// Backfill an `app_uuid` and `gateway_token` for `slug` if missing
    /// (i.e. for entries written before the gateway-credential fields
    /// existed). Returns the secret in its final form. No-op when the
    /// fields are already populated.
    pub fn ensure_gateway_credentials(&self, slug: &str) -> Result<AppSecret> {
        let Some(path) = &self.secrets_path else {
            return Err(DataverseError::NotProvisioned(slug.to_string()));
        };
        let mut secrets = read_secrets_file(path)?;
        let entry = secrets
            .apps
            .get_mut(slug)
            .ok_or_else(|| DataverseError::NotProvisioned(slug.to_string()))?;
        if !entry.has_gateway_credentials() {
            if entry.app_uuid.is_nil() {
                entry.app_uuid = Uuid::new_v4();
            }
            if entry.gateway_token.is_empty() {
                entry.gateway_token = AppSecret::fresh_token();
                entry.token_rotated_at = Some(Utc::now());
            }
            let snapshot = entry.clone();
            write_secrets_file(path, &secrets)?;
            return Ok(snapshot);
        }
        Ok(entry.clone())
    }

    /// Generate and persist a fresh `gateway_token` for `slug`. Invalidates
    /// the previous token immediately. The `app_uuid` is left untouched.
    pub fn rotate_token(&self, slug: &str) -> Result<String> {
        let Some(path) = &self.secrets_path else {
            return Err(DataverseError::NotProvisioned(slug.to_string()));
        };
        let mut secrets = read_secrets_file(path)?;
        let entry = secrets
            .apps
            .get_mut(slug)
            .ok_or_else(|| DataverseError::NotProvisioned(slug.to_string()))?;
        let new_token = AppSecret::fresh_token();
        entry.gateway_token = new_token.clone();
        entry.token_rotated_at = Some(Utc::now());
        if entry.app_uuid.is_nil() {
            entry.app_uuid = Uuid::new_v4();
        }
        write_secrets_file(path, &secrets)?;
        Ok(new_token)
    }

    /// Verify that `presented` matches the stored `gateway_token` for
    /// `slug`. Returns the `app_uuid` on success. Constant-time on the
    /// token comparison.
    pub fn verify_token(&self, slug: &str, presented: &str) -> Result<Uuid> {
        let secret = self
            .read_secret(slug)?
            .ok_or_else(|| DataverseError::NotProvisioned(slug.to_string()))?;
        if secret.gateway_token.is_empty() {
            return Err(DataverseError::internal(
                "gateway_token not yet provisioned — call ensure_gateway_credentials",
            ));
        }
        if !ct_eq(secret.gateway_token.as_bytes(), presented.as_bytes()) {
            return Err(DataverseError::internal("invalid gateway token"));
        }
        Ok(secret.app_uuid)
    }

    /// Tear down database + role for an app and remove its secret entry.
    pub async fn drop_app(&self, slug: &str) -> Result<()> {
        self.evict(slug).await;
        provisioning::drop_app(&self.admin_pool, slug).await?;
        if let Some(path) = &self.secrets_path {
            if let Ok(mut secrets) = read_secrets_file(path) {
                secrets.apps.remove(slug);
                let _ = write_secrets_file(path, &secrets);
            }
        }
        Ok(())
    }
}

fn read_secrets_file(path: &Path) -> Result<SecretsFile> {
    if !path.exists() {
        return Ok(SecretsFile::default());
    }
    let bytes = std::fs::read(path)
        .map_err(|e| DataverseError::internal(format!("read secrets {}: {}", path.display(), e)))?;
    let parsed: SecretsFile = serde_json::from_slice(&bytes)?;
    Ok(parsed)
}

fn write_secrets_file(path: &Path, secrets: &SecretsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            DataverseError::internal(format!("mkdir {}: {}", parent.display(), e))
        })?;
    }
    let bytes = serde_json::to_vec_pretty(secrets)?;
    std::fs::write(path, &bytes)
        .map_err(|e| DataverseError::internal(format!("write secrets {}: {}", path.display(), e)))?;
    set_owner_only(path);
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

/// Constant-time byte slice equality.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("hr-dataverse-test-{}-{}.json", name, nanos))
    }

    #[test]
    fn read_missing_secrets_file_returns_empty() {
        let path = unique_tmp("missing");
        assert!(!path.exists());
        let s = read_secrets_file(&path).unwrap();
        assert!(s.apps.is_empty());
    }

    #[test]
    fn round_trip_secrets_file() {
        let path = unique_tmp("round-trip");
        let mut s = SecretsFile::default();
        s.apps.insert("foo".into(), AppSecret {
            db_name: "app_foo".into(),
            role_name: "app_foo".into(),
            password: "deadbeef".into(),
            dsn: "postgres://app_foo:deadbeef@localhost:5432/app_foo".into(),
            app_uuid: Uuid::new_v4(),
            gateway_token: AppSecret::fresh_token(),
            token_rotated_at: Some(Utc::now()),
        });
        write_secrets_file(&path, &s).unwrap();
        let read = read_secrets_file(&path).unwrap();
        assert_eq!(read.apps.len(), 1);
        let entry = read.apps.get("foo").unwrap();
        assert_eq!(entry.db_name, "app_foo");
        assert!(!entry.app_uuid.is_nil());
        assert!(!entry.gateway_token.is_empty());
        assert!(entry.has_gateway_credentials());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn legacy_entry_without_gateway_fields_deserialises() {
        // Pre-base-model entry: only the four legacy fields. The new fields
        // must default to their zero values so older secrets files keep
        // loading.
        let path = unique_tmp("legacy");
        std::fs::write(
            &path,
            r#"{
              "apps": {
                "legacy": {
                  "db_name": "app_legacy",
                  "role_name": "app_legacy",
                  "password": "p",
                  "dsn": "postgres://app_legacy:p@localhost:5432/app_legacy"
                }
              }
            }"#,
        )
        .unwrap();
        let read = read_secrets_file(&path).unwrap();
        let entry = read.apps.get("legacy").unwrap();
        assert!(entry.app_uuid.is_nil());
        assert!(entry.gateway_token.is_empty());
        assert!(!entry.has_gateway_credentials());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fresh_token_is_unique_and_url_safe() {
        let a = AppSecret::fresh_token();
        let b = AppSecret::fresh_token();
        assert_ne!(a, b);
        assert!(!a.is_empty());
        // base64url-no-pad: only [A-Za-z0-9_-]
        assert!(a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    #[test]
    fn ct_eq_equal_strings() {
        assert!(ct_eq(b"hello", b"hello"));
        assert!(!ct_eq(b"hello", b"world"));
        assert!(!ct_eq(b"short", b"short_but_longer"));
    }
}
