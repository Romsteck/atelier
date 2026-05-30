use serde::{Deserialize, Serialize};

use crate::schema::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum MigrationOp {
    CreateTable(TableDefinition),
    AddColumn {
        table: String,
        column: ColumnDefinition,
    },
    RemoveColumn {
        table: String,
        column: String,
    },
    RenameColumn {
        table: String,
        old_name: String,
        new_name: String,
    },
    DropTable {
        table: String,
    },
    CreateRelation {
        relation: RelationDefinition,
    },
    DropRelation {
        from_table: String,
        from_column: String,
        to_table: String,
        to_column: String,
    },
}

/// Generate DDL SQL for a migration operation.
pub fn generate_ddl(op: &MigrationOp) -> Vec<String> {
    match op {
        MigrationOp::CreateTable(table) => {
            let mut cols = vec![
                "\"id\" INTEGER PRIMARY KEY AUTOINCREMENT".to_string(),
                "\"created_at\" TEXT NOT NULL DEFAULT (datetime('now'))".to_string(),
                "\"updated_at\" TEXT NOT NULL DEFAULT (datetime('now'))".to_string(),
            ];
            for col in &table.columns {
                if col.field_type == FieldType::Formula {
                    if let Some(ref expr) = col.formula_expression {
                        cols.push(format!(
                            "\"{}\" TEXT GENERATED ALWAYS AS ({}) STORED",
                            col.name, expr
                        ));
                        continue;
                    }
                }
                let mut def = format!("\"{}\" {}", col.name, col.field_type.sqlite_type());
                if col.required {
                    def.push_str(" NOT NULL");
                }
                if col.unique {
                    def.push_str(" UNIQUE");
                }
                if let Some(ref dv) = col.default_value {
                    def.push_str(&format!(" DEFAULT '{}'", dv.replace('\'', "''")));
                }
                cols.push(def);
            }
            vec![format!(
                "CREATE TABLE \"{}\" (\n  {}\n)",
                table.name,
                cols.join(",\n  ")
            )]
        }
        MigrationOp::AddColumn { table, column } => {
            if column.field_type == FieldType::Formula {
                // SQLite does not support ALTER TABLE ADD COLUMN ... GENERATED ALWAYS AS
                return vec![];
            }
            let mut def = format!(
                "ALTER TABLE \"{}\" ADD COLUMN \"{}\" {}",
                table,
                column.name,
                column.field_type.sqlite_type()
            );
            if column.required {
                // SQLite doesn't allow NOT NULL without default on ADD COLUMN
                if let Some(ref dv) = column.default_value {
                    def.push_str(&format!(" NOT NULL DEFAULT '{}'", dv.replace('\'', "''")));
                }
            }
            if let Some(ref dv) = column.default_value {
                if !column.required {
                    def.push_str(&format!(" DEFAULT '{}'", dv.replace('\'', "''")));
                }
            }
            vec![def]
        }
        MigrationOp::RemoveColumn { table, column } => {
            vec![format!(
                "ALTER TABLE \"{}\" DROP COLUMN \"{}\"",
                table, column
            )]
        }
        MigrationOp::RenameColumn {
            table,
            old_name,
            new_name,
        } => {
            vec![format!(
                "ALTER TABLE \"{}\" RENAME COLUMN \"{}\" TO \"{}\"",
                table, old_name, new_name
            )]
        }
        MigrationOp::DropTable { table } => {
            vec![format!("DROP TABLE IF EXISTS \"{}\"", table)]
        }
        MigrationOp::CreateRelation { .. } | MigrationOp::DropRelation { .. } => {
            // Relations are tracked in metadata only (SQLite FK constraints on table creation)
            vec![]
        }
    }
}
