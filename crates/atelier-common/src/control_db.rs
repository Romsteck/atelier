//! Control-plane Postgres store (database `atelier_meta`).
//!
//! Single shared `PgPool` opened once at boot and handed to the registry / port
//! / task / docs stores. Reaches `atelier_meta` via the dataverse admin DSN
//! (`ATELIER_DV_ADMIN_URL`) — the same database the surveillance subsystem
//! (atelier-watcher) uses, with its own idempotent DDL blob applied here.
//!
//! Mirrors the per-crate sqlx shim + bootstrap helpers used by atelier-watcher
//! and atelier-logging (duplicated on purpose — the house style avoids a
//! cross-crate dep just for these few helpers).

use std::time::Duration;

#[allow(unused_imports)]
pub mod sqlx {
    pub use sqlx_core::Error;
    pub use sqlx_core::executor::Executor;
    pub use sqlx_core::pool::Pool;
    pub use sqlx_core::query::query;
    pub use sqlx_core::query_as::query_as;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::row::Row;
    pub use sqlx_core::sql_str::AssertSqlSafe;
    pub use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
}

use sqlx::{AssertSqlSafe, PgPool, PgPoolOptions, Pool, Postgres, query, query_as, raw_sql};

pub const INIT_SQL: &str = include_str!("../migrations/001_control.sql");

pub const DEFAULT_DB_NAME: &str = "atelier_meta";

async fn open_admin_pool(dsn: &str) -> anyhow::Result<Pool<Postgres>> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(dsn)
        .await?;
    Ok(pool)
}

async fn open_pool(dsn: &str) -> anyhow::Result<Pool<Postgres>> {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(dsn)
        .await?;
    Ok(pool)
}

async fn ensure_database(admin_pool: &Pool<Postgres>, dbname: &str) -> anyhow::Result<()> {
    let exists: Option<(i32,)> = query_as("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(dbname)
        .fetch_optional(admin_pool)
        .await?;
    if exists.is_none() {
        let stmt = format!("CREATE DATABASE \"{}\"", dbname.replace('"', "\"\""));
        query(AssertSqlSafe(stmt)).execute(admin_pool).await?;
    }
    Ok(())
}

async fn run_migrations(pool: &Pool<Postgres>) -> anyhow::Result<()> {
    raw_sql(INIT_SQL).execute(pool).await?;
    Ok(())
}

/// Swap the database segment of a Postgres DSN (e.g. `.../postgres` → `.../atelier_meta`).
fn swap_db(dsn: &str, dbname: &str) -> String {
    if let Some((head, tail)) = dsn.rsplit_once('/') {
        let (_, after) = tail
            .split_once('?')
            .map(|(a, b)| (a, format!("?{}", b)))
            .unwrap_or((tail, String::new()));
        format!("{}/{}{}", head, dbname, after)
    } else {
        dsn.to_string()
    }
}

/// Open the `atelier_meta` pool and apply the control-plane DDL.
///
/// `admin_dsn` is the dataverse admin DSN (points at the `postgres` database).
/// Ensures the target DB exists, then connects to it and runs the idempotent
/// migrations. Returns the pool ready for the control-plane stores.
pub async fn bootstrap(admin_dsn: &str) -> anyhow::Result<PgPool> {
    let admin_pool = open_admin_pool(admin_dsn).await?;
    ensure_database(&admin_pool, DEFAULT_DB_NAME).await?;
    admin_pool.close().await;

    let target_dsn = swap_db(admin_dsn, DEFAULT_DB_NAME);
    let pool = open_pool(&target_dsn).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}
