//! Per-app documentation: overview + screens + features + components + mermaid diagrams.
//!
//! Storage is hybrid:
//! - Filesystem at `/var/lib/atelier/docs/{app_id}/` is the source of truth (one `.md`
//!   per entry with YAML frontmatter, plus `.mmd` files for mermaid diagrams).
//! - Postgres (`atelier_meta`, table `doc_entries`) is a rebuildable cache used
//!   exclusively for full-text search (tsvector + GIN).
//!
//! See `model.rs` for types, `fs.rs` for filesystem IO, `index.rs` for the Postgres index,
//! and `migrate.rs` for legacy → v2 migration.

pub mod fs;
pub mod index;
pub mod migrate;
pub mod model;

pub use fs::{Store, StoreError};
pub use index::{Index, IndexError, SearchHit};
pub use migrate::{MigrateReport, run_all};
pub use model::{
    DocEntry, DocType, Frontmatter, Meta, Overview, Scope, validate_app_id, validate_entry_name,
};

/// Default root for filesystem storage.
pub const DEFAULT_DOCS_DIR: &str = "/var/lib/atelier/docs";

/// Schema version written into `meta.json`. Bumped on breaking changes.
pub const SCHEMA_VERSION: u32 = 2;
