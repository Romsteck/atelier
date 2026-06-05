//! Pure SQL builders for row CRUD on user tables.
//!
//! Each function returns a `(sql, params)` pair ready to bind+execute on
//! a Postgres transaction. Execution + transactional grouping lives in
//! the orchestrator handler — keeping this module IO-free makes it
//! cheap to unit-test the SQL shape without a database.
//!
//! Invariants enforced here:
//! - Every mutation populates `created_by` / `updated_by` /
//!   `*_by_kind` from the calling [`Identity`].
//! - `UPDATE` and `DELETE` (soft-delete) must include an `If-Match`
//!   version. The builder emits `WHERE id=$ AND version=$ AND
//!   is_deleted=…`; the caller distinguishes `412 Precondition
//!   Failed` (row exists, wrong version) from `404 Not Found` by
//!   checking `rows_affected` and re-fetching the row.
//! - `DELETE` is **soft** (sets `is_deleted=TRUE`). Hard-delete is
//!   only available via the admin DDL endpoints.

use std::collections::BTreeMap;

use atelier_common::Identity;
use serde_json::Value;

use crate::error::{DataverseError, Result};
use crate::migration::{is_base_column, quote_ident, BASE_COLUMNS};
use crate::query::QueryParam;
use crate::schema::{FieldType, IdStrategy, TableDefinition};

/// Postgres cast suffix for the row-id column. The id column is created
/// as `uuid` for `IdStrategy::Uuid` tables and `bigserial` (`bigint`) for
/// `IdStrategy::Bigserial`. CRUD builders bind the id JSON value as
/// `text`, so we need an explicit cast on the WHERE side or PG raises
/// `operator does not exist: uuid = text`.
fn id_cast(table: &TableDefinition) -> &'static str {
    match table.id_strategy {
        IdStrategy::Uuid => "::uuid",
        IdStrategy::Bigserial => "::bigint",
    }
}

/// Look up the declared `FieldType` of a payload column. Falls back to
/// `Text` if the column isn't in the schema (validation upstream rejects
/// unknown columns, so this default only ever applies to defensive paths).
fn column_field_type(table: &TableDefinition, name: &str) -> FieldType {
    table
        .columns
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.field_type)
        .unwrap_or(FieldType::Text)
}

/// Bind a payload value with a `QueryParam` variant matching the column's
/// declared type, plus an optional Postgres cast suffix appended to the
/// SQL placeholder.
///
/// Why this exists: `json_to_query_param` maps on the *runtime* JSON
/// shape (always `Text` for strings, `Int`/`Float` for numbers). Postgres
/// then refuses implicit casts from `text` to `timestamptz`, `jsonb`,
/// `uuid` and `date`. This helper looks at the *schema* type and routes
/// strings to the typed `QueryParam` variants (already wired to bind as
/// `DateTime<Utc>` / `NaiveDate` / `Uuid` in `dv_io`). For `Json`, we
/// keep `Text` and add a `::jsonb` SQL cast since there is no dedicated
/// `QueryParam::Json` variant.
/// For a JSON-null payload value, return the SQL `NULL` literal to inline in
/// place of a bind parameter (typed cast for columns Postgres can't reach from
/// an untyped NULL). Returns `None` for non-null values.
///
/// Why we inline NULL for *every* column type (not just jsonb/timestamptz/…):
/// the bind layer encodes [`QueryParam::Null`] as `Option::<i64>::None`, i.e. a
/// `bigint` NULL. On a sometimes-null column (e.g. a nullable `text` symbol),
/// the first NULL bind poisons sqlx's *cached prepared statement* — its
/// parameter type for that position is fixed as `bigint` — so a later non-null
/// `text` bind at the same position is read by Postgres with the wrong binary
/// `recv` and fails with `08P01` (insufficient data) / `22P03` (incorrect
/// binary format). Inlining NULL keeps a sometimes-null position from ever
/// being a bound bigint-NULL, and makes the null vs non-null shapes compile to
/// distinct SQL (hence distinct cached statements). A bare untyped `NULL` is
/// unambiguous in `INSERT … VALUES` / `UPDATE SET col = …` because the target
/// column resolves its type.
fn typed_null_literal(value: &Value, field_type: FieldType) -> Option<&'static str> {
    if !matches!(value, Value::Null) {
        return None;
    }
    Some(match field_type {
        FieldType::Json => "NULL::jsonb",
        FieldType::DateTime => "NULL::timestamptz",
        FieldType::Date => "NULL::date",
        FieldType::Uuid => "NULL::uuid",
        _ => "NULL",
    })
}

/// Bind a non-null payload value with a `QueryParam` variant matching the
/// column's declared type. Emits an optional `::jsonb` cast for json
/// columns (since there's no dedicated `QueryParam::Json`); for
/// DateTime/Date/Uuid the typed `QueryParam` variant already tells sqlx
/// the right Postgres type, so no cast is needed.
fn param_for_column(value: &Value, field_type: FieldType) -> (QueryParam, Option<&'static str>) {
    debug_assert!(!matches!(value, Value::Null), "callers must handle null first");
    match (field_type, value) {
        (FieldType::DateTime, Value::String(s)) => (QueryParam::Timestamp(s.clone()), None),
        (FieldType::Date, Value::String(s)) => (QueryParam::Date(s.clone()), None),
        (FieldType::Uuid, Value::String(s)) => (QueryParam::Uuid(s.clone()), None),
        (FieldType::Json, _) => {
            // Serialise any JSON shape to text + cast at SQL side.
            let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
            (QueryParam::Text(text), Some("::jsonb"))
        }
        (FieldType::Money, _) => {
            // Money is stored as NUMERIC(20,6) but received on the wire as
            // either a JSON string (preferred, full precision) or a JSON
            // number (legacy / convenience). Bind as text + cast on the
            // SQL side so Postgres parses the decimal directly. Reject
            // anything that can't be turned into a string upfront — sqlx
            // would otherwise error mid-execute with a less actionable
            // message.
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => "0".to_string(), // payload validation should never let
                                      // arrays/objects through to here
            };
            (QueryParam::Text(text), Some("::numeric"))
        }
        _ => (json_to_query_param(value), None),
    }
}

/// Output of a CRUD builder.
#[derive(Debug, Clone)]
pub struct CompiledMutation {
    pub sql: String,
    pub params: Vec<QueryParam>,
    /// Columns selected in the `RETURNING` clause, in order.
    pub returning: Vec<String>,
}

/// Build an `INSERT INTO {table} (…) VALUES (…) RETURNING …`.
///
/// `payload` is the user-supplied column → value map. Base columns
/// (`id`, `created_by`, …) supplied here are rejected; they are filled
/// in by the builder from the [`Identity`].
pub fn build_insert(
    table: &TableDefinition,
    payload: &BTreeMap<String, Value>,
    identity: &Identity,
) -> Result<CompiledMutation> {
    reject_base_columns(payload)?;
    validate_payload_columns(table, payload)?;

    let actor_uuid = identity.actor_uuid();
    let kind = identity.kind_str();

    let mut col_names: Vec<&str> = Vec::new();
    let mut placeholders: Vec<String> = Vec::new();
    let mut params: Vec<QueryParam> = Vec::new();

    for (name, value) in payload {
        col_names.push(name.as_str());
        let ft = column_field_type(table, name);
        if let Some(literal) = typed_null_literal(value, ft) {
            // Inline NULL literal instead of binding a parameter. Avoids
            // sqlx defaulting `Option::<i64>::None` to `bigint`, which then
            // fails to cast to e.g. `jsonb` (no `bigint → jsonb` in PG).
            placeholders.push(literal.to_string());
        } else {
            let (param, cast) = param_for_column(value, ft);
            let n = params.len() + 1;
            placeholders.push(format!("${}{}", n, cast.unwrap_or("")));
            params.push(param);
        }
    }

    // Audit columns. created_by/updated_by are the same uuid + kind on
    // INSERT (no prior modifier). `None` actor_uuid (Identity::System) is
    // inlined as `NULL::uuid` — see `actor_uuid_sql` for why we don't bind
    // a Null QueryParam here.
    col_names.extend(["created_by", "updated_by", "created_by_kind", "updated_by_kind"]);
    placeholders.push(actor_uuid_sql(actor_uuid, &mut params));
    placeholders.push(actor_uuid_sql(actor_uuid, &mut params));
    placeholders.push(format!("${}", params.len() + 1));
    params.push(QueryParam::Text(kind.into()));
    placeholders.push(format!("${}", params.len() + 1));
    params.push(QueryParam::Text(kind.into()));

    let returning = full_returning_columns(table);
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING {};",
        quote_ident(&table.name),
        col_names
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders.join(", "),
        returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
    );

    check_param_count(&sql, params.len(), &table.name)?;
    Ok(CompiledMutation {
        sql,
        params,
        returning,
    })
}

/// Build an `UPDATE {table} SET … WHERE id=$ AND version=$ AND
/// is_deleted=FALSE RETURNING …`. Returns 0 rows when:
/// - the row does not exist (caller should reply 404)
/// - the row exists but `version` mismatches (caller should reply 412)
/// - the row is soft-deleted (caller should reply 404)
pub fn build_update(
    table: &TableDefinition,
    id: &Value,
    if_version: i32,
    payload: &BTreeMap<String, Value>,
    identity: &Identity,
) -> Result<CompiledMutation> {
    reject_base_columns(payload)?;
    validate_payload_columns(table, payload)?;
    if payload.is_empty() {
        return Err(DataverseError::internal("UPDATE payload is empty"));
    }

    let actor_uuid = identity.actor_uuid();
    let kind = identity.kind_str();

    let mut params: Vec<QueryParam> = Vec::new();
    let mut sets: Vec<String> = Vec::new();

    for (name, value) in payload {
        let ft = column_field_type(table, name);
        if let Some(literal) = typed_null_literal(value, ft) {
            sets.push(format!("{} = {}", quote_ident(name), literal));
        } else {
            let (param, cast) = param_for_column(value, ft);
            params.push(param);
            sets.push(format!(
                "{} = ${}{}",
                quote_ident(name),
                params.len(),
                cast.unwrap_or("")
            ));
        }
    }

    let updated_by_sql = actor_uuid_sql(actor_uuid, &mut params);
    sets.push(format!("\"updated_by\" = {updated_by_sql}"));
    params.push(QueryParam::Text(kind.into()));
    sets.push(format!("\"updated_by_kind\" = ${}", params.len()));

    // WHERE id=$idx AND version=$idx AND is_deleted=FALSE
    params.push(json_to_query_param(id));
    let id_idx = params.len();
    params.push(QueryParam::Int(if_version as i64));
    let ver_idx = params.len();

    let returning = full_returning_columns(table);
    let sql = format!(
        "UPDATE {tbl} SET {sets} WHERE {id_col} = ${id_idx}{cast} AND \"version\" = ${ver_idx} AND \"is_deleted\" = FALSE RETURNING {ret};",
        tbl = quote_ident(&table.name),
        sets = sets.join(", "),
        id_col = quote_ident("id"),
        cast = id_cast(table),
        id_idx = id_idx,
        ver_idx = ver_idx,
        ret = returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
    );

    check_param_count(&sql, params.len(), &table.name)?;
    Ok(CompiledMutation {
        sql,
        params,
        returning,
    })
}

/// Build a soft-delete: `UPDATE {table} SET is_deleted=TRUE,
/// updated_by=…, updated_by_kind=… WHERE id=$ AND version=$ AND
/// is_deleted=FALSE RETURNING …`. Same 0-row semantics as
/// [`build_update`].
pub fn build_soft_delete(
    table: &TableDefinition,
    id: &Value,
    if_version: i32,
    identity: &Identity,
) -> Result<CompiledMutation> {
    let actor_uuid = identity.actor_uuid();
    let kind = identity.kind_str();
    let mut params: Vec<QueryParam> = Vec::new();

    let by_sql = actor_uuid_sql(actor_uuid, &mut params);
    params.push(QueryParam::Text(kind.into()));
    let by_kind_idx = params.len();
    params.push(json_to_query_param(id));
    let id_idx = params.len();
    params.push(QueryParam::Int(if_version as i64));
    let ver_idx = params.len();

    let returning = full_returning_columns(table);
    let sql = format!(
        "UPDATE {tbl} SET \"is_deleted\" = TRUE, \"updated_by\" = {by_sql}, \"updated_by_kind\" = ${by_kind_idx} \
         WHERE \"id\" = ${id_idx}{cast} AND \"version\" = ${ver_idx} AND \"is_deleted\" = FALSE RETURNING {ret};",
        tbl = quote_ident(&table.name),
        cast = id_cast(table),
        by_sql = by_sql,
        by_kind_idx = by_kind_idx,
        id_idx = id_idx,
        ver_idx = ver_idx,
        ret = returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
    );
    check_param_count(&sql, params.len(), &table.name)?;
    Ok(CompiledMutation {
        sql,
        params,
        returning,
    })
}

/// Build a restore: undoes a soft-delete. The version match is required
/// (`If-Match` semantics) and the row must currently be deleted.
pub fn build_restore(
    table: &TableDefinition,
    id: &Value,
    if_version: i32,
    identity: &Identity,
) -> Result<CompiledMutation> {
    let actor_uuid = identity.actor_uuid();
    let kind = identity.kind_str();
    let mut params: Vec<QueryParam> = Vec::new();

    let by_sql = actor_uuid_sql(actor_uuid, &mut params);
    params.push(QueryParam::Text(kind.into()));
    let by_kind_idx = params.len();
    params.push(json_to_query_param(id));
    let id_idx = params.len();
    params.push(QueryParam::Int(if_version as i64));
    let ver_idx = params.len();

    let returning = full_returning_columns(table);
    let sql = format!(
        "UPDATE {tbl} SET \"is_deleted\" = FALSE, \"updated_by\" = {by_sql}, \"updated_by_kind\" = ${by_kind_idx} \
         WHERE \"id\" = ${id_idx}{cast} AND \"version\" = ${ver_idx} AND \"is_deleted\" = TRUE RETURNING {ret};",
        tbl = quote_ident(&table.name),
        cast = id_cast(table),
        by_sql = by_sql,
        by_kind_idx = by_kind_idx,
        id_idx = id_idx,
        ver_idx = ver_idx,
        ret = returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
    );
    check_param_count(&sql, params.len(), &table.name)?;
    Ok(CompiledMutation {
        sql,
        params,
        returning,
    })
}

/// Single-row fetch: `SELECT … FROM {table} WHERE id=$ AND
/// is_deleted=FALSE`. `include_deleted` lifts the soft-delete filter.
pub fn build_get(
    table: &TableDefinition,
    id: &Value,
    include_deleted: bool,
) -> CompiledMutation {
    let returning = full_returning_columns(table);
    let mut params = Vec::new();
    params.push(json_to_query_param(id));
    let where_extra = if include_deleted {
        ""
    } else {
        " AND \"is_deleted\" = FALSE"
    };
    let sql = format!(
        "SELECT {ret} FROM {tbl} WHERE \"id\" = $1{cast}{extra};",
        ret = returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
        tbl = quote_ident(&table.name),
        cast = id_cast(table),
        extra = where_extra,
    );
    CompiledMutation {
        sql,
        params,
        returning,
    }
}

fn full_returning_columns(table: &TableDefinition) -> Vec<String> {
    let mut out: Vec<String> = BASE_COLUMNS.iter().map(|s| s.to_string()).collect();
    for c in &table.columns {
        out.push(c.name.clone());
    }
    out
}

fn reject_base_columns(payload: &BTreeMap<String, Value>) -> Result<()> {
    for k in payload.keys() {
        if is_base_column(k) {
            return Err(DataverseError::internal(format!(
                "payload column '{}' is reserved by the base data model",
                k
            )));
        }
    }
    Ok(())
}

fn validate_payload_columns(
    table: &TableDefinition,
    payload: &BTreeMap<String, Value>,
) -> Result<()> {
    for k in payload.keys() {
        if !table.columns.iter().any(|c| &c.name == k) {
            return Err(DataverseError::internal(format!(
                "payload column '{}' is not declared on table '{}'",
                k, table.name
            )));
        }
    }
    Ok(())
}

fn json_to_query_param(v: &Value) -> QueryParam {
    match v {
        Value::Null => QueryParam::Null,
        Value::Bool(b) => QueryParam::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryParam::Int(i)
            } else if let Some(f) = n.as_f64() {
                QueryParam::Float(f)
            } else {
                QueryParam::Text(n.to_string())
            }
        }
        Value::String(s) => QueryParam::Text(s.clone()),
        // Arrays / objects are passed as JSON text — Postgres JSONB columns
        // accept the cast; the orchestrator binds them as JSON when the
        // column type is JSON/JSONB. For simple text columns this becomes a
        // raw JSON literal which is rarely what the caller wants.
        Value::Array(_) | Value::Object(_) => QueryParam::Text(v.to_string()),
    }
}

/// Return the SQL fragment for a `created_by`/`updated_by`/`deleted_by`
/// position, pushing a `QueryParam::Uuid` when the identity carries an
/// actor UUID, or emitting the literal `NULL::uuid` when it doesn't
/// (`Identity::System`). We do NOT bind `QueryParam::Null` for these
/// columns because the bind layer encodes `Null` as `Option::<i64>::None`
/// (bigint NULL) which Postgres then refuses to cast to `uuid`. Mirrors
/// the typed-null inlining already used for nullable data columns
/// (cf. `typed_null_literal`).
fn actor_uuid_sql(
    actor_uuid: Option<uuid::Uuid>,
    params: &mut Vec<QueryParam>,
) -> String {
    match actor_uuid {
        Some(u) => {
            params.push(QueryParam::Uuid(u.to_string()));
            format!("${}", params.len())
        }
        None => "NULL::uuid".into(),
    }
}

/// Defense-in-depth invariant for every compiled mutation: the distinct `$N`
/// placeholders in the SQL must be exactly `1..=params.len()`.
///
/// Why this exists: a placeholder with no bound parameter (or a bound param with
/// no placeholder) makes Postgres reject the Bind message with an opaque
/// `08P01` "insufficient data left in message" — impossible to attribute. We
/// fail loudly here instead, naming the table, before any DB round-trip. The
/// builders are aligned by construction; this guards against future drift (and
/// is asserted directly by `param_count_invariant_holds`).
fn check_param_count(sql: &str, params_len: usize, table: &str) -> Result<()> {
    let mut indices = std::collections::BTreeSet::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // `$N` only ever appears as a bind placeholder in our builders — values
        // are bound, never inlined — so a naive scan is safe (no string literals
        // carry user `$N`).
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            let mut n = 0usize;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                n = n * 10 + (bytes[j] - b'0') as usize;
                j += 1;
            }
            indices.insert(n);
            i = j;
        } else {
            i += 1;
        }
    }
    let expected: std::collections::BTreeSet<usize> = (1..=params_len).collect();
    if indices != expected {
        return Err(DataverseError::internal(format!(
            "placeholder/param count mismatch on table '{}': sql placeholders={:?}, params.len()={}",
            table, indices, params_len
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDefinition, FieldType, IdStrategy};
    use chrono::Utc;
    use serde_json::json;
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
                col("name", FieldType::Text),
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
    fn insert_appends_audit_columns() {
        let mut p = BTreeMap::new();
        p.insert("qty".into(), json!(3));
        p.insert("name".into(), json!("widget"));
        let m = build_insert(&table_orders(), &p, &id()).unwrap();
        assert!(m.sql.starts_with("INSERT INTO \"orders\" "));
        assert!(m.sql.contains("\"created_by\""));
        assert!(m.sql.contains("\"updated_by\""));
        assert!(m.sql.contains("\"created_by_kind\""));
        assert!(m.sql.contains("\"updated_by_kind\""));
        assert!(m.sql.contains("RETURNING"));
        // Two payload values + 2 uuid binds + 2 kind binds = 6 params.
        assert_eq!(m.params.len(), 6);
    }

    #[test]
    fn insert_rejects_payload_with_base_column() {
        let mut p = BTreeMap::new();
        p.insert("qty".into(), json!(3));
        p.insert("created_by".into(), json!("x"));
        let err = build_insert(&table_orders(), &p, &id()).unwrap_err();
        assert!(format!("{}", err).contains("reserved"));
    }

    #[test]
    fn insert_rejects_unknown_column() {
        let mut p = BTreeMap::new();
        p.insert("nope".into(), json!(1));
        assert!(build_insert(&table_orders(), &p, &id()).is_err());
    }

    #[test]
    fn update_emits_if_match_predicate() {
        let mut p = BTreeMap::new();
        p.insert("name".into(), json!("renamed"));
        let m = build_update(&table_orders(), &json!(42), 3, &p, &id()).unwrap();
        assert!(m.sql.contains("UPDATE \"orders\""));
        assert!(m.sql.contains("\"version\" = "));
        assert!(m.sql.contains("\"is_deleted\" = FALSE"));
        assert!(m.sql.contains("RETURNING"));
    }

    #[test]
    fn update_rejects_empty_payload() {
        let p = BTreeMap::new();
        assert!(build_update(&table_orders(), &json!(1), 0, &p, &id()).is_err());
    }

    #[test]
    fn soft_delete_filters_active_only() {
        let m = build_soft_delete(&table_orders(), &json!(1), 5, &id()).unwrap();
        assert!(m.sql.contains("\"is_deleted\" = TRUE"));
        assert!(m.sql.contains("AND \"is_deleted\" = FALSE")); // WHERE clause
    }

    #[test]
    fn restore_filters_deleted_only() {
        let m = build_restore(&table_orders(), &json!(1), 5, &id()).unwrap();
        assert!(m.sql.contains("\"is_deleted\" = FALSE"));  // SET clause
        assert!(m.sql.contains("AND \"is_deleted\" = TRUE")); // WHERE clause
    }

    #[test]
    fn get_default_excludes_deleted() {
        let m = build_get(&table_orders(), &json!(1), false);
        assert!(m.sql.contains("\"is_deleted\" = FALSE"));
    }

    #[test]
    fn get_include_deleted() {
        let m = build_get(&table_orders(), &json!(1), true);
        assert!(!m.sql.contains("\"is_deleted\" = FALSE"));
    }

    #[test]
    fn returning_includes_base_and_user_columns() {
        let mut p = BTreeMap::new();
        p.insert("qty".into(), json!(1));
        let m = build_insert(&table_orders(), &p, &id()).unwrap();
        assert!(m.returning.contains(&"id".to_string()));
        assert!(m.returning.contains(&"version".to_string()));
        assert!(m.returning.contains(&"is_deleted".to_string()));
        assert!(m.returning.contains(&"qty".to_string()));
        assert!(m.returning.contains(&"name".to_string()));
    }

    #[test]
    fn system_identity_inlines_null_uuid_in_sql() {
        let mut p = BTreeMap::new();
        p.insert("qty".into(), json!(1));
        let m = build_insert(&table_orders(), &p, &Identity::system()).unwrap();
        // `created_by` / `updated_by` are inlined as `NULL::uuid` SQL literals
        // (not bound params) — bind layer would otherwise encode `Null` as
        // `Option::<i64>::None` and PG rejects bigint→uuid. Mirrors the
        // behaviour for nullable Uuid data columns (typed_null_literal).
        assert!(
            m.sql.contains("NULL::uuid"),
            "expected NULL::uuid SQL literal for created_by/updated_by, got SQL: {}",
            m.sql
        );
        // The remaining audit binds are the two `kind` Text values.
        let last2 = &m.params[m.params.len() - 2..];
        assert!(matches!(&last2[0], QueryParam::Text(s) if s == "system"));
        assert!(matches!(&last2[1], QueryParam::Text(s) if s == "system"));
        // Sanity: no Null param remains anywhere in the bind vector for this
        // single-column-non-null payload.
        assert!(
            !m.params.iter().any(|p| matches!(p, QueryParam::Null)),
            "no QueryParam::Null should remain when Identity is System"
        );
    }

    #[test]
    fn user_identity_binds_uuid_param() {
        let mut p = BTreeMap::new();
        p.insert("qty".into(), json!(1));
        let actor = uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let m = build_insert(&table_orders(), &p, &Identity::user(actor, "alice")).unwrap();
        // No NULL::uuid literal — both audit positions are $N placeholders.
        assert!(
            !m.sql.contains("NULL::uuid"),
            "user identity must bind UUID params, not inline NULL::uuid"
        );
        // Last 4 params: uuid, uuid, "user", "user"
        let last4 = &m.params[m.params.len() - 4..];
        assert!(matches!(&last4[0], QueryParam::Uuid(s) if s == &actor.to_string()));
        assert!(matches!(&last4[1], QueryParam::Uuid(s) if s == &actor.to_string()));
        assert!(matches!(&last4[2], QueryParam::Text(s) if s == "user"));
        assert!(matches!(&last4[3], QueryParam::Text(s) if s == "user"));
    }

    fn table_typed() -> TableDefinition {
        TableDefinition {
            name: "events".into(),
            slug: "events".into(),
            columns: vec![
                col("happened_at", FieldType::DateTime),
                col("when_day", FieldType::Date),
                col("ref_id", FieldType::Uuid),
                col("payload", FieldType::Json),
                col("label", FieldType::Text),
            ],
            description: None,
            id_strategy: IdStrategy::Bigserial,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn insert_routes_typed_columns() {
        let mut p = BTreeMap::new();
        p.insert("happened_at".into(), json!("2026-05-06T14:30:00Z"));
        p.insert("when_day".into(), json!("2026-05-06"));
        p.insert("ref_id".into(), json!("00000000-0000-0000-0000-000000000001"));
        p.insert("payload".into(), json!({"k": [1, 2]}));
        p.insert("label".into(), json!("hello"));
        let m = build_insert(&table_typed(), &p, &id()).unwrap();

        // Json columns carry a `::jsonb` cast on their placeholder (because
        // we bind via Text). DateTime/Date/Uuid use typed `QueryParam`
        // variants that already tell sqlx the PG type, so no cast needed.
        assert!(m.sql.contains("::jsonb"), "expected ::jsonb cast, got: {}", m.sql);
        assert!(!m.sql.contains("::timestamptz"));
        assert!(!m.sql.contains("::date"));
        // `::uuid` may legitimately appear for the row-id WHERE side on uuid
        // tables; here the table is bigserial so it should not appear at all.
        assert!(!m.sql.contains("::uuid"));

        // First 5 params are payload columns (BTreeMap iterates in key order):
        // happened_at, label, payload, ref_id, when_day.
        assert!(matches!(m.params[0], QueryParam::Timestamp(ref s) if s == "2026-05-06T14:30:00Z"),
            "happened_at should bind as Timestamp, got {:?}", m.params[0]);
        assert!(matches!(m.params[1], QueryParam::Text(ref s) if s == "hello"));
        assert!(matches!(m.params[2], QueryParam::Text(ref s) if s == r#"{"k":[1,2]}"#),
            "payload should bind as text JSON, got {:?}", m.params[2]);
        assert!(matches!(m.params[3], QueryParam::Uuid(ref s) if s == "00000000-0000-0000-0000-000000000001"));
        assert!(matches!(m.params[4], QueryParam::Date(ref s) if s == "2026-05-06"));
    }

    #[test]
    fn update_routes_typed_columns() {
        let mut p = BTreeMap::new();
        p.insert("happened_at".into(), json!("2026-05-06T14:30:00Z"));
        p.insert("payload".into(), json!({"k": "v"}));
        let m = build_update(&table_typed(), &json!(7), 1, &p, &id()).unwrap();

        // payload SET clause carries a ::jsonb cast (Text bind).
        // happened_at SET carries no cast (typed Timestamp bind).
        assert!(m.sql.contains("\"payload\" = $") && m.sql.contains("::jsonb"),
            "expected ::jsonb on payload SET, got: {}", m.sql);
        let happened_at_set = m.sql.split(',')
            .find(|s| s.contains("\"happened_at\" = "))
            .expect("happened_at SET present");
        assert!(!happened_at_set.contains("::"),
            "happened_at SET should not carry cast (typed bind), got: {}", happened_at_set);

        // Params: payload columns first (BTreeMap key order: happened_at, payload),
        // then audit (uuid, kind), then id (Int 7), then version (Int 1).
        assert!(matches!(m.params[0], QueryParam::Timestamp(ref s) if s == "2026-05-06T14:30:00Z"));
        assert!(matches!(m.params[1], QueryParam::Text(ref s) if s == r#"{"k":"v"}"#));
    }

    #[test]
    fn null_value_on_typed_column_uses_inline_literal() {
        // A null value on a DateTime/Json/Date/Uuid column emits an inline
        // SQL literal (`NULL::timestamptz`, `NULL::jsonb`, …) instead of
        // binding a parameter — so sqlx never has a chance to default the
        // bind type to bigint, which would fail to cast to e.g. jsonb.
        let mut p = BTreeMap::new();
        p.insert("happened_at".into(), json!(null));
        p.insert("payload".into(), json!(null));
        let m = build_update(&table_typed(), &json!(1), 0, &p, &id()).unwrap();
        assert!(m.sql.contains("\"happened_at\" = NULL::timestamptz"),
            "expected inline NULL::timestamptz, got: {}", m.sql);
        assert!(m.sql.contains("\"payload\" = NULL::jsonb"),
            "expected inline NULL::jsonb, got: {}", m.sql);
        // None of the typed-null payload values should land in `params`:
        // params should be only the audit (uuid + kind) + id + version, i.e. 4.
        assert_eq!(m.params.len(), 4, "params should not carry typed nulls; got {:?}", m.params);
    }

    #[test]
    fn check_param_count_rejects_mismatch() {
        // More params than placeholders → actionable, table-named error.
        let err = check_param_count("INSERT INTO t (a,b) VALUES ($1,$2)", 3, "t").unwrap_err();
        assert!(format!("{err}").contains("count mismatch"), "got: {err}");
        assert!(format!("{err}").contains("'t'"), "error must name the table: {err}");
        // A gap in the indices ($1,$3 with len 2) is also a mismatch.
        assert!(check_param_count("VALUES ($1, $3)", 2, "t").is_err());
        // Cast suffixes and multi-digit indices parse correctly.
        assert!(check_param_count("VALUES ($1::jsonb, $2::numeric)", 2, "t").is_ok());
        let many = (1..=12).map(|n| format!("${n}")).collect::<Vec<_>>().join(",");
        assert!(check_param_count(&format!("VALUES ({many})"), 12, "t").is_ok());
    }

    #[test]
    fn param_count_invariant_holds() {
        // Every mutation builder, across all identity kinds, typed columns and
        // inlined typed-nulls, must keep `$N` placeholders == params.len().
        // The builders now enforce this internally (`check_param_count`), so an
        // `.unwrap()` already proves it; we re-check explicitly for clarity and
        // to cover the System identity (NULL::uuid inlined) + typed-null paths
        // that drop params without dropping correctness.
        let identities = [
            Identity::system(),
            Identity::user(Uuid::new_v4(), "alice"),
            Identity::app(Uuid::new_v4(), "trader"),
        ];
        for ident in &identities {
            let mut full = BTreeMap::new();
            full.insert("happened_at".into(), json!("2026-05-06T14:30:00Z"));
            full.insert("when_day".into(), json!("2026-05-06"));
            full.insert("ref_id".into(), json!("00000000-0000-0000-0000-000000000001"));
            full.insert("payload".into(), json!({"k": [1, 2]}));
            full.insert("label".into(), json!("hello"));
            let m = build_insert(&table_typed(), &full, ident).unwrap();
            check_param_count(&m.sql, m.params.len(), "events").unwrap();

            // Typed-nulls (inlined as NULL::… literals, no bound param) mixed
            // with a present value — the trap that would desync the counts.
            let mut nulls = BTreeMap::new();
            nulls.insert("happened_at".into(), json!(null));
            nulls.insert("payload".into(), json!(null));
            nulls.insert("label".into(), json!("present"));
            let mn = build_insert(&table_typed(), &nulls, ident).unwrap();
            check_param_count(&mn.sql, mn.params.len(), "events").unwrap();

            // Update / soft-delete / restore (If-Match WHERE id+version).
            let mut upd = BTreeMap::new();
            upd.insert("label".into(), json!("renamed"));
            let mu = build_update(&table_typed(), &json!(7), 1, &upd, ident).unwrap();
            check_param_count(&mu.sql, mu.params.len(), "events").unwrap();
            let md = build_soft_delete(&table_typed(), &json!(7), 1, ident).unwrap();
            check_param_count(&md.sql, md.params.len(), "events").unwrap();
            let mr = build_restore(&table_typed(), &json!(7), 1, ident).unwrap();
            check_param_count(&mr.sql, mr.params.len(), "events").unwrap();
        }
    }

    #[test]
    fn insert_typed_null_uses_inline_literal() {
        let mut p = BTreeMap::new();
        p.insert("payload".into(), json!(null));
        p.insert("happened_at".into(), json!(null));
        p.insert("label".into(), json!("present"));
        let m = build_insert(&table_typed(), &p, &id()).unwrap();
        assert!(m.sql.contains("NULL::jsonb"), "got: {}", m.sql);
        assert!(m.sql.contains("NULL::timestamptz"), "got: {}", m.sql);
        // Only the non-null `label` + 4 audit columns make it into params (5 total).
        assert_eq!(m.params.len(), 5, "got {:?}", m.params);
    }

    #[test]
    fn null_on_plain_column_is_inlined_not_bound() {
        // Regression for the `scan_history_events` 08P01/22P03 flood: a NULL on a
        // plain (text/number/bool) column must be inlined as a bare `NULL`
        // literal, NOT bound as `QueryParam::Null` (which the bind layer encodes
        // as a `bigint` NULL and which poisons the cached prepared statement's
        // param type for that position — see `typed_null_literal`).
        let mut p = BTreeMap::new();
        p.insert("name".into(), json!(null)); // `name` is a Text column
        let m = build_insert(&table_orders(), &p, &id()).unwrap();
        assert!(
            m.sql.contains("\"name\""),
            "name column present in INSERT: {}",
            m.sql
        );
        assert!(
            !m.sql.contains("NULL::"),
            "a text NULL needs no cast, just bare NULL: {}",
            m.sql
        );
        assert!(
            !m.params.iter().any(|p| matches!(p, QueryParam::Null)),
            "text NULL must be inlined, never bound as a bigint NULL: {:?}",
            m.params
        );
        // The null vs non-null shapes must compile to DIFFERENT SQL, so they map
        // to distinct cached prepared statements (no cross-position poisoning).
        let mut p2 = BTreeMap::new();
        p2.insert("name".into(), json!("widget"));
        let m2 = build_insert(&table_orders(), &p2, &id()).unwrap();
        assert_ne!(
            m.sql, m2.sql,
            "null and non-null inserts must produce distinct SQL"
        );
    }
}
