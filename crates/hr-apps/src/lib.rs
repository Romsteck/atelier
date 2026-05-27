//! hr-apps — Application management for HomeRoute (replaces hr-environment + env-agent).

pub mod context;
pub mod db_manager;
pub mod port_registry;
pub mod registry;
pub mod supervisor;
pub mod todos;
pub mod types;

pub use context::ContextGenerator;
pub use db_manager::{DbManager, QueryResult, TableColumn, TableRelation, TableSchema};
pub use hr_db::engine::SyncResult;
pub use hr_db::schema::{ColumnDefinition, DatabaseSchema, RelationDefinition, TableDefinition};
pub use hr_db::query::{Filter, FilterOp, Pagination};
pub use port_registry::PortRegistry;
pub use registry::AppRegistry;
pub use supervisor::{AppSupervisor, ProcessStatus};
pub use types::*;
