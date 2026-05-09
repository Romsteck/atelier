use crate::schema::{
    ColumnDefinition, DatabaseSchema, FieldType, RelationDefinition, TableDefinition,
};

/// Postgres reserved words (the subset that are actual *parser* keywords).
/// Any of these as an unquoted identifier would fail to parse — but since
/// our DDL emission always quotes user identifiers via [`crate::migration::quote_ident`],
/// the practical risk is low. We still reject them upfront to keep the
/// generated SQL portable and to surface confusing names early.
///
/// Notable exclusions:
/// - **type names** (`integer`, `text`, `timestamp`, `date`, `time`,
///   `interval`, `jsonb`, `uuid`, etc.) are NOT in this list. They are
///   only meaningful in type positions, never as bare identifiers in
///   parser contexts; quoting them makes them safe column names. Apps
///   like trader have legitimate columns named `date` and `timestamp`.
/// - the implicit Dataverse columns (`id`, `created_at`, `updated_at`)
///   and the `_dv_*` system tables are handled by [`validate_user_identifier`]
///   below — they're forbidden as user-defined names there.
const RESERVED_WORDS: &[&str] = &[
    "all", "analyse", "analyze", "and", "any", "array", "as", "asc", "asymmetric",
    "both", "case", "cast", "check", "collate", "column", "constraint", "create",
    "current_catalog", "current_date", "current_role", "current_time", "current_timestamp",
    "current_user", "default", "deferrable", "desc", "distinct", "do", "else", "end",
    "except", "false", "fetch", "for", "foreign", "from", "grant", "group", "having",
    "in", "initially", "intersect", "into", "lateral", "leading", "limit", "localtime",
    "localtimestamp", "not", "null", "offset", "on", "only", "or", "order", "placing",
    "primary", "references", "returning", "select", "session_user", "some", "symmetric",
    "table", "then", "to", "trailing", "true", "union", "unique", "user", "using",
    "variadic", "when", "where", "window", "with",
];

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid name '{0}': must be 1-63 chars, alphanumeric or underscore, start with letter or underscore")]
    InvalidName(String),
    #[error("reserved or implicit name: '{0}'")]
    ReservedWord(String),
    #[error("name '{0}' starts with reserved prefix '_dv_'")]
    ReservedPrefix(String),
    #[error("duplicate column name: '{0}'")]
    DuplicateColumn(String),
    #[error("table '{0}' already exists")]
    TableExists(String),
    #[error("table '{0}' not found")]
    TableNotFound(String),
    #[error("column '{0}' not found in table '{1}'")]
    ColumnNotFound(String, String),
    #[error("Choice/MultiChoice field '{0}' must have at least one choice")]
    EmptyChoices(String),
    #[error("Lookup field '{0}' requires lookup_target")]
    MissingLookupTarget(String),
    #[error("Lookup field '{0}' references non-existent table '{1}'")]
    LookupTargetNotFound(String, String),
    #[error("Formula field '{0}' requires a formula_expression")]
    MissingFormulaExpression(String),
    #[error("field '{0}' has formula_expression but is not a Formula type")]
    UnexpectedFormulaExpression(String),
    #[error("relation references non-existent table '{0}'")]
    RelationTableNotFound(String),
    #[error("relation references non-existent column '{0}' in table '{1}'")]
    RelationColumnNotFound(String, String),
}

/// Validate that `name` is a safe Postgres identifier we generate without quoting.
pub fn validate_identifier(name: &str) -> Result<(), ValidationError> {
    // Postgres identifier max length is 63 bytes (NAMEDATALEN-1).
    if name.is_empty() || name.len() > 63 {
        return Err(ValidationError::InvalidName(name.to_string()));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return Err(ValidationError::InvalidName(name.to_string()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ValidationError::InvalidName(name.to_string()));
    }
    let lower = name.to_lowercase();
    if RESERVED_WORDS.contains(&lower.as_str()) {
        return Err(ValidationError::ReservedWord(name.to_string()));
    }
    Ok(())
}

/// Validate a name that *must not* collide with the `_dv_` system
/// prefix or the implicit base-model columns. Used for user-defined
/// tables and columns; system metadata tables bypass this.
pub fn validate_user_identifier(name: &str) -> Result<(), ValidationError> {
    validate_identifier(name)?;
    let lower = name.to_lowercase();
    if lower.starts_with("_dv_") {
        return Err(ValidationError::ReservedPrefix(name.to_string()));
    }
    // Base-model columns are auto-added to every user table; redeclaring
    // them would conflict at CREATE TABLE time and confuses the gateway's
    // `@meta`/visibility split.
    if crate::migration::is_base_column(&lower) {
        return Err(ValidationError::ReservedWord(name.to_string()));
    }
    Ok(())
}

pub fn validate_column(col: &ColumnDefinition) -> Result<(), ValidationError> {
    validate_user_identifier(&col.name)?;

    if matches!(col.field_type, FieldType::Choice | FieldType::MultiChoice)
        && col.choices.is_empty()
    {
        return Err(ValidationError::EmptyChoices(col.name.clone()));
    }
    if col.field_type == FieldType::Lookup && col.lookup_target.is_none() {
        return Err(ValidationError::MissingLookupTarget(col.name.clone()));
    }
    if col.field_type == FieldType::Formula {
        let expr_ok = col
            .formula_expression
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if !expr_ok {
            return Err(ValidationError::MissingFormulaExpression(col.name.clone()));
        }
    } else if col.formula_expression.is_some() {
        return Err(ValidationError::UnexpectedFormulaExpression(col.name.clone()));
    }
    Ok(())
}

pub fn validate_table_definition(
    table: &TableDefinition,
    schema: &DatabaseSchema,
) -> Result<(), ValidationError> {
    validate_user_identifier(&table.name)?;
    validate_user_identifier(&table.slug)?;

    if schema
        .tables
        .iter()
        .any(|t| t.name == table.name || t.slug == table.slug)
    {
        return Err(ValidationError::TableExists(table.name.clone()));
    }

    let mut seen = std::collections::HashSet::new();
    for col in &table.columns {
        if !seen.insert(col.name.clone()) {
            return Err(ValidationError::DuplicateColumn(col.name.clone()));
        }
        validate_column(col)?;

        // Lookup target must exist in the schema (unless it's the table being created itself for self-FK)
        if col.field_type == FieldType::Lookup {
            if let Some(target) = &col.lookup_target {
                let exists = target == &table.name
                    || schema.tables.iter().any(|t| &t.name == target);
                if !exists {
                    return Err(ValidationError::LookupTargetNotFound(
                        col.name.clone(),
                        target.clone(),
                    ));
                }
            }
        }
    }

    Ok(())
}

pub fn validate_relation(
    rel: &RelationDefinition,
    schema: &DatabaseSchema,
) -> Result<(), ValidationError> {
    let from_table = schema
        .tables
        .iter()
        .find(|t| t.name == rel.from_table)
        .ok_or_else(|| ValidationError::RelationTableNotFound(rel.from_table.clone()))?;
    let to_table = schema
        .tables
        .iter()
        .find(|t| t.name == rel.to_table)
        .ok_or_else(|| ValidationError::RelationTableNotFound(rel.to_table.clone()))?;

    // `id` is the implicit primary key, always valid.
    let from_ok = rel.from_column == "id"
        || from_table.columns.iter().any(|c| c.name == rel.from_column);
    if !from_ok {
        return Err(ValidationError::RelationColumnNotFound(
            rel.from_column.clone(),
            rel.from_table.clone(),
        ));
    }
    let to_ok = rel.to_column == "id"
        || to_table.columns.iter().any(|c| c.name == rel.to_column);
    if !to_ok {
        return Err(ValidationError::RelationColumnNotFound(
            rel.to_column.clone(),
            rel.to_table.clone(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_words() {
        assert!(validate_identifier("user").is_err());
        assert!(validate_identifier("table").is_err());
        assert!(validate_identifier("select").is_err());
        // Type names are allowed at the bare-identifier level — they're
        // safe column names because we always quote in DDL emission.
        assert!(validate_identifier("date").is_ok());
        assert!(validate_identifier("timestamp").is_ok());
        assert!(validate_identifier("integer").is_ok());
        // Implicit dataverse columns are blocked at the user-identifier
        // level, not at the bare-identifier level.
        assert!(validate_identifier("created_at").is_ok());
        assert!(validate_user_identifier("created_at").is_err());
        assert!(validate_user_identifier("id").is_err());
        assert!(validate_user_identifier("updated_at").is_err());
        // Base-model columns are also reserved.
        assert!(validate_user_identifier("created_by").is_err());
        assert!(validate_user_identifier("updated_by").is_err());
        assert!(validate_user_identifier("version").is_err());
        assert!(validate_user_identifier("is_deleted").is_err());
        // But `date` / `timestamp` ARE allowed as user columns
        // (they were the real-world blocker for trader's migration).
        assert!(validate_user_identifier("date").is_ok());
        assert!(validate_user_identifier("timestamp").is_ok());
    }

    #[test]
    fn rejects_dv_prefix_for_user_identifiers() {
        assert!(validate_user_identifier("_dv_foo").is_err());
        assert!(validate_user_identifier("contacts").is_ok());
    }

    #[test]
    fn rejects_invalid_chars() {
        assert!(validate_identifier("with space").is_err());
        assert!(validate_identifier("123abc").is_err());
        assert!(validate_identifier("good_name").is_ok());
        assert!(validate_identifier("_underscore").is_ok());
    }
}
