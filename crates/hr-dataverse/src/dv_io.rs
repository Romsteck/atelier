//! Postgres I/O glue for the dataverse REST gateway.
//!
//! Bridges the pure builders ([`crate::query`], [`crate::crud`],
//! [`crate::audit`]) to actual `sqlx` execution: converts [`QueryParam`]
//! into bound `PgArguments`, runs queries against a [`PgPool`] (or a
//! transaction), and decodes [`PgRow`]s into JSON honoring the
//! [`FieldType`] of each user column plus the implicit base columns.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Number, Value};
use sqlx_core::arguments::Arguments;
use sqlx_postgres::{PgArguments, PgRow};
use sqlx_core::row::Row as _;
use uuid::Uuid;

use crate::error::{DataverseError, Result};
use crate::query::{CompiledListQuery, QueryParam};
use crate::schema::{ColumnDefinition, FieldType, IdStrategy, TableDefinition};

/// Append a single [`QueryParam`] to a `PgArguments` accumulator.
pub fn bind_param(args: &mut PgArguments, p: &QueryParam) -> Result<()> {
    let r = match p {
        QueryParam::Int(v) => args.add(*v),
        QueryParam::Float(v) => args.add(*v),
        QueryParam::Text(s) => args.add(s.clone()),
        QueryParam::Bool(b) => args.add(*b),
        QueryParam::Null => args.add(Option::<i64>::None),
        QueryParam::Timestamp(s) => {
            let dt: DateTime<Utc> = s.parse().map_err(|e| {
                DataverseError::internal(format!("bad timestamp '{}': {}", s, e))
            })?;
            args.add(dt)
        }
        QueryParam::Date(s) => {
            let d: NaiveDate = s.parse().map_err(|e| {
                DataverseError::internal(format!("bad date '{}': {}", s, e))
            })?;
            args.add(d)
        }
        QueryParam::Uuid(s) => {
            let u: Uuid = s.parse().map_err(|e| {
                DataverseError::internal(format!("bad uuid '{}': {}", s, e))
            })?;
            args.add(u)
        }
    };
    r.map_err(|e| DataverseError::internal(format!("bind: {}", e)))
}

/// Build a `PgArguments` from a vector of [`QueryParam`]s.
pub fn bind_all(params: &[QueryParam]) -> Result<PgArguments> {
    let mut args = PgArguments::default();
    for p in params {
        bind_param(&mut args, p)?;
    }
    Ok(args)
}

/// Decode a row that includes the full base model + the user columns of
/// `table`, into a JSON object. Column names that the row does not carry
/// (because the SELECT didn't ask for them) are skipped silently.
pub fn decode_row(row: &PgRow, table: &TableDefinition) -> Result<Map<String, Value>> {
    let mut out = Map::new();

    // Base columns — present on every gateway-managed row.
    if has_column(row, "id") {
        match table.id_strategy {
            IdStrategy::Bigserial => {
                let id: i64 = row.try_get("id").map_err(map_err)?;
                out.insert("id".into(), Value::Number(Number::from(id)));
            }
            IdStrategy::Uuid => {
                let id: Uuid = row.try_get("id").map_err(map_err)?;
                out.insert("id".into(), Value::String(id.to_string()));
            }
        }
    }
    insert_optional_timestamp(row, &mut out, "created_at")?;
    insert_optional_timestamp(row, &mut out, "updated_at")?;
    insert_optional_uuid(row, &mut out, "created_by")?;
    insert_optional_uuid(row, &mut out, "updated_by")?;
    insert_optional_text(row, &mut out, "created_by_kind")?;
    insert_optional_text(row, &mut out, "updated_by_kind")?;
    if has_column(row, "version") {
        let v: i32 = row.try_get("version").map_err(map_err)?;
        out.insert("version".into(), Value::Number(Number::from(v as i64)));
    }
    if has_column(row, "is_deleted") {
        let v: bool = row.try_get("is_deleted").map_err(map_err)?;
        out.insert("is_deleted".into(), Value::Bool(v));
    }

    // User columns — read in declared order.
    for col in &table.columns {
        if has_column(row, &col.name) {
            out.insert(col.name.clone(), read_user_column(row, col)?);
        }
    }
    Ok(out)
}

fn read_user_column(row: &PgRow, col: &ColumnDefinition) -> Result<Value> {
    let name = col.name.as_str();
    Ok(match col.field_type {
        FieldType::Boolean => match row.try_get::<Option<bool>, _>(name).map_err(map_err)? {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        },
        FieldType::Number | FieldType::AutoIncrement | FieldType::Lookup => {
            match row.try_get::<Option<i64>, _>(name).map_err(map_err)? {
                Some(i) => Value::Number(Number::from(i)),
                None => Value::Null,
            }
        }
        FieldType::Decimal | FieldType::Currency | FieldType::Percent => {
            // PG `NUMERIC` decodes via the bigdecimal feature; we then
            // narrow to f64 for the JSON wire shape. Lossy on very large
            // / very precise values; acceptable for currency at typical
            // ranges, and consumers that need full precision can declare
            // the column as `Text`.
            match row
                .try_get::<Option<sqlx_core::types::BigDecimal>, _>(name)
                .map_err(map_err)?
            {
                Some(d) => {
                    use std::str::FromStr;
                    let s = d.to_string();
                    let f = f64::from_str(&s).unwrap_or(0.0);
                    Value::Number(Number::from_f64(f).unwrap_or_else(|| Number::from(0)))
                }
                None => Value::Null,
            }
        }
        FieldType::Text
        | FieldType::Email
        | FieldType::Url
        | FieldType::Phone
        | FieldType::Choice
        | FieldType::Formula => match row.try_get::<Option<String>, _>(name).map_err(map_err)? {
            Some(s) => Value::String(s),
            None => Value::Null,
        },
        FieldType::DateTime => match row.try_get::<Option<DateTime<Utc>>, _>(name).map_err(map_err)? {
            Some(dt) => Value::String(dt.to_rfc3339()),
            None => Value::Null,
        },
        FieldType::Date => match row.try_get::<Option<NaiveDate>, _>(name).map_err(map_err)? {
            Some(d) => Value::String(d.to_string()),
            None => Value::Null,
        },
        FieldType::Time => match row.try_get::<Option<chrono::NaiveTime>, _>(name).map_err(map_err)? {
            Some(t) => Value::String(t.to_string()),
            None => Value::Null,
        },
        FieldType::Duration => {
            // INTERVAL stringification — Postgres returns a textual form via cast.
            match row.try_get::<Option<String>, _>(name) {
                Ok(Some(s)) => Value::String(s),
                _ => Value::Null,
            }
        }
        FieldType::Json => {
            match row.try_get::<Option<sqlx_core::types::Json<Value>>, _>(name).map_err(map_err)? {
                Some(j) => j.0,
                None => Value::Null,
            }
        }
        FieldType::Uuid => match row.try_get::<Option<Uuid>, _>(name).map_err(map_err)? {
            Some(u) => Value::String(u.to_string()),
            None => Value::Null,
        },
        FieldType::MultiChoice => {
            match row.try_get::<Option<Vec<String>>, _>(name).map_err(map_err)? {
                Some(v) => Value::Array(v.into_iter().map(Value::String).collect()),
                None => Value::Null,
            }
        }
    })
}

fn has_column(row: &PgRow, name: &str) -> bool {
    row.try_column(name).is_ok()
}

fn insert_optional_timestamp(
    row: &PgRow,
    out: &mut Map<String, Value>,
    name: &str,
) -> Result<()> {
    if has_column(row, name) {
        if let Some(dt) = row
            .try_get::<Option<DateTime<Utc>>, _>(name)
            .map_err(map_err)?
        {
            out.insert(name.into(), Value::String(dt.to_rfc3339()));
        } else {
            out.insert(name.into(), Value::Null);
        }
    }
    Ok(())
}

fn insert_optional_uuid(row: &PgRow, out: &mut Map<String, Value>, name: &str) -> Result<()> {
    if has_column(row, name) {
        if let Some(u) = row.try_get::<Option<Uuid>, _>(name).map_err(map_err)? {
            out.insert(name.into(), Value::String(u.to_string()));
        } else {
            out.insert(name.into(), Value::Null);
        }
    }
    Ok(())
}

fn insert_optional_text(row: &PgRow, out: &mut Map<String, Value>, name: &str) -> Result<()> {
    if has_column(row, name) {
        if let Some(s) = row.try_get::<Option<String>, _>(name).map_err(map_err)? {
            out.insert(name.into(), Value::String(s));
        } else {
            out.insert(name.into(), Value::Null);
        }
    }
    Ok(())
}

fn map_err(e: sqlx_core::Error) -> DataverseError {
    DataverseError::internal(format!("decode row: {}", e))
}

/// Run a [`CompiledListQuery`] and return the rows as JSON objects.
pub async fn run_list(
    pool: &sqlx_postgres::PgPool,
    table: &TableDefinition,
    compiled: &CompiledListQuery,
) -> Result<Vec<Value>> {
    let args = bind_all(&compiled.params)?;
    let rows = sqlx_core::query::query_with(&compiled.sql, args)
        .fetch_all(pool)
        .await
        .map_err(|e| DataverseError::internal(format!("list fetch: {}", e)))?;
    rows.into_iter()
        .map(|r| decode_row(&r, table).map(Value::Object))
        .collect()
}

/// Run a single-row SELECT (typically [`crate::crud::build_get`]) and
/// return the row as JSON, or `None` if absent.
pub async fn run_get(
    pool: &sqlx_postgres::PgPool,
    table: &TableDefinition,
    sql: &str,
    params: &[QueryParam],
) -> Result<Option<Value>> {
    let args = bind_all(params)?;
    let row = sqlx_core::query::query_with(sql, args)
        .fetch_optional(pool)
        .await
        .map_err(|e| DataverseError::internal(format!("get fetch: {}", e)))?;
    Ok(row.map(|r| decode_row(&r, table).map(Value::Object)).transpose()?)
}

/// Run a parameterised `COUNT(*)` query.
pub async fn run_count(
    pool: &sqlx_postgres::PgPool,
    sql: &str,
    params: &[QueryParam],
) -> Result<i64> {
    let args = bind_all(params)?;
    let row = sqlx_core::query::query_with(sql, args)
        .fetch_one(pool)
        .await
        .map_err(|e| DataverseError::internal(format!("count fetch: {}", e)))?;
    let n: i64 = row.try_get(0).map_err(map_err)?;
    Ok(n)
}

/// Status of a CRUD mutation.
#[derive(Debug, Clone)]
pub enum MutationOutcome {
    /// The row was found and the operation succeeded. Carries the new
    /// row state (post-trigger, including the bumped version).
    Applied(Value),
    /// The row exists but its current `version` does not match the
    /// caller's `If-Match` value (or its `is_deleted` flag is in the
    /// wrong state). Caller maps this to HTTP 412.
    PreconditionFailed,
    /// No row at this id (or row is filtered out by soft-delete).
    NotFound,
}

/// Run a CRUD mutation (insert/update/soft-delete/restore) inside a
/// transaction, also append the supplied audit row, and commit. The
/// row JSON returned by the mutation's RETURNING clause is decoded
/// against `table`'s schema.
///
/// Returns:
/// - `MutationOutcome::Applied(row)` on success
/// - `MutationOutcome::NotFound` if the mutation affected 0 rows AND the
///   row does not exist at all
/// - `MutationOutcome::PreconditionFailed` if 0 rows but the row exists
///   (typically a wrong `If-Match` version, or wrong soft-delete state)
pub async fn run_mutation(
    pool: &sqlx_postgres::PgPool,
    table: &TableDefinition,
    mutation_sql: &str,
    mutation_params: &[QueryParam],
    audit_sql: Option<&str>,
    audit_params: &[QueryParam],
    row_id_for_404_probe: &Value,
) -> Result<MutationOutcome> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| DataverseError::internal(format!("begin tx: {}", e)))?;

    let args = bind_all(mutation_params)?;
    let row_opt = sqlx_core::query::query_with(mutation_sql, args)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| DataverseError::internal(format!("mutation: {}", e)))?;

    let row = match row_opt {
        Some(r) => r,
        None => {
            tx.rollback()
                .await
                .map_err(|e| DataverseError::internal(format!("rollback: {}", e)))?;
            return Ok(probe_outcome(pool, &table.name, row_id_for_404_probe).await);
        }
    };

    let json = decode_row(&row, table)?;

    if let Some(sql) = audit_sql {
        let args = bind_all(audit_params)?;
        sqlx_core::query::query_with(sql, args)
            .execute(&mut *tx)
            .await
            .map_err(|e| DataverseError::internal(format!("audit insert: {}", e)))?;
    }

    tx.commit()
        .await
        .map_err(|e| DataverseError::internal(format!("commit: {}", e)))?;

    Ok(MutationOutcome::Applied(Value::Object(json)))
}

async fn probe_outcome(
    pool: &sqlx_postgres::PgPool,
    table_name: &str,
    id: &Value,
) -> MutationOutcome {
    // Check whether the row exists at all (ignoring soft-delete and version).
    let id_param = match id {
        Value::Null => return MutationOutcome::NotFound,
        Value::Number(_) | Value::String(_) | Value::Bool(_) => id.clone(),
        _ => return MutationOutcome::NotFound,
    };
    let mut args = PgArguments::default();
    let bind_result = match &id_param {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                args.add(i)
            } else {
                return MutationOutcome::NotFound;
            }
        }
        Value::String(s) => {
            if let Ok(u) = s.parse::<Uuid>() {
                args.add(u)
            } else {
                args.add(s.clone())
            }
        }
        _ => return MutationOutcome::NotFound,
    };
    if bind_result.is_err() {
        return MutationOutcome::NotFound;
    }

    let sql = format!(
        "SELECT 1 FROM {} WHERE \"id\" = $1 LIMIT 1",
        crate::migration::quote_ident(table_name)
    );
    match sqlx_core::query::query_with(&sql, args)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(_)) => MutationOutcome::PreconditionFailed,
        _ => MutationOutcome::NotFound,
    }
}
