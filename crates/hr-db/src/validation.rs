use crate::schema::{
    ColumnDefinition, DatabaseSchema, FieldType, RelationDefinition, TableDefinition,
};

/// SQL reserved words that cannot be used as identifiers.
const RESERVED_WORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "insert",
    "update",
    "delete",
    "create",
    "drop",
    "alter",
    "table",
    "column",
    "index",
    "primary",
    "key",
    "foreign",
    "references",
    "null",
    "not",
    "and",
    "or",
    "in",
    "like",
    "between",
    "join",
    "on",
    "group",
    "order",
    "by",
    "having",
    "limit",
    "offset",
    "union",
    "all",
    "distinct",
    "as",
    "is",
    "exists",
    "case",
    "when",
    "then",
    "else",
    "end",
    "values",
    "set",
    "into",
    "default",
    "constraint",
    "unique",
    "check",
    "integer",
    "text",
    "real",
    "blob",
    "boolean",
];

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error(
        "Invalid name '{0}': must be 1-64 chars, alphanumeric or underscore, start with letter"
    )]
    InvalidName(String),
    #[error("Reserved SQL keyword: '{0}'")]
    ReservedWord(String),
    #[error("Duplicate column name: '{0}'")]
    DuplicateColumn(String),
    #[error("Table '{0}' already exists")]
    TableExists(String),
    #[error("Table '{0}' not found")]
    TableNotFound(String),
    #[error("Column '{0}' not found in table '{1}'")]
    ColumnNotFound(String, String),
    #[error("Choice field '{0}' must have at least one choice")]
    EmptyChoices(String),
    #[error("Relation references non-existent table '{0}'")]
    RelationTableNotFound(String),
    #[error("Relation references non-existent column '{0}' in table '{1}'")]
    RelationColumnNotFound(String, String),
    #[error("Formula field '{0}' requires a formula_expression")]
    MissingFormulaExpression(String),
    #[error("Field '{0}' has formula_expression but is not a Formula type")]
    UnexpectedFormulaExpression(String),
}

pub fn validate_identifier(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() || name.len() > 64 {
        return Err(ValidationError::InvalidName(name.to_string()));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return Err(ValidationError::InvalidName(name.to_string()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ValidationError::InvalidName(name.to_string()));
    }
    if RESERVED_WORDS.contains(&name.to_lowercase().as_str()) {
        return Err(ValidationError::ReservedWord(name.to_string()));
    }
    Ok(())
}

pub fn validate_table_definition(
    table: &TableDefinition,
    schema: &DatabaseSchema,
) -> Result<(), ValidationError> {
    validate_identifier(&table.name)?;
    validate_identifier(&table.slug)?;

    // Check no duplicate table
    if schema
        .tables
        .iter()
        .any(|t| t.name == table.name || t.slug == table.slug)
    {
        return Err(ValidationError::TableExists(table.name.clone()));
    }

    // Check columns
    let mut seen = std::collections::HashSet::new();
    for col in &table.columns {
        validate_identifier(&col.name)?;
        if !seen.insert(&col.name) {
            return Err(ValidationError::DuplicateColumn(col.name.clone()));
        }
        validate_column(col)?;
    }

    Ok(())
}

pub fn validate_column(col: &ColumnDefinition) -> Result<(), ValidationError> {
    validate_identifier(&col.name)?;
    if matches!(col.field_type, FieldType::Choice | FieldType::MultiChoice)
        && col.choices.is_empty()
    {
        return Err(ValidationError::EmptyChoices(col.name.clone()));
    }
    if col.field_type == FieldType::Formula {
        match &col.formula_expression {
            None => return Err(ValidationError::MissingFormulaExpression(col.name.clone())),
            Some(expr) if expr.trim().is_empty() => {
                return Err(ValidationError::MissingFormulaExpression(col.name.clone()))
            }
            _ => {}
        }
    } else if col.formula_expression.is_some() {
        return Err(ValidationError::UnexpectedFormulaExpression(col.name.clone()));
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

    // `id` is the implicit primary key for every Dataverse-managed table, so it is
    // always a valid target even though it is not stored in `_dv_columns`.
    let from_ok =
        rel.from_column == "id" || from_table.columns.iter().any(|c| c.name == rel.from_column);
    if !from_ok {
        return Err(ValidationError::RelationColumnNotFound(
            rel.from_column.clone(),
            rel.from_table.clone(),
        ));
    }
    let to_ok = rel.to_column == "id" || to_table.columns.iter().any(|c| c.name == rel.to_column);
    if !to_ok {
        return Err(ValidationError::RelationColumnNotFound(
            rel.to_column.clone(),
            rel.to_table.clone(),
        ));
    }

    Ok(())
}
