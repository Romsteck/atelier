use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::EngineError;
use crate::validation::validate_identifier;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    Like,
    In,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub column: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    #[serde(default = "default_limit")]
    pub limit: u64,
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub order_by: Option<String>,
    #[serde(default)]
    pub order_desc: bool,
}

fn default_limit() -> u64 {
    100
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            limit: 100,
            offset: 0,
            order_by: None,
            order_desc: false,
        }
    }
}

/// Execute a SELECT query with filters and pagination.
pub fn query_rows(
    conn: &Connection,
    table: &str,
    filters: &[Filter],
    pagination: &Pagination,
) -> Result<Vec<Value>, EngineError> {
    validate_identifier(table).map_err(EngineError::Validation)?;

    let mut sql = format!("SELECT * FROM \"{}\"", table);
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if !filters.is_empty() {
        let mut conditions = Vec::new();
        for f in filters {
            validate_identifier(&f.column).map_err(EngineError::Validation)?;
            let (cond, vals) = build_filter_clause(&f.column, &f.op, &f.value);
            conditions.push(cond);
            param_values.extend(vals);
        }
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }

    if let Some(ref order_col) = pagination.order_by {
        validate_identifier(order_col).map_err(EngineError::Validation)?;
        sql.push_str(&format!(
            " ORDER BY \"{}\" {}",
            order_col,
            if pagination.order_desc { "DESC" } else { "ASC" }
        ));
    }

    sql.push_str(&format!(
        " LIMIT {} OFFSET {}",
        pagination.limit.min(1000),
        pagination.offset
    ));

    let mut stmt = conn.prepare(&sql)?;
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(params_from_iter(param_refs.iter()), |row| {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row_value_to_json(row, i);
            obj.insert(name.clone(), val);
        }
        Ok(Value::Object(obj))
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Insert rows into a table. Returns the number of rows inserted.
pub fn insert_rows(conn: &Connection, table: &str, rows: &[Value]) -> Result<usize, EngineError> {
    validate_identifier(table).map_err(EngineError::Validation)?;
    if rows.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for row in rows {
        let obj = row
            .as_object()
            .ok_or_else(|| EngineError::Other("Row must be a JSON object".to_string()))?;

        let cols: Vec<&String> = obj.keys().collect();
        for c in &cols {
            validate_identifier(c).map_err(EngineError::Validation)?;
        }

        let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{}", i)).collect();
        let col_names: Vec<String> = cols.iter().map(|c| format!("\"{}\"", c)).collect();

        let sql = format!(
            "INSERT INTO \"{}\" ({}) VALUES ({})",
            table,
            col_names.join(", "),
            placeholders.join(", ")
        );

        let values: Vec<Box<dyn rusqlite::types::ToSql>> =
            cols.iter().map(|c| json_to_sql_value(&obj[*c])).collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|p| p.as_ref()).collect();

        conn.execute(&sql, params_from_iter(param_refs.iter()))?;
        count += 1;
    }
    Ok(count)
}

/// Delete rows matching filters. Returns the number of rows deleted.
pub fn delete_rows(
    conn: &Connection,
    table: &str,
    filters: &[Filter],
) -> Result<usize, EngineError> {
    validate_identifier(table).map_err(EngineError::Validation)?;
    if filters.is_empty() {
        return Err(EngineError::Other(
            "Delete requires at least one filter (use drop_table for full delete)".to_string(),
        ));
    }

    let mut sql = format!("DELETE FROM \"{}\"", table);
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut conditions = Vec::new();
    for f in filters {
        validate_identifier(&f.column).map_err(EngineError::Validation)?;
        let (cond, vals) = build_filter_clause(&f.column, &f.op, &f.value);
        conditions.push(cond);
        param_values.extend(vals);
    }
    sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let affected = conn.execute(&sql, params_from_iter(param_refs.iter()))?;
    Ok(affected)
}

/// Update rows matching filters. Returns the number of rows updated.
pub fn update_rows(
    conn: &Connection,
    table: &str,
    updates: &Value,
    filters: &[Filter],
) -> Result<usize, EngineError> {
    validate_identifier(table).map_err(EngineError::Validation)?;
    if filters.is_empty() {
        return Err(EngineError::Other(
            "Update requires at least one filter".to_string(),
        ));
    }

    let obj = updates
        .as_object()
        .ok_or_else(|| EngineError::Other("Updates must be a JSON object".to_string()))?;
    if obj.is_empty() {
        return Ok(0);
    }

    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut set_clauses = Vec::new();

    for (col, val) in obj {
        validate_identifier(col).map_err(EngineError::Validation)?;
        set_clauses.push(format!("\"{}\" = ?", col));
        param_values.push(json_to_sql_value(val));
    }

    // Add "updated_at" automatically
    set_clauses.push("\"updated_at\" = datetime('now')".to_string());

    let mut conditions = Vec::new();
    for f in filters {
        validate_identifier(&f.column).map_err(EngineError::Validation)?;
        let (cond, vals) = build_filter_clause(&f.column, &f.op, &f.value);
        conditions.push(cond);
        param_values.extend(vals);
    }

    let sql = format!(
        "UPDATE \"{}\" SET {} WHERE {}",
        table,
        set_clauses.join(", "),
        conditions.join(" AND ")
    );

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let affected = conn.execute(&sql, params_from_iter(param_refs.iter()))?;
    Ok(affected)
}

/// Execute a SELECT with optional LEFT JOIN expansion on Lookup columns.
/// `expand` is a list of `(from_col, to_table, to_col, display_col)`.
pub fn query_rows_expanded(
    conn: &Connection,
    table: &str,
    filters: &[Filter],
    pagination: &Pagination,
    expand: &[(&str, &str, &str, &str)],
) -> Result<Vec<Value>, EngineError> {
    validate_identifier(table).map_err(EngineError::Validation)?;

    if expand.is_empty() {
        return query_rows(conn, table, filters, pagination);
    }

    // Validate all expand identifiers
    for (from_col, to_table, to_col, display_col) in expand {
        validate_identifier(from_col).map_err(EngineError::Validation)?;
        validate_identifier(to_table).map_err(EngineError::Validation)?;
        validate_identifier(to_col).map_err(EngineError::Validation)?;
        validate_identifier(display_col).map_err(EngineError::Validation)?;
    }

    // Build SELECT: base table.* + expanded display columns
    let mut sql = format!("SELECT \"{}\".*", table);
    for (from_col, _to_table, _to_col, display_col) in expand {
        let alias = format!("_exp_{}", from_col);
        sql.push_str(&format!(
            ", \"{}\".\"{}\" AS \"{}_display\"",
            alias, display_col, from_col
        ));
    }

    sql.push_str(&format!(" FROM \"{}\"", table));

    // LEFT JOINs
    for (from_col, to_table, to_col, _display_col) in expand {
        let alias = format!("_exp_{}", from_col);
        sql.push_str(&format!(
            " LEFT JOIN \"{}\" AS \"{}\" ON \"{}\".\"{}\" = \"{}\".\"{}\"",
            to_table, alias, table, from_col, alias, to_col
        ));
    }

    // WHERE
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !filters.is_empty() {
        let mut conditions = Vec::new();
        for f in filters {
            validate_identifier(&f.column).map_err(EngineError::Validation)?;
            let qualified_col = format!("\"{}\".\"{}\"", table, f.column);
            let (cond, vals) = build_filter_clause_raw(&qualified_col, &f.op, &f.value);
            conditions.push(cond);
            param_values.extend(vals);
        }
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }

    // ORDER BY
    if let Some(ref order_col) = pagination.order_by {
        validate_identifier(order_col).map_err(EngineError::Validation)?;
        sql.push_str(&format!(
            " ORDER BY \"{}\".\"{}\" {}",
            table,
            order_col,
            if pagination.order_desc { "DESC" } else { "ASC" }
        ));
    }

    sql.push_str(&format!(
        " LIMIT {} OFFSET {}",
        pagination.limit.min(1000),
        pagination.offset
    ));

    let mut stmt = conn.prepare(&sql)?;
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(params_from_iter(param_refs.iter()), |row| {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row_value_to_json(row, i);
            obj.insert(name.clone(), val);
        }
        Ok(Value::Object(obj))
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Same as build_filter_clause but takes a pre-formatted column expression (for qualified names).
fn build_filter_clause_raw(
    col_expr: &str,
    op: &FilterOp,
    value: &Option<Value>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    match op {
        FilterOp::IsNull => (format!("{} IS NULL", col_expr), vec![]),
        FilterOp::IsNotNull => (format!("{} IS NOT NULL", col_expr), vec![]),
        _ => {
            let sql_op = match op {
                FilterOp::Eq => "=",
                FilterOp::Ne => "!=",
                FilterOp::Gt => ">",
                FilterOp::Lt => "<",
                FilterOp::Gte => ">=",
                FilterOp::Lte => "<=",
                FilterOp::Like => "LIKE",
                FilterOp::In => "IN",
                _ => unreachable!(),
            };
            if matches!(op, FilterOp::In) {
                if let Some(Value::Array(arr)) = value {
                    let placeholders: Vec<&str> = arr.iter().map(|_| "?").collect();
                    let vals: Vec<Box<dyn rusqlite::types::ToSql>> =
                        arr.iter().map(|v| json_to_sql_value(v)).collect();
                    (format!("{} IN ({})", col_expr, placeholders.join(",")), vals)
                } else {
                    (
                        format!("{} IN (?)", col_expr),
                        vec![json_to_sql_value(value.as_ref().unwrap_or(&Value::Null))],
                    )
                }
            } else {
                let val = json_to_sql_value(value.as_ref().unwrap_or(&Value::Null));
                (format!("{} {} ?", col_expr, sql_op), vec![val])
            }
        }
    }
}

fn build_filter_clause(
    col: &str,
    op: &FilterOp,
    value: &Option<Value>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    match op {
        FilterOp::IsNull => (format!("\"{}\" IS NULL", col), vec![]),
        FilterOp::IsNotNull => (format!("\"{}\" IS NOT NULL", col), vec![]),
        _ => {
            let sql_op = match op {
                FilterOp::Eq => "=",
                FilterOp::Ne => "!=",
                FilterOp::Gt => ">",
                FilterOp::Lt => "<",
                FilterOp::Gte => ">=",
                FilterOp::Lte => "<=",
                FilterOp::Like => "LIKE",
                FilterOp::In => "IN",
                _ => unreachable!(),
            };
            if matches!(op, FilterOp::In) {
                // IN clause
                if let Some(Value::Array(arr)) = value {
                    let placeholders: Vec<&str> = arr.iter().map(|_| "?").collect();
                    let vals: Vec<Box<dyn rusqlite::types::ToSql>> =
                        arr.iter().map(|v| json_to_sql_value(v)).collect();
                    (format!("\"{}\" IN ({})", col, placeholders.join(",")), vals)
                } else {
                    (
                        format!("\"{}\" IN (?)", col),
                        vec![json_to_sql_value(value.as_ref().unwrap_or(&Value::Null))],
                    )
                }
            } else {
                let val = json_to_sql_value(value.as_ref().unwrap_or(&Value::Null));
                (format!("\"{}\" {} ?", col, sql_op), vec![val])
            }
        }
    }
}

fn json_to_sql_value(val: &Value) -> Box<dyn rusqlite::types::ToSql> {
    match val {
        Value::Null => Box::new(Option::<String>::None),
        Value::Bool(b) => Box::new(*b as i32),
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
        _ => Box::new(val.to_string()),
    }
}

fn row_value_to_json(row: &rusqlite::Row<'_>, idx: usize) -> Value {
    // Try different types
    if let Ok(v) = row.get::<_, Option<i64>>(idx) {
        return v.map(Value::from).unwrap_or(Value::Null);
    }
    if let Ok(v) = row.get::<_, Option<f64>>(idx) {
        return v
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .unwrap_or(Value::Null);
    }
    if let Ok(v) = row.get::<_, Option<String>>(idx) {
        return v.map(Value::String).unwrap_or(Value::Null);
    }
    Value::Null
}
