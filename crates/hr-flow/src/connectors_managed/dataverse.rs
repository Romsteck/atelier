//! `dataverse` connector — exposes the app's own Dataverse engine to flows.
//!
//! Wraps the pure SQL builders in `hr_dataverse::{query, crud, audit}` plus
//! the IO helpers in `hr_dataverse::dv_io`. The trait `Connector` only sees
//! a `(op, Value)` pair, so this module is essentially a router from named
//! ops to typed payloads — no SQL is rebuilt here.
//!
//! Instantiated per-app at `FlowEngine` build time: each connector owns
//! `Arc<DataverseManager>` + the `slug` it serves, so flows never see the
//! slug in their TOML.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use hr_common::Identity;
use hr_dataverse::audit::{AuditOp, build_audit_insert};
use hr_dataverse::crud::{build_get, build_insert, build_restore, build_soft_delete, build_update};
use hr_dataverse::dv_io::{MutationOutcome, bind_all, run_count, run_get, run_list, run_mutation};
use hr_dataverse::query::{
    Direction, ListQuery, OrderBy, QueryParam, build_count_sql, build_list_sql,
};
use hr_dataverse::{DataverseManager, TableDefinition};
use serde_json::{Map, Value, json};
use tracing::{debug, warn};

use crate::connector::Connector;
use crate::error::{FlowError, FlowResult};

/// The connector. Cheap to clone (everything inside is `Arc` or `String`).
pub struct DataverseConnector {
    manager: Arc<DataverseManager>,
    slug: String,
}

impl DataverseConnector {
    pub fn new(manager: Arc<DataverseManager>, slug: impl Into<String>) -> Self {
        Self { manager, slug: slug.into() }
    }
}

#[async_trait]
impl Connector for DataverseConnector {
    fn name(&self) -> &str { "dataverse" }

    async fn call(&self, op: &str, params: Value) -> FlowResult<Value> {
        match op {
            "list" => self.op_list(params).await,
            "get" => self.op_get(params).await,
            "get_by_natural_key" => self.op_get_by_natural_key(params).await,
            "insert" => self.op_insert(params).await,
            "update" => self.op_update(params).await,
            "soft_delete" => self.op_soft_delete(params).await,
            "restore" => self.op_restore(params).await,
            "schema" => self.op_schema(params).await,
            "audit_list" => self.op_audit_list(params).await,
            _ => Err(FlowError::UnknownOperation {
                connector: "dataverse".into(),
                op: op.to_string(),
            }),
        }
    }
}

// ── ops ───────────────────────────────────────────────────────────────────

impl DataverseConnector {
    async fn op_list(&self, params: Value) -> FlowResult<Value> {
        let req = parse_list_params(&params)?;
        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("list", e))?;
        let table_def = find_table(&schema.tables, &req.table)?;

        let lq = req.into_list_query();
        let identity = Identity::system();
        let compiled = build_list_sql(table_def, &lq, &identity).map_err(|e| io_err("list", e))?;
        let rows = run_list(engine.pool(), table_def, &compiled).await.map_err(|e| io_err("list", e))?;

        let mut out = Map::new();
        out.insert("rows".into(), Value::Array(rows));
        if lq.count {
            let (sql, ps) = build_count_sql(table_def, &lq, &identity).map_err(|e| io_err("list", e))?;
            let n = run_count(engine.pool(), &sql, &ps).await.map_err(|e| io_err("list", e))?;
            out.insert("count".into(), json!(n));
        }
        Ok(Value::Object(out))
    }

    async fn op_get(&self, params: Value) -> FlowResult<Value> {
        let table = required_string(&params, "table")?;
        let id = required(&params, "id")?;
        let include_deleted = params.get("include_deleted").and_then(|v| v.as_bool()).unwrap_or(false);

        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("get", e))?;
        let table_def = find_table(&schema.tables, &table)?;
        let compiled = build_get(table_def, id, include_deleted);
        match run_get(engine.pool(), table_def, &compiled.sql, &compiled.params).await {
            Ok(Some(row)) => Ok(row),
            Ok(None) => Ok(Value::Null),
            Err(e) => Err(io_err("get", e)),
        }
    }

    /// `get_by_natural_key` — lookup by an arbitrary column. Returns the
    /// first matching row (≤ 1 expected) or `null`. Implemented on top
    /// of `build_list_sql` so it benefits from the soft-delete filter and
    /// optimistic-lock metadata for free.
    async fn op_get_by_natural_key(&self, params: Value) -> FlowResult<Value> {
        let table = required_string(&params, "table")?;
        let key = required_string(&params, "key")?;
        let value = required(&params, "value")?;

        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("get_by_natural_key", e))?;
        let table_def = find_table(&schema.tables, &table)?;

        let filter = format!("{key} == {}", render_dvexpr_literal(value));
        let lq = ListQuery {
            filter: Some(filter),
            select: Vec::new(),
            orderby: Vec::new(),
            top: Some(1),
            skip: None,
            count: false,
            include_deleted: false,
        };
        let identity = Identity::system();
        let compiled = build_list_sql(table_def, &lq, &identity)
            .map_err(|e| io_err("get_by_natural_key", e))?;
        let mut rows = run_list(engine.pool(), table_def, &compiled)
            .await
            .map_err(|e| io_err("get_by_natural_key", e))?;
        Ok(rows.pop().unwrap_or(Value::Null))
    }

    async fn op_insert(&self, params: Value) -> FlowResult<Value> {
        let table = required_string(&params, "table")?;
        let data = required_object(&params, "data")?;

        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("insert", e))?;
        let table_def = find_table(&schema.tables, &table)?;

        let identity = Identity::system();
        let mutation = build_insert(table_def, &data, &identity).map_err(|e| io_err("insert", e))?;
        let outcome = run_mutation(
            engine.pool(), table_def,
            &mutation.sql, &mutation.params,
            None, &[],
            &Value::Null,
        )
        .await
        .map_err(|e| io_err("insert", e))?;

        match outcome {
            MutationOutcome::Applied(row) => {
                audit_best_effort(engine.pool(), &table, &row, AuditOp::Insert, &identity, None).await;
                Ok(row)
            }
            other => Err(connector_err("insert", format!("unexpected outcome: {other:?}"))),
        }
    }

    async fn op_update(&self, params: Value) -> FlowResult<Value> {
        let table = required_string(&params, "table")?;
        let id = required(&params, "id")?;
        let version = required_i32(&params, "version")?;
        let data = required_object(&params, "data")?;

        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("update", e))?;
        let table_def = find_table(&schema.tables, &table)?;

        // Snapshot before for the audit diff. Best-effort.
        let before = {
            let compiled = build_get(table_def, id, true);
            run_get(engine.pool(), table_def, &compiled.sql, &compiled.params).await.ok().flatten()
        };

        let identity = Identity::system();
        let mutation = build_update(table_def, id, version, &data, &identity)
            .map_err(|e| io_err("update", e))?;
        let outcome = run_mutation(
            engine.pool(), table_def,
            &mutation.sql, &mutation.params,
            None, &[],
            id,
        )
        .await
        .map_err(|e| io_err("update", e))?;

        match outcome {
            MutationOutcome::Applied(row) => {
                audit_best_effort(
                    engine.pool(), &table, &row, AuditOp::Update, &identity, before.as_ref(),
                )
                .await;
                Ok(row)
            }
            MutationOutcome::PreconditionFailed => Err(connector_err(
                "update",
                "precondition_failed (version mismatch — re-fetch the row)".into(),
            )),
            MutationOutcome::NotFound => Err(connector_err("update", "not_found".into())),
        }
    }

    async fn op_soft_delete(&self, params: Value) -> FlowResult<Value> {
        let table = required_string(&params, "table")?;
        let id = required(&params, "id")?;
        let version = required_i32(&params, "version")?;

        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("soft_delete", e))?;
        let table_def = find_table(&schema.tables, &table)?;

        let identity = Identity::system();
        let mutation = build_soft_delete(table_def, id, version, &identity)
            .map_err(|e| io_err("soft_delete", e))?;
        let outcome = run_mutation(
            engine.pool(), table_def,
            &mutation.sql, &mutation.params,
            None, &[],
            id,
        )
        .await
        .map_err(|e| io_err("soft_delete", e))?;

        match outcome {
            MutationOutcome::Applied(row) => {
                audit_best_effort(engine.pool(), &table, &row, AuditOp::Delete, &identity, None).await;
                Ok(json!({ "id": row.get("id"), "version": row.get("version") }))
            }
            MutationOutcome::PreconditionFailed => Err(connector_err(
                "soft_delete",
                "precondition_failed (version mismatch)".into(),
            )),
            MutationOutcome::NotFound => Err(connector_err("soft_delete", "not_found".into())),
        }
    }

    async fn op_restore(&self, params: Value) -> FlowResult<Value> {
        let table = required_string(&params, "table")?;
        let id = required(&params, "id")?;
        let version = required_i32(&params, "version")?;

        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("restore", e))?;
        let table_def = find_table(&schema.tables, &table)?;

        let identity = Identity::system();
        let mutation = build_restore(table_def, id, version, &identity)
            .map_err(|e| io_err("restore", e))?;
        let outcome = run_mutation(
            engine.pool(), table_def,
            &mutation.sql, &mutation.params,
            None, &[],
            id,
        )
        .await
        .map_err(|e| io_err("restore", e))?;

        match outcome {
            MutationOutcome::Applied(row) => {
                audit_best_effort(engine.pool(), &table, &row, AuditOp::Restore, &identity, None).await;
                Ok(row)
            }
            MutationOutcome::PreconditionFailed => {
                Err(connector_err("restore", "precondition_failed".into()))
            }
            MutationOutcome::NotFound => Err(connector_err("restore", "not_found".into())),
        }
    }

    async fn op_schema(&self, _params: Value) -> FlowResult<Value> {
        let engine = self.engine().await?;
        let schema = engine.get_schema().await.map_err(|e| io_err("schema", e))?;
        serde_json::to_value(&schema).map_err(|e| io_err("schema", e))
    }

    /// `audit_list` — returns the raw audit rows for the table. Currently
    /// no filter beyond the table name; pagination by `top`/`skip` follows
    /// the same conventions as `list`.
    async fn op_audit_list(&self, params: Value) -> FlowResult<Value> {
        let table = params.get("table").and_then(|v| v.as_str()).map(String::from);
        let top = params.get("top").and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as i64;
        let skip = params.get("skip").and_then(|v| v.as_u64()).unwrap_or(0) as i64;
        let row_id = params.get("id").map(|v| v.to_string());

        let engine = self.engine().await?;
        let mut sql =
            String::from("SELECT id, ts, table_name, row_id, op, identity_kind, identity_sub, \
                          before_row, after_row FROM _dv_audit WHERE 1=1");
        let mut bind_params: Vec<QueryParam> = Vec::new();
        if let Some(t) = table {
            bind_params.push(QueryParam::Text(t));
            sql.push_str(&format!(" AND table_name = ${}", bind_params.len()));
        }
        if let Some(r) = row_id {
            bind_params.push(QueryParam::Text(r));
            sql.push_str(&format!(" AND row_id = ${}", bind_params.len()));
        }
        sql.push_str(" ORDER BY ts DESC LIMIT ");
        sql.push_str(&top.to_string());
        sql.push_str(" OFFSET ");
        sql.push_str(&skip.to_string());

        let args = bind_all(&bind_params).map_err(|e| io_err("audit_list", e))?;
        let rows = sqlx_core::query::query_with(&sql, args)
            .fetch_all(engine.pool())
            .await
            .map_err(|e| io_err("audit_list", e))?;

        use sqlx_core::row::Row as _;
        let out: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                let id: i64 = r.try_get("id").unwrap_or(0);
                let ts: chrono::DateTime<chrono::Utc> =
                    r.try_get("ts").unwrap_or_else(|_| chrono::Utc::now());
                json!({
                    "id": id,
                    "ts": ts.to_rfc3339(),
                    "table_name": r.try_get::<String, _>("table_name").unwrap_or_default(),
                    "row_id": r.try_get::<String, _>("row_id").unwrap_or_default(),
                    "op": r.try_get::<String, _>("op").unwrap_or_default(),
                    "identity_kind": r.try_get::<String, _>("identity_kind").unwrap_or_default(),
                    "identity_sub": r.try_get::<Option<String>, _>("identity_sub").unwrap_or(None),
                    "before_row": r.try_get::<Option<Value>, _>("before_row").unwrap_or(None),
                    "after_row": r.try_get::<Option<Value>, _>("after_row").unwrap_or(None),
                })
            })
            .collect();
        Ok(json!({ "rows": out }))
    }

    async fn engine(&self) -> FlowResult<Arc<hr_dataverse::DataverseEngine>> {
        self.manager
            .engine_for(&self.slug)
            .await
            .map_err(|e| io_err("engine_for", e))
    }
}

// ── helpers ───────────────────────────────────────────────────────────────

#[derive(Debug)]
struct ListParams {
    table: String,
    filter: Option<String>,
    select: Vec<String>,
    orderby: Vec<OrderBy>,
    top: Option<u32>,
    skip: Option<u32>,
    count: bool,
    include_deleted: bool,
}

impl ListParams {
    fn into_list_query(self) -> ListQuery {
        ListQuery {
            filter: self.filter,
            select: self.select,
            orderby: self.orderby,
            top: self.top,
            skip: self.skip,
            count: self.count,
            include_deleted: self.include_deleted,
        }
    }
}

fn parse_list_params(params: &Value) -> FlowResult<ListParams> {
    let table = required_string(params, "table")?;
    // `filter` MUST be a string when present — silently dropping a non-string
    // value (e.g., a template that resolved to null or an object) used to
    // produce false-positives on `check_files_exist`-style flows : the query
    // ran without filter, returned ALL rows, and the caller wrongly concluded
    // the candidate existed. Reject loudly to surface the upstream bug
    // (usually a missing `output` field or wrong path in the templated
    // expression).
    let filter = match params.get("filter") {
        None => None,
        Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return Err(FlowError::Connector(format!(
                "dataverse.list: `filter` must be a string (got {}). \
                 Common cause: the templated expression resolved to a non-string \
                 (object, array, or missing field). Verify the upstream step's output.",
                value_type_name(other)
            )));
        }
    };
    let select: Vec<String> = params
        .get("select")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let orderby = parse_orderby(params.get("orderby"))?;
    let top = params.get("top").and_then(|v| v.as_u64()).map(|n| n.min(1000) as u32);
    let skip = params.get("skip").and_then(|v| v.as_u64()).map(|n| n as u32);
    let count = params.get("count").and_then(|v| v.as_bool()).unwrap_or(false);
    let include_deleted = params.get("include_deleted").and_then(|v| v.as_bool()).unwrap_or(false);

    debug!(
        table = %table,
        filter = ?filter,
        top, skip, count, include_deleted,
        select_count = select.len(),
        "dataverse.list: parsed params"
    );

    Ok(ListParams { table, filter, select, orderby, top, skip, count, include_deleted })
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Accept `orderby` as either a single string `"created_at desc"` or an
/// array of either strings or objects `{column, direction}`.
fn parse_orderby(v: Option<&Value>) -> FlowResult<Vec<OrderBy>> {
    let Some(v) = v else { return Ok(Vec::new()); };
    let items: Vec<Value> = match v {
        Value::String(s) => vec![Value::String(s.clone())],
        Value::Array(a) => a.clone(),
        Value::Null => return Ok(Vec::new()),
        other => {
            return Err(connector_err(
                "list",
                format!("orderby must be a string or array, got {other:?}"),
            ));
        }
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(s) => {
                let (col, dir) = parse_orderby_string(&s);
                out.push(OrderBy { column: col, direction: dir });
            }
            Value::Object(map) => {
                let column = map
                    .get("column")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| connector_err("list", "orderby.column missing".into()))?
                    .to_string();
                let direction = match map.get("direction").and_then(|v| v.as_str()) {
                    Some("desc") | Some("DESC") => Direction::Desc,
                    _ => Direction::Asc,
                };
                out.push(OrderBy { column, direction });
            }
            other => {
                return Err(connector_err(
                    "list",
                    format!("invalid orderby item: {other:?}"),
                ));
            }
        }
    }
    Ok(out)
}

fn parse_orderby_string(s: &str) -> (String, Direction) {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_suffix(" desc").or_else(|| trimmed.strip_suffix(" DESC")) {
        (rest.trim().to_string(), Direction::Desc)
    } else if let Some(rest) = trimmed.strip_suffix(" asc").or_else(|| trimmed.strip_suffix(" ASC")) {
        (rest.trim().to_string(), Direction::Asc)
    } else {
        (trimmed.to_string(), Direction::Asc)
    }
}

fn required<'a>(params: &'a Value, key: &str) -> FlowResult<&'a Value> {
    params.get(key).ok_or_else(|| connector_err("dataverse", format!("missing param `{key}`")))
}

fn required_string(params: &Value, key: &str) -> FlowResult<String> {
    required(params, key)?
        .as_str()
        .map(String::from)
        .ok_or_else(|| connector_err("dataverse", format!("`{key}` must be a string")))
}

fn required_i32(params: &Value, key: &str) -> FlowResult<i32> {
    required(params, key)?
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| connector_err("dataverse", format!("`{key}` must be a 32-bit integer")))
}

fn required_object(params: &Value, key: &str) -> FlowResult<BTreeMap<String, Value>> {
    let v = required(params, key)?;
    let map = v
        .as_object()
        .ok_or_else(|| connector_err("dataverse", format!("`{key}` must be an object")))?;
    Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn find_table<'a>(tables: &'a [TableDefinition], name: &str) -> FlowResult<&'a TableDefinition> {
    tables
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| connector_err("dataverse", format!("table `{name}` not found")))
}

fn render_dvexpr_literal(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        }
        other => {
            let escaped = other.to_string().replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        }
    }
}

fn io_err<E: std::fmt::Display>(op: &str, e: E) -> FlowError {
    FlowError::Connector(format!("dataverse.{op}: {e}"))
}

fn connector_err(op: &str, message: String) -> FlowError {
    FlowError::Connector(format!("dataverse.{op}: {message}"))
}

/// Same fail-soft contract as the MCP `dv_*` ops audit_after — we never
/// abort the data mutation because the audit insert failed.
async fn audit_best_effort(
    pool: &sqlx_postgres::PgPool,
    table: &str,
    after_row: &Value,
    op: AuditOp,
    identity: &Identity,
    before_row: Option<&Value>,
) {
    let row_id = after_row.get("id").cloned().unwrap_or(Value::String(String::new()));
    let before_for_audit = match op {
        AuditOp::Insert => None,
        _ => before_row,
    };
    let after_for_audit = match op {
        AuditOp::Delete => None,
        _ => Some(after_row),
    };
    let compiled = build_audit_insert(
        table, &row_id, op, identity, before_for_audit, after_for_audit,
    );
    let args = match bind_all(&compiled.params) {
        Ok(a) => a,
        Err(e) => {
            warn!(table, ?op, error = %e, "audit bind failed");
            return;
        }
    };
    if let Err(e) = sqlx_core::query::query_with(&compiled.sql, args)
        .execute(pool)
        .await
    {
        warn!(table, ?op, error = %e, "audit insert failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_filter_must_be_string_or_absent() {
        // None / absent → ok
        let p = json!({ "table": "files" });
        assert!(parse_list_params(&p).is_ok(), "absent filter is allowed");

        // String → ok
        let p = json!({ "table": "files", "filter": "name == 'x'" });
        let out = parse_list_params(&p).expect("string filter parses");
        assert_eq!(out.filter.as_deref(), Some("name == 'x'"));

        // null → treated as absent (templates often resolve to null when
        // the upstream field is missing — must not be a hard error there).
        let p = json!({ "table": "files", "filter": null });
        let out = parse_list_params(&p).expect("null filter is treated as absent");
        assert!(out.filter.is_none());

        // Object → loud error (used to be silently dropped before the fix,
        // producing false-positives on `check_files_exist`-style flows)
        let p = json!({ "table": "files", "filter": { "oops": "object" } });
        let err = parse_list_params(&p).expect_err("object filter must error");
        let msg = err.to_string();
        assert!(
            msg.contains("filter") && msg.contains("string"),
            "diagnostic must point at the filter/string mismatch, got: {msg}"
        );

        // Array → idem
        let p = json!({ "table": "files", "filter": ["x", "y"] });
        assert!(parse_list_params(&p).is_err(), "array filter must error");

        // Number → idem
        let p = json!({ "table": "files", "filter": 42 });
        assert!(parse_list_params(&p).is_err(), "number filter must error");
    }
}
