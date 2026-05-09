//! Query construction for the dataverse REST gateway.
//!
//! Translates a [`ListQuery`] (the parsed form of `GET /api/dv/{slug}/{table}?
//! $filter=…&$select=…&$orderby=…&$top=…&$skip=…&$includeDeleted=…`) into a
//! parameterised SQL `SELECT` statement.
//!
//! Two cross-cutting invariants enforced here:
//!
//! 1. **Soft-delete by default** — every WHERE auto-`AND`s `is_deleted =
//!    FALSE` unless `$includeDeleted=true`.
//! 2. **No SQL injection surface** — `$filter` is parsed by [`hr_dvexpr`];
//!    user values land as bind parameters, never as inlined SQL fragments.
//!
//! Lookup expansion (`$expand`) is handled in [`crate::expand`] (separate
//! module, separate query/round-trip) and not here.

use std::collections::HashMap;

use hr_common::Identity;
use hr_dvexpr::{
    checker::{Checker, ColumnSchema as DvExprSchema},
    sql::{ContextValues, Mode, Param as DvParam, SqlEmitter},
    Type,
};
use serde::{Deserialize, Serialize};

use crate::error::{DataverseError, Result};
use crate::migration::{is_base_column, quote_ident};
use crate::schema::{FieldType, IdStrategy, TableDefinition};

/// Parsed list-query parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListQuery {
    /// Source for the `$filter=…` query string. Parsed via [`hr_dvexpr`];
    /// the same expression language used for computed columns.
    pub filter: Option<String>,
    /// `$select`; when empty, all non-private columns are returned.
    pub select: Vec<String>,
    /// `$orderby` items, in the order specified.
    pub orderby: Vec<OrderBy>,
    /// `$top`; capped at [`MAX_TOP`].
    pub top: Option<u32>,
    /// `$skip`.
    pub skip: Option<u32>,
    /// `$includeDeleted`.
    pub include_deleted: bool,
    /// `$count` — when true, the gateway issues a parallel `COUNT(*)` to
    /// produce the `@count` envelope field.
    pub count: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBy {
    pub column: String,
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

impl Direction {
    pub fn as_sql(self) -> &'static str {
        match self {
            Direction::Asc => "ASC",
            Direction::Desc => "DESC",
        }
    }
}

pub const DEFAULT_TOP: u32 = 50;
pub const MAX_TOP: u32 = 1000;

/// A parameter value bound into the produced SQL. Mirrors
/// [`hr_dvexpr::sql::Param`] but stays inside this crate because the IPC
/// layer needs to serialise these.
#[derive(Debug, Clone)]
pub enum QueryParam {
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Null,
    Timestamp(String),
    Date(String),
    Uuid(String),
}

impl From<DvParam> for QueryParam {
    fn from(p: DvParam) -> Self {
        match p {
            DvParam::Int(v) => QueryParam::Int(v),
            DvParam::Float(v) => QueryParam::Float(v),
            DvParam::Text(s) => QueryParam::Text(s),
            DvParam::Bool(b) => QueryParam::Bool(b),
            DvParam::Null => QueryParam::Null,
            DvParam::Timestamp(s) => QueryParam::Timestamp(s),
            DvParam::Date(s) => QueryParam::Date(s),
            DvParam::Uuid(s) => QueryParam::Uuid(s),
        }
    }
}

/// Compiled list query: SQL string + bind parameters in `$1..$N` order.
#[derive(Debug, Clone)]
pub struct CompiledListQuery {
    pub sql: String,
    pub params: Vec<QueryParam>,
    /// Names of the columns selected, in the same order as the SELECT
    /// clause. Used by row decoding.
    pub columns: Vec<String>,
}

/// Build the SELECT for a list query. `_identity` is reserved for future
/// row-level rights; v1 ignores it.
pub fn build_list_sql(
    table: &TableDefinition,
    query: &ListQuery,
    _identity: &Identity,
) -> Result<CompiledListQuery> {
    let columns = resolve_select(table, &query.select)?;
    let select_sql = columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");

    let schema = build_dvexpr_schema(table);
    let ctx = ContextValues::default();
    let mut emitter = SqlEmitter {
        mode: Mode::Parameterized,
        ctx: &ctx,
        this_prefix: None,
        params: Vec::new(),
    };

    // Soft-delete predicate is always part of base predicates; user filter
    // composes with it via AND.
    let mut where_clauses: Vec<String> = Vec::new();
    if !query.include_deleted {
        where_clauses.push("\"is_deleted\" = FALSE".into());
    }
    if let Some(src) = &query.filter {
        let toks = hr_dvexpr::lexer::tokenize(src)
            .map_err(|e| DataverseError::internal(format!("$filter parse: {}", e)))?;
        let ast = hr_dvexpr::parser::parse(&toks)
            .map_err(|e| DataverseError::internal(format!("$filter parse: {}", e)))?;
        let mut checker = Checker::new(&schema);
        let typed = checker
            .check(&ast)
            .map_err(|e| DataverseError::internal(format!("$filter type: {}", e)))?;
        if typed.ty() != Type::Bool && typed.ty() != Type::Null {
            return Err(DataverseError::internal(format!(
                "$filter must be Bool, got {}",
                typed.ty()
            )));
        }
        let where_sql = emitter
            .emit(&typed)
            .map_err(|e| DataverseError::internal(format!("$filter emit: {}", e)))?;
        where_clauses.push(where_sql);
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };

    // ORDER BY
    let order_sql = if query.orderby.is_empty() {
        " ORDER BY \"id\" ASC".to_string()
    } else {
        let parts: Vec<String> = query
            .orderby
            .iter()
            .map(|o| {
                if !column_known(table, &o.column) {
                    return Err(DataverseError::internal(format!(
                        "$orderby: unknown column '{}'",
                        o.column
                    )));
                }
                Ok(format!("{} {}", quote_ident(&o.column), o.direction.as_sql()))
            })
            .collect::<Result<_>>()?;
        format!(" ORDER BY {}", parts.join(", "))
    };

    // Pagination
    let top = query.top.unwrap_or(DEFAULT_TOP).min(MAX_TOP);
    let mut limit_sql = format!(" LIMIT {}", top);
    if let Some(skip) = query.skip {
        if skip > 0 {
            limit_sql.push_str(&format!(" OFFSET {}", skip));
        }
    }

    let sql = format!(
        "SELECT {} FROM {}{}{}{};",
        select_sql,
        quote_ident(&table.name),
        where_sql,
        order_sql,
        limit_sql
    );

    Ok(CompiledListQuery {
        sql,
        params: emitter.params.into_iter().map(QueryParam::from).collect(),
        columns,
    })
}

/// Build the parallel `COUNT(*)` SQL for `$count=true`. Reuses the same
/// WHERE as [`build_list_sql`] but drops ORDER BY / LIMIT / OFFSET.
pub fn build_count_sql(
    table: &TableDefinition,
    query: &ListQuery,
    identity: &Identity,
) -> Result<(String, Vec<QueryParam>)> {
    // Reuse build_list_sql but reformat. Cheaper: inline the WHERE logic.
    let mut count_query = query.clone();
    count_query.select = vec!["id".into()];
    count_query.orderby = vec![];
    count_query.top = Some(MAX_TOP);
    count_query.skip = None;
    let compiled = build_list_sql(table, &count_query, identity)?;
    // Strip "SELECT … FROM" replace with "SELECT COUNT(*) FROM" and drop
    // ORDER BY/LIMIT trailing fragments.
    let sql = format!(
        "SELECT COUNT(*) FROM {}{};",
        quote_ident(&table.name),
        extract_where(&compiled.sql).unwrap_or_default()
    );
    Ok((sql, compiled.params))
}

fn extract_where(full_sql: &str) -> Option<String> {
    let upper = full_sql.to_uppercase();
    let start = upper.find(" WHERE ")?;
    let end_candidates = [" ORDER BY ", " LIMIT ", " OFFSET "];
    let end = end_candidates
        .iter()
        .filter_map(|kw| upper[start..].find(kw).map(|p| start + p))
        .min()
        .unwrap_or_else(|| full_sql.trim_end_matches(';').len());
    Some(full_sql[start..end].to_string())
}

fn column_known(table: &TableDefinition, col: &str) -> bool {
    is_base_column(col) || table.columns.iter().any(|c| c.name == col)
}

fn resolve_select(table: &TableDefinition, requested: &[String]) -> Result<Vec<String>> {
    if requested.is_empty() {
        // Default: id, timestamps, version, is_deleted, then user columns.
        // `created_by` / `updated_by` are NOT included unless explicitly
        // selected (or via `@audit` shorthand handled at the route layer).
        let mut out = vec![
            "id".to_string(),
            "created_at".into(),
            "updated_at".into(),
            "version".into(),
            "is_deleted".into(),
        ];
        for c in &table.columns {
            out.push(c.name.clone());
        }
        return Ok(out);
    }
    for col in requested {
        if !column_known(table, col) {
            return Err(DataverseError::internal(format!(
                "$select: unknown column '{}'",
                col
            )));
        }
    }
    Ok(requested.to_vec())
}

/// Build a [`hr_dvexpr::checker::ColumnSchema`] over a `TableDefinition`,
/// covering both base and user columns under the `this.<col>` namespace.
fn build_dvexpr_schema(table: &TableDefinition) -> InMemorySchema {
    let mut map: HashMap<Vec<String>, Type> = HashMap::new();
    let id_type = match table.id_strategy {
        IdStrategy::Bigserial => Type::INT,
        IdStrategy::Uuid => Type::Uuid,
    };
    map.insert(vec!["this".into(), "id".into()], id_type);
    map.insert(
        vec!["this".into(), "created_at".into()],
        Type::Timestamp,
    );
    map.insert(
        vec!["this".into(), "updated_at".into()],
        Type::Timestamp,
    );
    map.insert(vec!["this".into(), "created_by".into()], Type::Uuid);
    map.insert(vec!["this".into(), "updated_by".into()], Type::Uuid);
    map.insert(vec!["this".into(), "created_by_kind".into()], Type::Text);
    map.insert(vec!["this".into(), "updated_by_kind".into()], Type::Text);
    map.insert(vec!["this".into(), "version".into()], Type::INT);
    map.insert(vec!["this".into(), "is_deleted".into()], Type::Bool);
    for col in &table.columns {
        if let Some(t) = field_to_dvexpr_type(col.field_type) {
            map.insert(vec!["this".into(), col.name.clone()], t);
        }
    }
    InMemorySchema { map }
}

struct InMemorySchema {
    map: HashMap<Vec<String>, Type>,
}

impl DvExprSchema for InMemorySchema {
    fn lookup(&self, path: &[String]) -> Option<Type> {
        self.map.get(path).copied()
    }
}

fn field_to_dvexpr_type(ft: FieldType) -> Option<Type> {
    Some(match ft {
        FieldType::Text
        | FieldType::Email
        | FieldType::Url
        | FieldType::Phone
        | FieldType::Choice
        | FieldType::Time
        | FieldType::Duration
        | FieldType::Formula => Type::Text,
        // NUMERIC-backed columns are floating-point on the JSON wire side
        // (`pg_type()` emits `NUMERIC(20,6)`; `dv_io::field_to_json` narrows
        // to f64). Mapping them to `Type::Text` previously made `$filter`
        // reject ordered comparisons like `amount > 0`.
        FieldType::Decimal | FieldType::Currency | FieldType::Percent => Type::FLOAT,
        FieldType::Number | FieldType::AutoIncrement | FieldType::Lookup => Type::INT,
        FieldType::Boolean => Type::Bool,
        FieldType::DateTime => Type::Timestamp,
        FieldType::Date => Type::Date,
        FieldType::Uuid => Type::Uuid,
        // JSON / multichoice are not yet representable as a single dvexpr type;
        // exclude from filtering — agents can use raw column equality only via
        // future explicit support.
        FieldType::Json | FieldType::MultiChoice => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDefinition, IdStrategy};
    use chrono::Utc;
    use uuid::Uuid;

    fn col(name: &str, t: FieldType) -> ColumnDefinition {
        ColumnDefinition {
            name: name.into(),
            field_type: t,
            required: false,
            unique: false,
            default_value: None,
            description: None,
            choices: vec![],
            formula_expression: None,
            lookup_target: None,
        }
    }

    fn table_orders() -> TableDefinition {
        TableDefinition {
            name: "orders".into(),
            slug: "orders".into(),
            columns: vec![
                col("qty", FieldType::Number),
                col("price", FieldType::Decimal),
                col("name", FieldType::Text),
                col("active", FieldType::Boolean),
                col("due_at", FieldType::DateTime),
            ],
            description: None,
            id_strategy: IdStrategy::Bigserial,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn id() -> Identity {
        Identity::user(Uuid::new_v4(), "tester")
    }

    #[test]
    fn list_default_selects_all_with_soft_delete_filter() {
        let q = ListQuery::default();
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert!(cq.sql.starts_with("SELECT "));
        assert!(cq.sql.contains("\"orders\""));
        assert!(cq.sql.contains("\"is_deleted\" = FALSE"));
        assert!(cq.sql.contains("ORDER BY \"id\" ASC"));
        assert!(cq.sql.contains(&format!("LIMIT {}", DEFAULT_TOP)));
        // Default includes id, timestamps, version, is_deleted, user cols.
        assert!(cq.columns.contains(&"id".to_string()));
        assert!(cq.columns.contains(&"qty".to_string()));
        assert!(!cq.columns.contains(&"created_by".to_string()));
    }

    #[test]
    fn include_deleted_drops_default_predicate() {
        let q = ListQuery {
            include_deleted: true,
            ..Default::default()
        };
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert!(!cq.sql.contains("\"is_deleted\" = FALSE"));
    }

    #[test]
    fn filter_compiles_to_parameterised_where() {
        let q = ListQuery {
            filter: Some("qty > 10 && active == true".into()),
            ..Default::default()
        };
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert!(cq.sql.contains("WHERE"));
        // Both base predicate and user filter present.
        assert!(cq.sql.contains("\"is_deleted\" = FALSE"));
        assert!(cq.sql.contains("\"qty\""));
        assert!(cq.sql.contains("$1"));
        assert_eq!(cq.params.len(), 2);
    }

    #[test]
    fn filter_with_text_value() {
        let q = ListQuery {
            filter: Some("name == 'foo'".into()),
            ..Default::default()
        };
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert!(matches!(cq.params[0], QueryParam::Text(ref s) if s == "foo"));
    }

    #[test]
    fn select_user_columns_only() {
        let q = ListQuery {
            select: vec!["id".into(), "qty".into()],
            ..Default::default()
        };
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert_eq!(cq.columns, vec!["id".to_string(), "qty".into()]);
        assert!(!cq.sql.contains("\"price\""));
    }

    #[test]
    fn unknown_column_in_select_errors() {
        let q = ListQuery {
            select: vec!["nope".into()],
            ..Default::default()
        };
        assert!(build_list_sql(&table_orders(), &q, &id()).is_err());
    }

    #[test]
    fn top_capped_at_max() {
        let q = ListQuery {
            top: Some(10_000),
            ..Default::default()
        };
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert!(cq.sql.contains(&format!("LIMIT {}", MAX_TOP)));
    }

    #[test]
    fn orderby_multi_with_directions() {
        let q = ListQuery {
            orderby: vec![
                OrderBy {
                    column: "qty".into(),
                    direction: Direction::Desc,
                },
                OrderBy {
                    column: "id".into(),
                    direction: Direction::Asc,
                },
            ],
            ..Default::default()
        };
        let cq = build_list_sql(&table_orders(), &q, &id()).unwrap();
        assert!(cq.sql.contains("ORDER BY \"qty\" DESC, \"id\" ASC"));
    }

    #[test]
    fn count_sql_skips_order_and_limit() {
        let q = ListQuery {
            filter: Some("qty > 10".into()),
            ..Default::default()
        };
        let (sql, params) = build_count_sql(&table_orders(), &q, &id()).unwrap();
        assert!(sql.contains("COUNT(*)"));
        assert!(sql.contains("\"is_deleted\" = FALSE"));
        assert!(!sql.contains("ORDER BY"));
        assert!(!sql.contains("LIMIT"));
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn filter_must_be_bool() {
        let q = ListQuery {
            filter: Some("qty + 1".into()), // Number, not Bool
            ..Default::default()
        };
        assert!(build_list_sql(&table_orders(), &q, &id()).is_err());
    }

    #[test]
    fn unknown_orderby_column_errors() {
        let q = ListQuery {
            orderby: vec![OrderBy {
                column: "nope".into(),
                direction: Direction::Asc,
            }],
            ..Default::default()
        };
        assert!(build_list_sql(&table_orders(), &q, &id()).is_err());
    }
}
