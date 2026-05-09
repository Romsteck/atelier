use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Event emitted when schema changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaChangedEvent {
    pub table_name: String,
    pub operation: SchemaOperation,
    pub version: u64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaOperation {
    TableCreated,
    TableDropped,
    ColumnAdded,
    ColumnRemoved,
    RelationCreated,
    RelationDropped,
}

/// Event emitted when data changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataChangedEvent {
    pub table_name: String,
    pub operation: DataOperation,
    pub row_count: usize,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataOperation {
    Insert,
    Update,
    Delete,
}
