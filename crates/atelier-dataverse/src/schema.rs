use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Strategy for the implicit `id` primary-key column of a user table.
///
/// `Bigserial` (default) gives an `id BIGINT` auto-incrementing PK and the
/// GraphQL `id` field is typed `Int!`. `Uuid` gives `id UUID DEFAULT
/// gen_random_uuid()` and the GraphQL `id` field is typed `String!`. The
/// strategy is per-table — a database can mix Bigserial and UUID tables,
/// although a Lookup column's storage type is always derived from the
/// target table's strategy at create time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdStrategy {
    #[default]
    Bigserial,
    Uuid,
}

impl IdStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bigserial => "bigserial",
            Self::Uuid => "uuid",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "bigserial" => Self::Bigserial,
            "uuid" => Self::Uuid,
            _ => return None,
        })
    }

    /// Postgres column type the implicit `id` column gets in DDL.
    pub fn pg_id_type(&self) -> &'static str {
        match self {
            Self::Bigserial => "BIGSERIAL",
            Self::Uuid => "UUID",
        }
    }

    /// Postgres column type for a Lookup column whose target uses this
    /// strategy. (BIGINT or UUID — never BIGSERIAL on the FK side.)
    pub fn pg_fk_type(&self) -> &'static str {
        match self {
            Self::Bigserial => "BIGINT",
            Self::Uuid => "UUID",
        }
    }

    /// Whether `id` should be emitted with a `DEFAULT gen_random_uuid()`
    /// clause (Bigserial declares the default via the SERIAL pseudo-type).
    pub fn id_default_clause(&self) -> &'static str {
        match self {
            Self::Bigserial => "",
            Self::Uuid => " DEFAULT gen_random_uuid()",
        }
    }
}

/// Supported field types for Dataverse columns.
///
/// Each variant maps to:
/// - a Postgres column type (`pg_type`)
/// - a GraphQL scalar/object name (`graphql_type_name`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Like `Currency` (NUMERIC(20,6)) but serialised as a JSON string
    /// instead of an f64 — preserves full precision for amounts beyond
    /// 2^53 cents. New apps should prefer this over `Currency`/`Decimal`
    /// when they care about precision. Existing apps can migrate column
    /// by column; cf. the C2 finding in the dataverse audit.
    Money,
}

impl FieldType {
    /// Postgres column type for this field type.
    ///
    /// Note: `Lookup` is `BIGINT` (foreign key to target.id), the FK is added
    /// separately via [`crate::migration::add_foreign_key`].
    /// `AutoIncrement` is only valid as the implicit `id` column (BIGSERIAL),
    /// users should not declare it manually.
    pub fn pg_type(&self) -> &'static str {
        match self {
            Self::Text | Self::Email | Self::Url | Self::Phone => "TEXT",
            Self::Number => "BIGINT",
            Self::Decimal | Self::Currency | Self::Percent | Self::Money => "NUMERIC(20, 6)",
            Self::Boolean => "BOOLEAN",
            Self::DateTime => "TIMESTAMPTZ",
            Self::Date => "DATE",
            Self::Time => "TIME",
            Self::Duration => "INTERVAL",
            Self::Json => "JSONB",
            Self::Uuid => "UUID",
            Self::AutoIncrement => "BIGSERIAL",
            Self::Choice => "TEXT",
            Self::MultiChoice => "TEXT[]",
            Self::Lookup => "BIGINT",
            // Formula columns use GENERATED ALWAYS AS (expr) STORED — the
            // base type is configurable in the future. For V1 we default
            // to TEXT and let the expression cast as needed.
            Self::Formula => "TEXT",
        }
    }

    /// GraphQL scalar/type name for this field type.
    ///
    /// `Lookup` returns `"Int"` here — the resolver layer rewrites this into
    /// the target object type when building the schema.
    pub fn graphql_type_name(&self) -> &'static str {
        match self {
            Self::Text | Self::Email | Self::Url | Self::Phone | Self::Time | Self::Duration
            | Self::Choice | Self::Decimal | Self::Currency | Self::Percent | Self::Money => "String",
            Self::Number | Self::AutoIncrement | Self::Lookup => "Int",
            Self::Boolean => "Boolean",
            Self::DateTime => "DateTime",
            Self::Date => "Date",
            Self::Json => "JSON",
            Self::Uuid => "UUID",
            Self::MultiChoice => "[String!]",
            Self::Formula => "String",
        }
    }

    /// Stable string code stored in `_dv_columns.field_type`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::DateTime => "date_time",
            Self::Date => "date",
            Self::Time => "time",
            Self::Email => "email",
            Self::Url => "url",
            Self::Phone => "phone",
            Self::Currency => "currency",
            Self::Percent => "percent",
            Self::Duration => "duration",
            Self::Json => "json",
            Self::Uuid => "uuid",
            Self::AutoIncrement => "auto_increment",
            Self::Choice => "choice",
            Self::MultiChoice => "multi_choice",
            Self::Lookup => "lookup",
            Self::Formula => "formula",
            Self::Money => "money",
        }
    }

    /// Inverse of [`as_str`].
    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "text" => Self::Text,
            "number" => Self::Number,
            "decimal" => Self::Decimal,
            "boolean" => Self::Boolean,
            "date_time" => Self::DateTime,
            "date" => Self::Date,
            "time" => Self::Time,
            "email" => Self::Email,
            "url" => Self::Url,
            "phone" => Self::Phone,
            "currency" => Self::Currency,
            "percent" => Self::Percent,
            "duration" => Self::Duration,
            "json" => Self::Json,
            "uuid" => Self::Uuid,
            "auto_increment" => Self::AutoIncrement,
            "choice" => Self::Choice,
            "multi_choice" => Self::MultiChoice,
            "lookup" => Self::Lookup,
            "formula" => Self::Formula,
            "money" => Self::Money,
            _ => return None,
        })
    }

    /// Infer a FieldType from a Postgres column type + column name heuristics.
    ///
    /// Used by `sync_schema` to (re)build Dataverse metadata from a database
    /// that may have been mutated outside the engine.
    pub fn from_pg_type(pg_type: &str, col_name: &str) -> Self {
        let ty = pg_type.to_uppercase();
        let name = col_name.to_lowercase();

        // Name-based heuristics first
        if (name == "id" || name.ends_with("_id")) && (ty.contains("BIGINT") || ty.contains("INT")) {
            return if name == "id" { Self::AutoIncrement } else { Self::Number };
        }
        if name.ends_with("_at") {
            return Self::DateTime;
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

        // Postgres type affinity
        match ty.as_str() {
            t if t.contains("TIMESTAMP") => Self::DateTime,
            "DATE" => Self::Date,
            "TIME" | "TIMETZ" => Self::Time,
            "INTERVAL" => Self::Duration,
            "BOOL" | "BOOLEAN" => Self::Boolean,
            t if t.contains("BIGSERIAL") => Self::AutoIncrement,
            t if t.contains("BIGINT") || t.contains("INT8") => Self::Number,
            t if t.contains("INT") => Self::Number,
            t if t.contains("NUMERIC") || t.contains("DECIMAL") => Self::Decimal,
            t if t.contains("REAL") || t.contains("FLOAT") || t.contains("DOUBLE") => Self::Decimal,
            "JSONB" | "JSON" => Self::Json,
            "UUID" => Self::Uuid,
            t if t.contains("[]") || t.contains("ARRAY") => Self::MultiChoice,
            _ => Self::Text,
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
    /// For Lookup fields: target table name (the column references {target}.id).
    #[serde(default)]
    pub lookup_target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDefinition {
    pub name: String,
    pub slug: String,
    pub columns: Vec<ColumnDefinition>,
    #[serde(default)]
    pub description: Option<String>,
    /// Primary-key strategy for this table's implicit `id` column.
    /// Defaults to [`IdStrategy::Bigserial`] for backward compatibility
    /// with all tables created before UUID-PK support landed.
    #[serde(default)]
    pub id_strategy: IdStrategy,
    /// Primary display column: the user column whose value stands in for a
    /// row when this table is referenced by a Lookup (DbExplorer cells,
    /// selectors, gateway `$expand`). `None` = auto mode — a heuristic picks
    /// it, see [`DatabaseSchema::effective_display_column`].
    #[serde(default)]
    pub display_column: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    OneToMany,
    ManyToMany,
    SelfReferential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

impl CascadeAction {
    pub fn as_sql(&self) -> &'static str {
        match self {
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::Restrict => "RESTRICT",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cascade => "cascade",
            Self::SetNull => "set_null",
            Self::Restrict => "restrict",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "cascade" => Self::Cascade,
            "set_null" => Self::SetNull,
            "restrict" => Self::Restrict,
            _ => return None,
        })
    }
}

impl RelationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OneToMany => "one_to_many",
            Self::ManyToMany => "many_to_many",
            Self::SelfReferential => "self_referential",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "one_to_many" => Self::OneToMany,
            "many_to_many" => Self::ManyToMany,
            "self_referential" => Self::SelfReferential,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CascadeRules {
    #[serde(default)]
    pub on_delete: CascadeAction,
    #[serde(default = "default_on_update")]
    pub on_update: CascadeAction,
}

fn default_on_update() -> CascadeAction {
    CascadeAction::Cascade
}

impl Default for CascadeRules {
    fn default() -> Self {
        Self { on_delete: CascadeAction::Restrict, on_update: CascadeAction::Cascade }
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

/// Full database schema metadata (snapshot read from `_dv_*` tables).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatabaseSchema {
    pub tables: Vec<TableDefinition>,
    pub relations: Vec<RelationDefinition>,
    pub version: u64,
    pub updated_at: Option<DateTime<Utc>>,
}

impl DatabaseSchema {
    /// Resolve the effective **primary display column** of `table`: the column
    /// whose value represents a row when the table is referenced by a Lookup
    /// (DbExplorer cells, lookup selectors, gateway `$expand`).
    ///
    /// Priority:
    /// 1. an explicit `display_column` that still exists and is textual;
    /// 2. a heuristic over the table's user (non-base) textual columns —
    ///    first of `name`, `title`, `label` (case-insensitive), else the
    ///    first textual column in declared order;
    /// 3. `"id"` as a last resort (callers treat `"id"` as "no readable
    ///    display" and skip enrichment, so behaviour degrades to the raw id).
    pub fn effective_display_column(&self, table: &TableDefinition) -> String {
        fn is_textual(ft: FieldType) -> bool {
            matches!(
                ft,
                FieldType::Text | FieldType::Email | FieldType::Url | FieldType::Phone
            )
        }

        // (1) Explicit override. An explicit `id` pin means "show the raw id"
        //     and is honoured verbatim. Any other pin is honoured only if the
        //     column still exists and is textual (a stale/retyped pin silently
        //     falls back to the heuristic).
        if let Some(explicit) = table.display_column.as_deref() {
            if explicit == "id" {
                return "id".to_string();
            }
            if table
                .columns
                .iter()
                .any(|c| c.name == explicit && is_textual(c.field_type))
            {
                return explicit.to_string();
            }
        }

        // (2) Heuristic over user textual columns. `table.columns` holds only
        //     user columns (base columns live in DDL, not `_dv_columns`), but
        //     filter defensively all the same.
        let textual: Vec<&ColumnDefinition> = table
            .columns
            .iter()
            .filter(|c| !crate::migration::is_base_column(&c.name) && is_textual(c.field_type))
            .collect();
        for preferred in ["name", "title", "label"] {
            if let Some(c) = textual
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(preferred))
            {
                return c.name.clone();
            }
        }
        if let Some(c) = textual.first() {
            return c.name.clone();
        }

        // (3) No readable column — the raw id is all we can show.
        "id".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_type_covers_all_variants() {
        // If a new variant is added without a pg_type mapping, this test
        // fails to compile (match exhaustiveness).
        for ty in [
            FieldType::Text, FieldType::Number, FieldType::Decimal, FieldType::Boolean,
            FieldType::DateTime, FieldType::Date, FieldType::Time, FieldType::Email,
            FieldType::Url, FieldType::Phone, FieldType::Currency, FieldType::Percent,
            FieldType::Duration, FieldType::Json, FieldType::Uuid, FieldType::AutoIncrement,
            FieldType::Choice, FieldType::MultiChoice, FieldType::Lookup, FieldType::Formula,
            FieldType::Money,
        ] {
            assert!(!ty.pg_type().is_empty());
            assert!(!ty.graphql_type_name().is_empty());
        }
    }

    #[test]
    fn from_pg_type_basics() {
        assert_eq!(FieldType::from_pg_type("BIGSERIAL", "id"), FieldType::AutoIncrement);
        assert_eq!(FieldType::from_pg_type("BIGINT", "company_id"), FieldType::Number);
        assert_eq!(FieldType::from_pg_type("TIMESTAMPTZ", "created_at"), FieldType::DateTime);
        assert_eq!(FieldType::from_pg_type("TEXT", "email"), FieldType::Email);
        assert_eq!(FieldType::from_pg_type("BOOLEAN", "is_active"), FieldType::Boolean);
        assert_eq!(FieldType::from_pg_type("JSONB", "data"), FieldType::Json);
        assert_eq!(FieldType::from_pg_type("UUID", "uid"), FieldType::Uuid);
    }

    fn col(name: &str, ft: FieldType) -> ColumnDefinition {
        ColumnDefinition {
            name: name.into(),
            field_type: ft,
            required: false,
            unique: false,
            default_value: None,
            description: None,
            choices: vec![],
            formula_expression: None,
            lookup_target: None,
        }
    }

    fn table(display_column: Option<&str>, columns: Vec<ColumnDefinition>) -> TableDefinition {
        TableDefinition {
            name: "t".into(),
            slug: "t".into(),
            columns,
            description: None,
            id_strategy: IdStrategy::default(),
            display_column: display_column.map(str::to_string),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn effective_display_column_prefers_name() {
        let schema = DatabaseSchema::default();
        let t = table(None, vec![col("title", FieldType::Text), col("name", FieldType::Text)]);
        // `name` wins over `title` regardless of declaration order.
        assert_eq!(schema.effective_display_column(&t), "name");
    }

    #[test]
    fn effective_display_column_falls_back_through_heuristic() {
        let schema = DatabaseSchema::default();
        // No name → title; no title → label; no preferred → first textual.
        assert_eq!(
            schema.effective_display_column(&table(None, vec![col("title", FieldType::Text)])),
            "title"
        );
        assert_eq!(
            schema.effective_display_column(&table(None, vec![col("label", FieldType::Text)])),
            "label"
        );
        assert_eq!(
            schema.effective_display_column(&table(
                None,
                vec![col("slug", FieldType::Text), col("descr", FieldType::Text)]
            )),
            "slug"
        );
    }

    #[test]
    fn effective_display_column_id_when_no_textual_column() {
        let schema = DatabaseSchema::default();
        let t = table(None, vec![col("qty", FieldType::Number), col("done", FieldType::Boolean)]);
        assert_eq!(schema.effective_display_column(&t), "id");
    }

    #[test]
    fn effective_display_column_honours_valid_explicit_pin() {
        let schema = DatabaseSchema::default();
        // Explicit textual pin beats the `name` heuristic.
        let t = table(Some("title"), vec![col("name", FieldType::Text), col("title", FieldType::Text)]);
        assert_eq!(schema.effective_display_column(&t), "title");
    }

    #[test]
    fn effective_display_column_ignores_stale_or_nontextual_pin() {
        let schema = DatabaseSchema::default();
        // Pin points at a column that no longer exists → heuristic.
        let gone = table(Some("ghost"), vec![col("name", FieldType::Text)]);
        assert_eq!(schema.effective_display_column(&gone), "name");
        // Pin points at a non-textual column → heuristic (here → id).
        let nontext = table(Some("qty"), vec![col("qty", FieldType::Number)]);
        assert_eq!(schema.effective_display_column(&nontext), "id");
    }

    #[test]
    fn effective_display_column_honours_explicit_id_pin() {
        let schema = DatabaseSchema::default();
        // Explicit "id" means "show the raw id" even when a text column exists.
        let t = table(Some("id"), vec![col("name", FieldType::Text)]);
        assert_eq!(schema.effective_display_column(&t), "id");
    }
}
