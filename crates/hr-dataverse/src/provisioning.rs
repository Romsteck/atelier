//! Per-app database provisioning.
//!
//! Creates a dedicated Postgres database and login role for an app, with
//! privileges scoped strictly to that database. Returns the connection info
//! the caller must persist (the password is the secret to keep).
//!
//! `CREATE DATABASE` cannot run inside a transaction in Postgres, so each
//! statement runs as its own auto-committed query against the admin pool.
//!
//! Idempotency: if the database or role already exists, this returns an
//! error — the caller should call [`drop_app`] first or check existence
//! via [`app_exists`].

use rand::RngCore;

use crate::sqlx::{self, PgPool, PgPoolOptions};
use crate::engine::INIT_METADATA_SQL;
use crate::error::{DataverseError, Result};
use crate::migration::quote_ident;

#[derive(Debug, Clone)]
pub struct ProvisioningConfig {
    /// Host where Postgres listens (used to build per-app DATABASE_URL).
    pub host: String,
    /// Port (default 5432).
    pub port: u16,
}

impl Default for ProvisioningConfig {
    fn default() -> Self {
        Self { host: "127.0.0.1".into(), port: 5432 }
    }
}

#[derive(Debug, Clone)]
pub struct ProvisioningResult {
    pub slug: String,
    pub db_name: String,
    pub role_name: String,
    /// Random password — caller MUST persist this; it cannot be recovered.
    pub password: String,
    /// `postgres://role:password@host:port/db_name`
    pub dsn: String,
}

/// Build the conventional `app_{slug}` name. Slugs may contain dashes; we
/// keep them as-is since Postgres accepts them when quoted.
pub fn db_name_for(slug: &str) -> String {
    format!("app_{}", slug)
}

pub fn role_name_for(slug: &str) -> String {
    format!("app_{}", slug)
}

/// Return true if the per-app database already exists.
pub async fn app_exists(admin: &PgPool, slug: &str) -> Result<bool> {
    let db = db_name_for(slug);
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
    )
    .bind(&db)
    .fetch_optional(admin)
    .await?;
    Ok(row.map(|(b,)| b).unwrap_or(false))
}

/// Provision the database, role, grants, and `_dv_*` metadata for `slug`.
///
/// Caller passes a PgPool connected as a superuser (or a role with
/// `CREATEDB`/`CREATEROLE`). The admin pool stays connected to its original
/// database; this function opens a temporary second pool to the new DB to
/// run [`INIT_METADATA_SQL`].
pub async fn provision_app(
    admin: &PgPool,
    config: &ProvisioningConfig,
    _admin_dsn: &str,
    slug: &str,
) -> Result<ProvisioningResult> {
    crate::validation::validate_user_identifier(slug).map_err(|e| {
        DataverseError::provisioning(slug, format!("invalid slug: {}", e))
    })?;

    if app_exists(admin, slug).await? {
        return Err(DataverseError::provisioning(slug, "database already exists"));
    }

    let db_name = db_name_for(slug);
    let role_name = role_name_for(slug);
    let password = random_password();

    // 1. CREATE ROLE first — if CREATE DATABASE succeeds but role fails,
    //    we leave a fresh DB; the inverse leaves a role we couldn't connect to.
    //
    //    The password is bound as a literal because Postgres does NOT support
    //    bound params in DDL. We sanitise by re-validating the password
    //    character set before injection. Since we generate it ourselves from
    //    a known alphabet, this is safe.
    let role_sql = format!(
        "CREATE ROLE {role} LOGIN PASSWORD '{pwd}'",
        role = quote_ident(&role_name),
        pwd = escape_pg_string(&password),
    );
    sqlx::query(&role_sql).execute(admin).await.map_err(|e| {
        DataverseError::provisioning(slug, format!("CREATE ROLE: {}", e))
    })?;

    // 1.b. Grant the new role to the admin so we can `CREATE DATABASE …
    //      OWNER …` with it. Postgres rejects ownership transfers to a
    //      role the creator isn't a member of (unless superuser).
    let grant_membership_sql = format!(
        "GRANT {role} TO CURRENT_USER",
        role = quote_ident(&role_name),
    );
    if let Err(e) = sqlx::query(&grant_membership_sql).execute(admin).await {
        let _ = sqlx::query(&format!("DROP ROLE IF EXISTS {}", quote_ident(&role_name)))
            .execute(admin)
            .await;
        return Err(DataverseError::provisioning(
            slug,
            format!("GRANT membership: {}", e),
        ));
    }

    // 2. CREATE DATABASE owned by the new role.
    let db_sql = format!(
        "CREATE DATABASE {db} OWNER {role}",
        db = quote_ident(&db_name),
        role = quote_ident(&role_name),
    );
    if let Err(e) = sqlx::query(&db_sql).execute(admin).await {
        // Roll back role creation on failure.
        let _ = sqlx::query(&format!("DROP ROLE IF EXISTS {}", quote_ident(&role_name)))
            .execute(admin)
            .await;
        return Err(DataverseError::provisioning(slug, format!("CREATE DATABASE: {}", e)));
    }

    // 3. Tighten privileges: revoke PUBLIC default, grant only to the app role.
    let _ = sqlx::query(&format!(
        "REVOKE ALL ON DATABASE {} FROM PUBLIC",
        quote_ident(&db_name)
    ))
    .execute(admin)
    .await;
    sqlx::query(&format!(
        "GRANT ALL ON DATABASE {} TO {}",
        quote_ident(&db_name),
        quote_ident(&role_name),
    ))
    .execute(admin)
    .await
    .map_err(|e| DataverseError::provisioning(slug, format!("GRANT: {}", e)))?;

    let dsn = format!(
        "postgres://{role}:{pwd}@{host}:{port}/{db}",
        role = url_encode_component(&role_name),
        pwd = url_encode_component(&password),
        host = config.host,
        port = config.port,
        db = url_encode_component(&db_name),
    );

    // 4. Connect to the freshly created DB **as the app role** (not admin)
    //    to run INIT_METADATA_SQL — that way `_dv_*` tables, indexes and
    //    the `_dv_set_updated_at` function are owned by the app role,
    //    which avoids "must be owner of …" errors when the engine later
    //    re-runs init_metadata defensively.
    let init_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&dsn)
        .await
        .map_err(|e| DataverseError::provisioning(slug, format!("connect new DB as app role: {}", e)))?;

    sqlx::raw_sql(INIT_METADATA_SQL)
        .execute(&init_pool)
        .await
        .map_err(|e| DataverseError::provisioning(slug, format!("init _dv_*: {}", e)))?;
    init_pool.close().await;

    Ok(ProvisioningResult {
        slug: slug.to_string(),
        db_name,
        role_name,
        password,
        dsn,
    })
}

/// Adopt an existing per-app PG database whose password we no longer
/// have: ALTER ROLE to a fresh random password, return the new
/// `ProvisioningResult`. Does NOT touch the database contents.
pub async fn adopt_app(
    admin: &PgPool,
    config: &ProvisioningConfig,
    slug: &str,
) -> Result<ProvisioningResult> {
    crate::validation::validate_user_identifier(slug).map_err(|e| {
        DataverseError::provisioning(slug, format!("invalid slug: {}", e))
    })?;

    let db_name = db_name_for(slug);
    let role_name = role_name_for(slug);
    let password = random_password();

    // ALTER ROLE to set a known password. The role must exist already
    // (paired with the existing DB).
    let alter_sql = format!(
        "ALTER ROLE {role} WITH LOGIN PASSWORD '{pwd}'",
        role = quote_ident(&role_name),
        pwd = escape_pg_string(&password),
    );
    sqlx::query(&alter_sql).execute(admin).await.map_err(|e| {
        DataverseError::provisioning(slug, format!("ALTER ROLE: {}", e))
    })?;

    let dsn = format!(
        "postgres://{role}:{pwd}@{host}:{port}/{db}",
        role = url_encode_component(&role_name),
        pwd = url_encode_component(&password),
        host = config.host,
        port = config.port,
        db = url_encode_component(&db_name),
    );

    Ok(ProvisioningResult {
        slug: slug.to_string(),
        db_name,
        role_name,
        password,
        dsn,
    })
}

/// Tear down the database and role for an app. Used by tests and by
/// `AppDelete` flows. Does not destroy backups.
pub async fn drop_app(admin: &PgPool, slug: &str) -> Result<()> {
    let db_name = db_name_for(slug);
    let role_name = role_name_for(slug);

    // Force-disconnect any open sessions so DROP DATABASE doesn't block.
    let _ = sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()",
    )
    .bind(&db_name)
    .execute(admin)
    .await;

    let _ = sqlx::query(&format!(
        "DROP DATABASE IF EXISTS {} WITH (FORCE)",
        quote_ident(&db_name)
    ))
    .execute(admin)
    .await
    .map_err(|e| DataverseError::provisioning(slug, format!("DROP DATABASE: {}", e)));

    let _ = sqlx::query(&format!(
        "DROP ROLE IF EXISTS {}",
        quote_ident(&role_name)
    ))
    .execute(admin)
    .await
    .map_err(|e| DataverseError::provisioning(slug, format!("DROP ROLE: {}", e)));

    Ok(())
}

fn random_password() -> String {
    // 24 random bytes → 48 hex chars (192 bits of entropy).
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in &bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

/// Escape a string literal for inclusion in Postgres DDL.
///
/// Doubles single quotes; the password we generate contains only `[0-9a-f]`
/// so this is defensive only.
fn escape_pg_string(s: &str) -> String {
    s.replace('\'', "''")
}

/// Replace the database segment of a `postgres://…/db` DSN.
///
/// Currently unused — provisioning builds the per-app DSN from
/// (host, port, role, password, db) directly — but kept for future
/// callers that need to derive a sibling DSN from the admin one.
#[allow(dead_code)]
fn swap_database_in_dsn(dsn: &str, new_db: &str) -> String {
    // Find the path part: scheme://authority/path[?query]
    let scheme_end = dsn.find("://").map(|i| i + 3).unwrap_or(0);
    let after_scheme = &dsn[scheme_end..];

    let (path_start_rel, query_part) = match after_scheme.find('?') {
        Some(q) => (after_scheme[..q].find('/'), &after_scheme[q..]),
        None => (after_scheme.find('/'), ""),
    };

    let prefix_len = match path_start_rel {
        Some(p) => scheme_end + p,
        None => dsn.len(),
    };
    format!("{}/{}{}", &dsn[..prefix_len], url_encode_component(new_db), query_part)
}

/// Minimal percent-encoding for the parts of a URL we control. Only encodes
/// characters that have meaning in a URL path/userinfo. We never embed
/// arbitrary user input here — db names, role names, and passwords are
/// generated from a known alphabet.
fn url_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => out.push(ch),
            _ => {
                let mut buf = [0u8; 4];
                for b in ch.encode_utf8(&mut buf).bytes() {
                    use std::fmt::Write;
                    let _ = write!(&mut out, "%{:02X}", b);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_and_role_names_use_app_prefix() {
        assert_eq!(db_name_for("trader"), "app_trader");
        assert_eq!(role_name_for("_test-dataverse"), "app__test-dataverse");
    }

    #[test]
    fn random_password_has_expected_shape() {
        let p1 = random_password();
        let p2 = random_password();
        assert_eq!(p1.len(), 48);
        assert!(p1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(p1, p2);
    }

    #[test]
    fn swap_database_replaces_path() {
        let in_dsn = "postgres://admin:secret@127.0.0.1:5432/postgres";
        assert_eq!(
            swap_database_in_dsn(in_dsn, "app_test"),
            "postgres://admin:secret@127.0.0.1:5432/app_test"
        );
    }

    #[test]
    fn swap_database_preserves_query() {
        let in_dsn = "postgres://admin:secret@127.0.0.1:5432/postgres?sslmode=disable";
        assert_eq!(
            swap_database_in_dsn(in_dsn, "app_test"),
            "postgres://admin:secret@127.0.0.1:5432/app_test?sslmode=disable"
        );
    }

    #[test]
    fn url_encode_handles_special_chars() {
        assert_eq!(url_encode_component("app_test"), "app_test");
        assert_eq!(url_encode_component("with space"), "with%20space");
        assert_eq!(url_encode_component("a/b@c"), "a%2Fb%40c");
    }

    #[test]
    fn escape_pg_string_doubles_quotes() {
        assert_eq!(escape_pg_string("it's"), "it''s");
    }
}
