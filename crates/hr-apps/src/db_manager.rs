use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tracing::info;

use hr_db::engine::{DataverseEngine, SyncResult};
use hr_db::query::{Filter, Pagination, query_rows, query_rows_expanded};
use hr_db::schema::{ColumnDefinition, DatabaseSchema, FieldType, RelationDefinition, TableDefinition};

/// Schema description for a single SQLite table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<TableColumn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<TableRelation>,
    pub row_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    pub name: String,
    pub field_type: FieldType,
    pub required: bool,
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula_expression: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRelation {
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub display_column: String,
}

impl From<&ColumnDefinition> for TableColumn {
    fn from(c: &ColumnDefinition) -> Self {
        Self {
            name: c.name.clone(),
            field_type: c.field_type.clone(),
            required: c.required,
            unique: c.unique,
            choices: c.choices.clone(),
            formula_expression: c.formula_expression.clone(),
        }
    }
}

/// Result of a SELECT query: column names + rows as JSON values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Value>,
    pub total: u64,
}

/// Per-app SQLite database manager. One database per app at `{apps_root}/{slug}/db.sqlite`.
#[derive(Clone)]
pub struct DbManager {
    apps_root: PathBuf,
    engines: Arc<RwLock<HashMap<String, Arc<Mutex<DataverseEngine>>>>>,
}

impl DbManager {
    pub fn new(apps_root: impl Into<PathBuf>) -> Self {
        let apps_root = apps_root.into();
        info!(path = %apps_root.display(), "DbManager initialized");
        Self {
            apps_root,
            engines: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Resolve the SQLite file path for an app.
    pub async fn db_path(&self, slug: &str) -> PathBuf {
        self.apps_root.join(slug).join("db.sqlite")
    }

    /// List all user-defined tables (excluding `_dv_*` metadata tables).
    pub async fn list_tables(&self, slug: &str) -> Result<Vec<String>> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        let schema = engine.get_schema().map_err(|e| anyhow!("{e}"))?;
        Ok(schema.tables.into_iter().map(|t| t.name).collect())
    }

    /// Describe a single table (columns + row count + relations).
    pub async fn describe_table(&self, slug: &str, table: &str) -> Result<TableSchema> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        let schema = engine.get_schema().map_err(|e| anyhow!("{e}"))?;
        let table_def = schema
            .tables
            .iter()
            .find(|t| t.name == table)
            .ok_or_else(|| anyhow!("table not found: {table}"))?;
        let row_count = engine.count_rows(table).unwrap_or(0);

        // Build relations for this table's Lookup columns
        let mut relations = Vec::new();
        for rel in &schema.relations {
            if rel.from_table == table {
                // Determine display column on target table
                let display_col = schema
                    .tables
                    .iter()
                    .find(|t| t.name == rel.to_table)
                    .map(|t| find_display_column(&t.columns))
                    .unwrap_or_else(|| "id".to_string());
                relations.push(TableRelation {
                    from_column: rel.from_column.clone(),
                    to_table: rel.to_table.clone(),
                    to_column: rel.to_column.clone(),
                    display_column: display_col,
                });
            }
        }

        Ok(TableSchema {
            name: table_def.name.clone(),
            columns: table_def.columns.iter().map(TableColumn::from).collect(),
            relations,
            row_count,
        })
    }

    /// Run a parameterised SELECT against the app DB.
    /// `sql` should be a SELECT statement; `params` are bound positionally.
    pub async fn query(&self, slug: &str, sql: &str, params: Vec<Value>) -> Result<QueryResult> {
        let trimmed = sql.trim_start();
        if !trimmed.to_lowercase().starts_with("select") {
            return Err(anyhow!("only SELECT statements are allowed"));
        }
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        let conn = engine.connection();

        let mut stmt = conn.prepare(sql).map_err(|e| anyhow!("prepare: {e}"))?;
        let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let bound: Vec<Box<dyn rusqlite::types::ToSql>> = params.iter().map(json_to_sql).collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = bound.iter().map(|b| b.as_ref()).collect();

        let names_for_map = column_names.clone();
        let mapped = stmt
            .query_map(rusqlite::params_from_iter(refs.iter()), move |row| {
                let mut obj = serde_json::Map::new();
                for (i, name) in names_for_map.iter().enumerate() {
                    obj.insert(name.clone(), sqlite_value_to_json(row, i));
                }
                Ok(Value::Object(obj))
            })
            .map_err(|e| anyhow!("query: {e}"))?;

        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|e| anyhow!("row: {e}"))?);
        }
        let total = rows.len() as u64;
        Ok(QueryResult {
            columns: column_names,
            rows,
            total,
        })
    }

    /// Execute a mutation (INSERT/UPDATE/DELETE) against the app DB.
    /// Returns the number of affected rows.
    pub async fn execute(&self, slug: &str, sql: &str, params: Vec<Value>) -> Result<usize> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        let conn = engine.connection();

        let bound: Vec<Box<dyn rusqlite::types::ToSql>> = params.iter().map(json_to_sql).collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = bound.iter().map(|b| b.as_ref()).collect();

        let rows_affected = conn
            .execute(sql, rusqlite::params_from_iter(refs.iter()))
            .map_err(|e| anyhow!("execute: {e}"))?;
        info!(app_slug = slug, rows_affected, "DB execute ok");
        Ok(rows_affected)
    }

    /// Convenience: list rows from a table with filters/pagination.
    pub async fn select_rows(
        &self,
        slug: &str,
        table: &str,
        filters: &[Filter],
        pagination: &Pagination,
    ) -> Result<QueryResult> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        let rows = query_rows(engine.connection(), table, filters, pagination)
            .map_err(|e| anyhow!("{e}"))?;
        let columns = rows
            .first()
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let total = engine.count_rows(table).unwrap_or(0);
        Ok(QueryResult {
            columns,
            rows,
            total,
        })
    }

    /// Structured query with optional Lookup expansion via LEFT JOIN.
    pub async fn select_rows_expanded(
        &self,
        slug: &str,
        table: &str,
        filters: &[Filter],
        pagination: &Pagination,
        expand: &[String],
    ) -> Result<QueryResult> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;

        if expand.is_empty() {
            let rows = query_rows(engine.connection(), table, filters, pagination)
                .map_err(|e| anyhow!("{e}"))?;
            let columns = rows
                .first()
                .and_then(|v| v.as_object())
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let total = engine.count_rows(table).unwrap_or(0);
            return Ok(QueryResult {
                columns,
                rows,
                total,
            });
        }

        // Resolve expand columns to join info from relations
        let schema = engine.get_schema().map_err(|e| anyhow!("{e}"))?;
        let mut expand_info = Vec::new();
        for col_name in expand {
            if let Some(rel) = schema
                .relations
                .iter()
                .find(|r| r.from_table == table && r.from_column == *col_name)
            {
                let display_col = schema
                    .tables
                    .iter()
                    .find(|t| t.name == rel.to_table)
                    .map(|t| find_display_column(&t.columns))
                    .unwrap_or_else(|| "id".to_string());
                expand_info.push((col_name.as_str(), rel.to_table.as_str(), rel.to_column.as_str(), display_col));
            }
        }

        let expand_refs: Vec<(&str, &str, &str, &str)> = expand_info
            .iter()
            .map(|(a, b, c, d)| (*a, *b, *c, d.as_str()))
            .collect();

        let rows = query_rows_expanded(
            engine.connection(),
            table,
            filters,
            pagination,
            &expand_refs,
        )
        .map_err(|e| anyhow!("{e}"))?;

        let columns = rows
            .first()
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        let total = engine.count_rows(table).unwrap_or(0);
        Ok(QueryResult {
            columns,
            rows,
            total,
        })
    }

    /// Sync SQLite tables into Dataverse metadata.
    pub async fn sync_schema(&self, slug: &str) -> Result<SyncResult> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.sync_schema().map_err(|e| anyhow!("{e}"))
    }

    /// Get full database schema (tables + relations + version).
    pub async fn get_schema(&self, slug: &str) -> Result<DatabaseSchema> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.get_schema().map_err(|e| anyhow!("{e}"))
    }

    /// Create a new table from a TableDefinition.
    pub async fn create_table(&self, slug: &str, definition: TableDefinition) -> Result<u64> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.create_table(&definition).map_err(|e| anyhow!("{e}"))
    }

    /// Drop a table.
    pub async fn drop_table(&self, slug: &str, table: &str) -> Result<u64> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.drop_table(table).map_err(|e| anyhow!("{e}"))
    }

    /// Add a column to a table.
    pub async fn add_column(&self, slug: &str, table: &str, column: ColumnDefinition) -> Result<u64> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.add_column(table, &column).map_err(|e| anyhow!("{e}"))
    }

    /// Remove a column from a table.
    pub async fn remove_column(&self, slug: &str, table: &str, column: &str) -> Result<u64> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.remove_column(table, column).map_err(|e| anyhow!("{e}"))
    }

    /// Create a relation between tables.
    pub async fn create_relation(&self, slug: &str, relation: RelationDefinition) -> Result<u64> {
        let engine = self.get_engine(slug).await?;
        let engine = engine.lock().await;
        engine.create_relation(&relation).map_err(|e| anyhow!("{e}"))
    }

    async fn get_engine(&self, slug: &str) -> Result<Arc<Mutex<DataverseEngine>>> {
        {
            let engines = self.engines.read().await;
            if let Some(e) = engines.get(slug) {
                return Ok(e.clone());
            }
        }
        let db_path = self.db_path(slug).await;
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let engine =
            DataverseEngine::open(&db_path).map_err(|e| anyhow!("open db for '{slug}': {e}"))?;

        // Auto-sync: import SQLite tables into Dataverse metadata on first load
        match engine.sync_schema() {
            Ok(r) if !r.tables_added.is_empty() => {
                info!(app_slug = slug, tables = ?r.tables_added, "auto-sync imported tables");
            }
            Err(e) => {
                tracing::warn!(app_slug = slug, error = %e, "auto-sync failed");
            }
            _ => {}
        }

        let engine = Arc::new(Mutex::new(engine));
        let mut engines = self.engines.write().await;
        engines.insert(slug.to_string(), engine.clone());
        info!(app_slug = slug, path = %db_path.display(), "db opened");
        Ok(engine)
    }
}

/// Heuristic: pick the best "display" column for a table.
/// Prefers "name", "title", "label", then first Text column, then "id".
fn find_display_column(columns: &[ColumnDefinition]) -> String {
    for preferred in &["name", "title", "label"] {
        if columns.iter().any(|c| c.name == *preferred) {
            return preferred.to_string();
        }
    }
    columns
        .iter()
        .find(|c| c.field_type == FieldType::Text)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "id".to_string())
}

fn json_to_sql(v: &Value) -> Box<dyn rusqlite::types::ToSql> {
    match v {
        Value::Null => Box::new(rusqlite::types::Null),
        Value::Bool(b) => Box::new(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else if let Some(f) = n.as_f64() {
                Box::new(f)
            } else {
                Box::new(n.to_string())
            }
        }
        Value::String(s) => Box::new(s.clone()),
        other => Box::new(other.to_string()),
    }
}

fn sqlite_value_to_json(row: &rusqlite::Row<'_>, idx: usize) -> Value {
    use rusqlite::types::ValueRef;
    match row.get_ref(idx) {
        Ok(ValueRef::Null) => Value::Null,
        Ok(ValueRef::Integer(i)) => Value::from(i),
        Ok(ValueRef::Real(f)) => Value::from(f),
        Ok(ValueRef::Text(t)) => Value::from(String::from_utf8_lossy(t).to_string()),
        Ok(ValueRef::Blob(b)) => Value::from(format!("<blob {} bytes>", b.len())),
        Err(_) => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn db_path_layout() {
        let dir = TempDir::new().unwrap();
        let mgr = DbManager::new(dir.path().to_path_buf());
        let p = mgr.db_path("trader").await;
        assert_eq!(p, dir.path().join("trader").join("db.sqlite"));
    }

    #[tokio::test]
    async fn open_and_list_tables_empty() {
        let dir = TempDir::new().unwrap();
        let mgr = DbManager::new(dir.path().to_path_buf());
        let tables = mgr.list_tables("trader").await.unwrap();
        assert!(tables.is_empty());
    }

    #[tokio::test]
    async fn query_rejects_non_select() {
        let dir = TempDir::new().unwrap();
        let mgr = DbManager::new(dir.path().to_path_buf());
        let res = mgr.query("trader", "DELETE FROM x", vec![]).await;
        assert!(res.is_err());
    }
}
