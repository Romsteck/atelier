use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Supported field types for Dataverse columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Number,
    Decimal,
    Boolean,
    DateTime,
    Date,
    Time,
    Email,
    Url,
    Phone,
    Currency,
    Percent,
    Duration,
    Json,
    Uuid,
    AutoIncrement,
    Choice,
    MultiChoice,
    Lookup,
    Formula,
}

impl FieldType {
    /// Infer a FieldType from a SQLite column type + column name heuristics.
    pub fn from_sqlite_affinity(sqlite_type: &str, col_name: &str) -> Self {
        let ty = sqlite_type.to_uppercase();
        let name = col_name.to_lowercase();

        // Name-based heuristics (more specific, checked first)
        if name == "id" || name.ends_with("_id") {
            if ty.contains("INTEGER") {
                return if name == "id" {
                    Self::AutoIncrement
                } else {
                    Self::Number // could be Lookup, resolved later via FK detection
                };
            }
        }
        if name.ends_with("_at") || name == "created_at" || name == "updated_at" {
            return Self::DateTime;
        }
        if name.contains("date") {
            return Self::Date;
        }
        if name.contains("email") {
            return Self::Email;
        }
        if name.contains("url") || name.contains("link") || name.contains("href") {
            return Self::Url;
        }
        if name.contains("phone") || name.contains("tel") {
            return Self::Phone;
        }
        if name.starts_with("is_") || name.starts_with("has_") || name == "active" || name == "enabled" {
            return Self::Boolean;
        }

        // SQLite type affinity
        if ty.contains("INT") {
            return Self::Number;
        }
        if ty.contains("REAL") || ty.contains("FLOAT") || ty.contains("DOUBLE") || ty.contains("NUMERIC") {
            return Self::Decimal;
        }
        if ty.contains("BLOB") {
            return Self::Json;
        }
        if ty.contains("BOOL") {
            return Self::Boolean;
        }

        // Default: TEXT → Text
        Self::Text
    }

    /// Returns the SQLite column type for this field type.
    pub fn sqlite_type(&self) -> &'static str {
        match self {
            Self::Text
            | Self::Email
            | Self::Url
            | Self::Phone
            | Self::Json
            | Self::Uuid
            | Self::Choice
            | Self::MultiChoice
            | Self::Formula
            | Self::Duration => "TEXT",
            Self::Number | Self::AutoIncrement => "INTEGER",
            Self::Decimal | Self::Currency | Self::Percent => "REAL",
            Self::Boolean => "INTEGER", // 0/1
            Self::DateTime => "TEXT",   // ISO 8601
            Self::Date => "TEXT",       // YYYY-MM-DD
            Self::Time => "TEXT",       // HH:MM:SS
            Self::Lookup => "INTEGER",  // foreign key
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDefinition {
    pub name: String,
    pub field_type: FieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Available choices for Choice/MultiChoice fields.
    #[serde(default)]
    pub choices: Vec<String>,
    /// SQL expression for Formula fields (GENERATED ALWAYS AS).
    #[serde(default)]
    pub formula_expression: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDefinition {
    pub name: String,
    pub slug: String,
    pub columns: Vec<ColumnDefinition>,
    #[serde(default)]
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    OneToMany,
    ManyToMany,
    SelfReferential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CascadeAction {
    Cascade,
    SetNull,
    Restrict,
}

impl Default for CascadeAction {
    fn default() -> Self {
        Self::Restrict
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeRules {
    #[serde(default)]
    pub on_delete: CascadeAction,
    #[serde(default)]
    pub on_update: CascadeAction,
}

impl Default for CascadeRules {
    fn default() -> Self {
        Self {
            on_delete: CascadeAction::Restrict,
            on_update: CascadeAction::Cascade,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationDefinition {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub relation_type: RelationType,
    #[serde(default)]
    pub cascade: CascadeRules,
}

/// Full database schema metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatabaseSchema {
    pub tables: Vec<TableDefinition>,
    pub relations: Vec<RelationDefinition>,
    pub version: u64,
    pub updated_at: Option<DateTime<Utc>>,
}
