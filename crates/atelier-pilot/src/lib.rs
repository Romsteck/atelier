//! Atelier Pilote: project backlog and safe autonomous execution.

#[allow(unused_imports)]
pub(crate) mod sqlx {
    pub use sqlx_core::executor::Executor;
    pub use sqlx_core::pool::Pool;
    pub use sqlx_core::query::query;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::row::Row;
    pub use sqlx_postgres::{PgPool, PgRow, Postgres};
}

pub mod backlog;
pub mod engine;
pub mod gitops;
pub mod runs;
pub mod schedule;
pub mod service;

pub use backlog::{BacklogItem, BacklogPatch, BacklogStore, NewBacklogItem, Question};
pub use engine::{ClaudeWorkerEngine, CodexWorkerEngine, EnginePolicy, WorkerEvent, WorkerExec};
pub use runs::{BacklogRun, RunsStore};
pub use schedule::{NightSnapshot, PilotSchedule, ScheduleStore};
pub use service::{
    AtelierWorkerReport, PilotConfig, PilotEvent, PilotHooks, PilotService, TranscriptLine,
};

pub const INIT_SQL: &str = include_str!("../migrations/001_init.sql");

pub async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(INIT_SQL).execute(pool).await?;
    Ok(())
}
