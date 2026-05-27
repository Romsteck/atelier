use std::time::Duration;

use crate::sqlx::{Pool, PgPoolOptions, Postgres, query, query_as, raw_sql};

pub const INIT_SQL: &str = include_str!("../migrations/001_init.sql");

pub async fn open_admin_pool(dsn: &str) -> anyhow::Result<Pool<Postgres>> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(dsn)
        .await?;
    Ok(pool)
}

pub async fn open_writer_pool(dsn: &str) -> anyhow::Result<Pool<Postgres>> {
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
        query(&stmt).execute(admin_pool).await?;
    }
    Ok(())
}

pub async fn ensure_writer_role(
    admin_pool: &Pool<Postgres>,
    role: &str,
    password: &str,
) -> anyhow::Result<()> {
    let exists: Option<(i32,)> = query_as("SELECT 1 FROM pg_roles WHERE rolname = $1")
        .bind(role)
        .fetch_optional(admin_pool)
        .await?;
    if exists.is_none() {
        let stmt = format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}'",
            role.replace('"', "\"\""),
            password.replace('\'', "''")
        );
        query(&stmt).execute(admin_pool).await?;
    }
    Ok(())
}

pub async fn run_migrations(pool: &Pool<Postgres>) -> anyhow::Result<()> {
    raw_sql(INIT_SQL).execute(pool).await?;
    Ok(())
}

pub async fn ensure_partition(
    pool: &Pool<Postgres>,
    date: chrono::NaiveDate,
) -> anyhow::Result<()> {
    query("SELECT ensure_partition($1)")
        .bind(date)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn drop_partitions_before(
    pool: &Pool<Postgres>,
    cutoff: chrono::NaiveDate,
) -> anyhow::Result<i32> {
    let (count,): (i32,) = query_as("SELECT drop_partitions_before($1)")
        .bind(cutoff)
        .fetch_one(pool)
        .await?;
    Ok(count)
}
