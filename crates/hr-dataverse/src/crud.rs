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

use hr_common::Identity;
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
/// If `value` is JSON null on a column whose Postgres type isn't directly
/// reachable by sqlx's default `Option::<i64>::None` binding, return a
/// SQL literal like `"NULL::jsonb"` to be inlined in place of a parameter.
/// Returns `None` for non-null values or for columns where a plain bound
/// NULL works (`text`, `bigint`, `boolean`, etc.).
fn typed_null_literal(value: &Value, field_type: FieldType) -> Option<&'static str> {
    if !matches!(value, Value::Null) {
        return None;
    }
    match field_type {
        FieldType::Json => Some("NULL::jsonb"),
        FieldType::DateTime => Some("NULL::timestamptz"),
        FieldType::Date => Some("NULL::date"),
        FieldType::Uuid => Some("NULL::uuid"),
        _ => None,
    }
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
    // INSERT (no prior modifier).
    col_names.extend(["created_by", "updated_by", "created_by_kind", "updated_by_kind"]);
    placeholders.push(format!("${}", params.len() + 1));
    params.push(uuid_param(actor_uuid));
    placeholders.push(format!("${}", params.len() + 1));
    params.push(uuid_param(actor_uuid));
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

    params.push(uuid_param(actor_uuid));
    sets.push(format!("\"updated_by\" = ${}", params.len()));
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

    params.push(uuid_param(actor_uuid));
    let by_idx = params.len();
    params.push(QueryParam::Text(kind.into()));
    let by_kind_idx = params.len();
    params.push(json_to_query_param(id));
    let id_idx = params.len();
    params.push(QueryParam::Int(if_version as i64));
    let ver_idx = params.len();

    let returning = full_returning_columns(table);
    let sql = format!(
        "UPDATE {tbl} SET \"is_deleted\" = TRUE, \"updated_by\" = ${by_idx}, \"updated_by_kind\" = ${by_kind_idx} \
         WHERE \"id\" = ${id_idx}{cast} AND \"version\" = ${ver_idx} AND \"is_deleted\" = FALSE RETURNING {ret};",
        tbl = quote_ident(&table.name),
        cast = id_cast(table),
        by_idx = by_idx,
        by_kind_idx = by_kind_idx,
        id_idx = id_idx,
        ver_idx = ver_idx,
        ret = returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
    );
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

    params.push(uuid_param(actor_uuid));
    let by_idx = params.len();
    params.push(QueryParam::Text(kind.into()));
    let by_kind_idx = params.len();
    params.push(json_to_query_param(id));
    let id_idx = params.len();
    params.push(QueryParam::Int(if_version as i64));
    let ver_idx = params.len();

    let returning = full_returning_columns(table);
    let sql = format!(
        "UPDATE {tbl} SET \"is_deleted\" = FALSE, \"updated_by\" = ${by_idx}, \"updated_by_kind\" = ${by_kind_idx} \
         WHERE \"id\" = ${id_idx}{cast} AND \"version\" = ${ver_idx} AND \"is_deleted\" = TRUE RETURNING {ret};",
        tbl = quote_ident(&table.name),
        cast = id_cast(table),
        by_idx = by_idx,
        by_kind_idx = by_kind_idx,
        id_idx = id_idx,
        ver_idx = ver_idx,
        ret = returning
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
    );
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

fn uuid_param(uuid: Option<uuid::Uuid>) -> QueryParam {
    match uuid {
        Some(u) => QueryParam::Uuid(u.to_string()),
        None => QueryParam::Null,
    }
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
    fn system_identity_emits_null_uuid() {
        let mut p = BTreeMap::new();
        p.insert("qty".into(), json!(1));
        let m = build_insert(&table_orders(), &p, &Identity::system()).unwrap();
        // The 4 audit binds are the last 4. Two should be Null (uuids), two Text 'system'.
        let last4 = &m.params[m.params.len() - 4..];
        assert!(matches!(last4[0], QueryParam::Null));
        assert!(matches!(last4[1], QueryParam::Null));
        assert!(matches!(&last4[2], QueryParam::Text(s) if s == "system"));
        assert!(matches!(&last4[3], QueryParam::Text(s) if s == "system"));
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
}
