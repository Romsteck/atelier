//! Dynamic GraphQL surface generated from `_dv_*` metadata.
//!
//! For each user table, the builder produces:
//! - an Object type carrying scalar columns plus implicit `id`,
//!   `created_at`, `updated_at` (Lookup expansion is wired but deferred);
//! - a `*Where` input for filtering with Hasura-style operators;
//! - a `*OrderBy` input for sorting;
//! - `*Insert` / `*Update` inputs for mutations;
//! - root Query fields `<table>`, `<table>ById`, `<table>Count`;
//! - root Mutation fields `insert<Table>`, `update<Table>`, `delete<Table>`.
//!
//! The schema is rebuilt whenever `_dv_meta.schema_version` changes; an
//! `Arc<Schema>` is cached per-engine ([`crate::engine::DataverseEngine`]).

pub mod builder;
pub mod cache;
pub mod filters;
pub mod naming;
pub mod sql;

pub use builder::build_schema;
pub use cache::SchemaCache;
