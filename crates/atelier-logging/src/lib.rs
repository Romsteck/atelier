/// Internal re-export facade so we can write `sqlx::query_as(...)` without
/// turbofish boilerplate — see `hr-dataverse` for the same pattern.
#[allow(unused_imports)]
pub(crate) mod sqlx {
    pub use sqlx_core::Error;
    pub use sqlx_core::executor::Executor;
    pub use sqlx_core::pool::Pool;
    pub use sqlx_core::query::query;
    pub use sqlx_core::query_as::query_as;
    pub use sqlx_core::query_scalar::query_scalar;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::row::Row;
    pub use sqlx_core::sql_str::AssertSqlSafe;
    pub use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
}

pub mod types;
pub mod ring_buffer;
pub mod migration;
pub mod store;
pub mod query;
pub mod ingest;
pub mod layer;
pub mod shipper;

pub use ingest::{LogIngestConfig, LogIngestService};
pub use layer::LoggingLayer;
pub use shipper::{HttpShipperConfig, HttpShipperLayer};
pub use query::{LogQuery, LogStats, ServiceCount, LevelCount, AppCount};
pub use types::{LogEntry, LogLevel, LogCategory, LogSource, RawIngestEntry, LogEntryBuilder};
