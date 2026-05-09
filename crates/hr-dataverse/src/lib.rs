//! `hr-dataverse` — Dataverse-like managed database engine on top of
//! PostgreSQL with a dynamically-generated GraphQL surface.
//!
//! This crate is the long-term replacement for `hr-db` (legacy SQLite). The
//! two coexist during the migration window and are selected at runtime by
//! `hr-apps` via the `db_backend` flag on each `Application`.
//!
//! Top-level concepts:
//! - [`schema`] — Dataverse-flavored types ([`schema::FieldType`],
//!   [`schema::TableDefinition`], …) and their mapping to Postgres /
//!   GraphQL.
//! - [`migration`] — DDL generators (`CREATE TABLE`, `ALTER`, FK).
//! - [`engine::DataverseEngine`] — bound to one app's database, exposes
//!   schema introspection and schema mutations.
//! - [`provisioning`] — `CREATE DATABASE`/`CREATE ROLE` per app.
//! - [`manager::DataverseManager`] — owns the admin pool, caches
//!   per-app engines, persists secrets.
//!
//! GraphQL execution lives under the `graphql` module (Phase B of the
//! refactor — currently a stub).

/// Internal re-exports to mimic the `sqlx` meta-crate facade — see the
/// workspace `Cargo.toml` comment for the dependency-graph rationale.
///
/// Items are added here as new modules need them.
#[allow(unused_imports)]
pub(crate) mod sqlx {
    pub use sqlx_core::Error;
    pub use sqlx_core::executor::Executor;
    pub use sqlx_core::pool::Pool;
    pub use sqlx_core::query::query;
    pub use sqlx_core::query_as::query_as;
    pub use sqlx_core::query_scalar::query_scalar;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::transaction::Transaction;
    pub use sqlx_postgres::{
        PgConnection, PgPool, PgPoolOptions, PgRow, Postgres,
    };
}

pub mod error;
pub mod schema;
pub mod validation;
pub mod migration;
pub mod provisioning;
pub mod engine;
pub mod manager;
pub mod query;
pub mod crud;
pub mod audit;
pub mod dv_io;
pub mod graphql;

// Re-exports for the most common API surface so callers can use
// `hr_dataverse::DataverseEngine` directly.
pub use crate::engine::DataverseEngine;
pub use crate::error::{DataverseError, Result};
pub use crate::manager::{
    AppSecret, DataverseManager, SecretsFile,
};
pub use crate::provisioning::{
    ProvisioningConfig, ProvisioningResult, db_name_for, role_name_for,
};
pub use crate::schema::{
    CascadeAction, CascadeRules, ColumnDefinition, DatabaseSchema, FieldType, IdStrategy,
    RelationDefinition, RelationType, TableDefinition,
};
pub use crate::validation::ValidationError;
