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
use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;
use crate::sqlx::{Executor, PgPool, PgPoolOptions};
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

/// Maximum age of a gateway token before it is rejected by
/// [`DataverseManager::verify_token`]. Apps must call `rotate_token` before
/// the deadline. Override via `ATELIER_DV_TOKEN_MAX_AGE_SECS` for tests or
/// short-lived deployments.
const DEFAULT_TOKEN_MAX_AGE_SECS: u64 = 90 * 24 * 3600; // 90 days

fn token_max_age_secs() -> u64 {
    std::env::var("ATELIER_DV_TOKEN_MAX_AGE_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TOKEN_MAX_AGE_SECS)
}

/// Default per-statement timeout enforced on every connection opened by
/// `hr-dataverse` (admin + app pools). 30s is generous for OLTP — schema
/// mutations and migrations run on the admin pool which is fine, large
/// migrations should use the offline `hr-dataverse-migrate` tool, not the
/// gateway. Without this, a transaction abandoned by a crashed client
/// holds row locks indefinitely.
const DEFAULT_STATEMENT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_IDLE_IN_TX_TIMEOUT_MS: u64 = 60_000;

/// Build a `PgPoolOptions` pre-configured with sane session defaults
/// (statement_timeout, idle_in_transaction_session_timeout). Wrap every
/// `PgPoolOptions::new()` call site that opens a pool against an app or
/// admin database.
pub fn pool_with_session_defaults() -> PgPoolOptions {
    let stmt_ms = std::env::var("ATELIER_DV_STATEMENT_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STATEMENT_TIMEOUT_MS);
    let idle_ms = std::env::var("ATELIER_DV_IDLE_IN_TX_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_IN_TX_TIMEOUT_MS);
    let sql = format!(
        "SET statement_timeout = {}; SET idle_in_transaction_session_timeout = {};",
        stmt_ms, idle_ms
    );
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(10))
        .after_connect(move |conn, _meta| {
            let sql = sql.clone();
            Box::pin(async move {
                conn.execute(sql.as_str()).await?;
                Ok(())
            })
        })
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
    /// Serializes all provisioning operations. Without this, two parallel
    /// `provision("foo")` calls can both pass the `app_exists` check and race
    /// to `CREATE ROLE`, leaving a half-provisioned state. Atelier is
    /// mono-process so a Rust mutex is sufficient; if we ever fan out to
    /// multiple instances, switch to a Postgres advisory lock on a pinned
    /// admin connection.
    provision_lock: tokio::sync::Mutex<()>,
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
            provision_lock: tokio::sync::Mutex::new(()),
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
        let admin_pool = pool_with_session_defaults()
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
    #[instrument(level = "info", skip(self), fields(slug = %slug))]
    pub async fn engine_for(&self, slug: &str) -> Result<Arc<DataverseEngine>> {
        if let Some(eng) = self.engines.read().await.get(slug).cloned() {
            return Ok(eng);
        }

        let dsn = self.resolve_dsn(slug).await?;
        let pool = pool_with_session_defaults()
            .max_connections(8)
            .connect(&dsn)
            .await
            .map_err(|e| DataverseError::provisioning(slug, format!("connect: {}", e)))?;

        let engine = Arc::new(DataverseEngine::new(pool, slug));
        engine.init_metadata().await?;
        // Drift detection in dry-run mode at first open. Surfaces orphans
        // left by partial DDL mutations (cf. SyncSchemaReport). Active
        // repair is gated behind the /_repair admin endpoint.
        if let Err(e) = engine.sync_schema(true).await {
            tracing::warn!(slug = %slug, error = ?e, "sync_schema check failed");
        }

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
    #[instrument(level = "info", skip(self), fields(slug = %slug))]
    pub async fn provision(&self, slug: &str) -> Result<ProvisioningResult> {
        // Serialize all provisioning operations to avoid TOCTOU races on the
        // `app_exists` check. Held for the whole CREATE ROLE/DATABASE +
        // INIT_METADATA sequence + secrets-file write.
        let _guard = self.provision_lock.lock().await;

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
    #[instrument(level = "info", skip(self), fields(slug = %slug))]
    pub async fn adopt_existing(&self, slug: &str) -> Result<ProvisioningResult> {
        // Share the provision_lock so adopt + provision serialize against
        // each other (both mutate the secrets file and the postgres role).
        let _guard = self.provision_lock.lock().await;

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
    /// token comparison. Rejects tokens whose `token_rotated_at` is older
    /// than [`TOKEN_MAX_AGE`] to force periodic rotation.
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
        // Enforce token age. Tokens written before this check existed have
        // `token_rotated_at = Some(now)` from the provision/adopt flow, so
        // the only entries without a timestamp are legacy ones from much
        // older provisioning paths — we treat those as expired so rotation
        // is required before next use.
        let rotated_at = secret.token_rotated_at.ok_or_else(|| {
            DataverseError::internal(
                "gateway token has no rotation timestamp; please call rotate_token",
            )
        })?;
        let age = Utc::now().signed_duration_since(rotated_at);
        let max_age = chrono::Duration::seconds(token_max_age_secs() as i64);
        if age > max_age {
            return Err(DataverseError::internal(format!(
                "gateway token expired (age {}d, max {}d); please rotate",
                age.num_days(),
                max_age.num_days()
            )));
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
    atomic_write_owner_only(path, &bytes)
}

/// Write `bytes` to `path` atomically with mode `0o600` from the moment the
/// file exists on disk. The previous implementation (`fs::write` then
/// `set_permissions`) briefly exposed the file under the process umask
/// (typically `0o644`) — a small but reproducible window where the per-app
/// Postgres passwords were world-readable.
#[cfg(unix)]
fn atomic_write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| DataverseError::internal(format!("no parent for {}", path.display())))?;

    // Pick a tmp name in the *same* directory so the final rename is atomic
    // on the same filesystem. Filename includes pid + random suffix so two
    // concurrent writers don't collide.
    let pid = std::process::id();
    let mut rnd = [0u8; 6];
    rand::rng().fill_bytes(&mut rnd);
    let suffix: String = rnd.iter().map(|b| format!("{:02x}", b)).collect();
    let fname = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "secrets".to_string());
    let tmp = parent.join(format!(".{}.tmp.{}.{}", fname, pid, suffix));

    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| {
            DataverseError::internal(format!("create tmp {}: {}", tmp.display(), e))
        })?;
    f.write_all(bytes).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        DataverseError::internal(format!("write tmp {}: {}", tmp.display(), e))
    })?;
    f.sync_all().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        DataverseError::internal(format!("fsync tmp {}: {}", tmp.display(), e))
    })?;
    drop(f);

    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        DataverseError::internal(format!(
            "rename {} -> {}: {}",
            tmp.display(),
            path.display(),
            e
        ))
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn atomic_write_owner_only(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)
        .map_err(|e| DataverseError::internal(format!("write secrets {}: {}", path.display(), e)))
}

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
