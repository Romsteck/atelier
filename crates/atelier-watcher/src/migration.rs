use std::time::Duration;

use crate::sqlx::{AssertSqlSafe, PgPoolOptions, Pool, Postgres, query, query_as, raw_sql};

pub const INIT_SQL: &str = include_str!("../migrations/001_init.sql");

pub const DEFAULT_DB_NAME: &str = "atelier_meta";

pub async fn open_admin_pool(dsn: &str) -> anyhow::Result<Pool<Postgres>> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(dsn)
        .await?;
    Ok(pool)
}

pub async fn open_pool(dsn: &str) -> anyhow::Result<Pool<Postgres>> {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(dsn)
        .await?;
    Ok(pool)
}

pub async fn ensure_database(admin_pool: &Pool<Postgres>, dbname: &str) -> anyhow::Result<()> {
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

pub async fn run_migrations(pool: &Pool<Postgres>) -> anyhow::Result<()> {
    raw_sql(INIT_SQL).execute(pool).await?;
    Ok(())
}

/// Swap the database segment of a Postgres DSN. Lifted from atelier-logging
/// to avoid a cross-crate dep just for this helper.
pub fn swap_db(dsn: &str, dbname: &str) -> String {
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
