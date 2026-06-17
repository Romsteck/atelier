//! IPC dispatch for the dataverse REST gateway. Each function takes the
//! shared [`AppsContext`] (owns the `DataverseManager`) and returns a
//! ready-to-send [`IpcResponse`].
//!
//! These handlers wrap the pure builders in [`atelier_dataverse::query`],
//! [`atelier_dataverse::crud`] and [`atelier_dataverse::audit`] — the builders stay
//! IO-free and unit-testable; this module is the only one that touches
//! Postgres for gateway operations.

use std::collections::BTreeMap;

use atelier_common::Identity;
use atelier_dataverse::{
    audit::{build_audit_insert, AuditOp},
    crud::{build_get, build_insert, build_restore, build_soft_delete, build_update},
    dv_io::{run_count, run_get, run_list, run_mutation, MutationOutcome},
    query::{build_count_sql, build_list_sql, ListQuery, QueryParam},
    DatabaseSchema, TableDefinition,
};
use atelier_ipc::types::IpcResponse;
use serde_json::{json, Value};
use tracing::{info, warn};

use super::apps_ops::AppsContext;

/// `DvSchema` — return tables/columns/relations as a JSON object.
pub async fn dv_schema(ctx: &AppsContext, slug: String) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    match engine.get_schema().await {
        Ok(schema) => IpcResponse::ok_data(schema_to_public_json(&schema)),
        Err(e) => IpcResponse::err(format!("dv_schema: {e}")),
    }
}

/// `DvList` — list with $filter/$select/$expand/$orderby/$top/$skip/$count.
pub async fn dv_list(
    ctx: &AppsContext,
    slug: String,
    table: String,
    query: Value,
    identity: Identity,
) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("dv_list: {e}")),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => return IpcResponse::err(format!("table '{}' not found", table)),
    };
    let lq: ListQuery = match serde_json::from_value(query) {
        Ok(q) => q,
        Err(e) => return IpcResponse::err(format!("invalid $query: {e}")),
    };

    let compiled = match build_list_sql(table_def, &lq, &identity) {
        Ok(c) => c,
        Err(e) => return IpcResponse::err(format!("$filter: {e}")),
    };
    let rows = match run_list(engine.pool(), table_def, &compiled).await {
        Ok(r) => r,
        Err(e) => return IpcResponse::err(format!("dv_list: {e}")),
    };

    let mut envelope = serde_json::Map::new();
    envelope.insert("value".into(), Value::Array(rows));

    if lq.count {
        match build_count_sql(table_def, &lq, &identity) {
            Ok((sql, params)) => match run_count(engine.pool(), &sql, &params).await {
                Ok(n) => {
                    envelope.insert("@count".into(), json!(n));
                }
                Err(e) => return IpcResponse::err(format!("dv_count: {e}")),
            },
            Err(e) => return IpcResponse::err(format!("$count: {e}")),
        }
    }

    info!(slug = %slug, table = %table, "DvList ok");
    IpcResponse::ok_data(Value::Object(envelope))
}

/// `DvGet` — single row.
pub async fn dv_get(
    ctx: &AppsContext,
    slug: String,
    table: String,
    id: Value,
    include_deleted: bool,
    _identity: Identity,
) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("dv_get: {e}")),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => return IpcResponse::err(format!("table '{}' not found", table)),
    };

    let compiled = build_get(table_def, &id, include_deleted);
    match run_get(engine.pool(), table_def, &compiled.sql, &compiled.params).await {
        Ok(Some(row)) => IpcResponse::ok_data(row),
        Ok(None) => IpcResponse::err("not_found"),
        Err(e) => IpcResponse::err(format!("dv_get: {e}")),
    }
}

/// `DvInsert`.
pub async fn dv_insert(
    ctx: &AppsContext,
    slug: String,
    table: String,
    payload: BTreeMap<String, Value>,
    identity: Identity,
) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("dv_insert: {e}")),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => return IpcResponse::err(format!("table '{}' not found", table)),
    };

    let mutation = match build_insert(table_def, &payload, &identity) {
        Ok(m) => m,
        Err(e) => return IpcResponse::err(format!("dv_insert: {e}")),
    };

    // We can only build the audit row AFTER the INSERT (we need the
    // generated id). Run the mutation first; on success, follow up with
    // an audit insert in a separate execution. Atomicity gap: an audit
    // insert failing leaves a "silent" row; logged loud and accepted.
    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &Value::Null,
    )
    .await
    {
        Ok(MutationOutcome::Applied(row)) => {
            audit_after(engine.pool(), &table, &row, AuditOp::Insert, &identity, None).await;
            info!(slug = %slug, table = %table, "DvInsert ok");
            IpcResponse::ok_data(row)
        }
        Ok(other) => IpcResponse::err(format!("dv_insert: unexpected {:?}", other)),
        Err(e) => IpcResponse::err(format!("dv_insert: {e}")),
    }
}

/// `DvUpdate`.
pub async fn dv_update(
    ctx: &AppsContext,
    slug: String,
    table: String,
    id: Value,
    if_version: i32,
    payload: BTreeMap<String, Value>,
    identity: Identity,
) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("dv_update: {e}")),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => return IpcResponse::err(format!("table '{}' not found", table)),
    };

    // Snapshot before — for audit diff.
    let before = match run_get(
        engine.pool(),
        table_def,
        &build_get(table_def, &id, true).sql,
        &[QueryParam::Int(0); 0]
            .iter()
            .cloned()
            .chain(std::iter::once(json_to_query_param(&id)))
            .collect::<Vec<_>>(),
    )
    .await
    {
        Ok(b) => b,
        Err(_) => None,
    };

    let mutation = match build_update(table_def, &id, if_version, &payload, &identity) {
        Ok(m) => m,
        Err(e) => return IpcResponse::err(format!("dv_update: {e}")),
    };

    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &id,
    )
    .await
    {
        Ok(MutationOutcome::Applied(row)) => {
            audit_after(
                engine.pool(),
                &table,
                &row,
                AuditOp::Update,
                &identity,
                before.as_ref(),
            )
            .await;
            info!(slug = %slug, table = %table, "DvUpdate ok");
            IpcResponse::ok_data(row)
        }
        Ok(MutationOutcome::PreconditionFailed) => {
            warn!(slug = %slug, table = %table, "DvUpdate precondition failed");
            IpcResponse::err("precondition_failed")
        }
        Ok(MutationOutcome::NotFound) => IpcResponse::err("not_found"),
        Err(e) => IpcResponse::err(format!("dv_update: {e}")),
    }
}

/// `DvSoftDelete`.
pub async fn dv_soft_delete(
    ctx: &AppsContext,
    slug: String,
    table: String,
    id: Value,
    if_version: i32,
    identity: Identity,
) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("dv_soft_delete: {e}")),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => return IpcResponse::err(format!("table '{}' not found", table)),
    };

    let mutation = match build_soft_delete(table_def, &id, if_version, &identity) {
        Ok(m) => m,
        Err(e) => return IpcResponse::err(format!("dv_soft_delete: {e}")),
    };

    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &id,
    )
    .await
    {
        Ok(MutationOutcome::Applied(row)) => {
            audit_after(engine.pool(), &table, &row, AuditOp::Delete, &identity, None).await;
            info!(slug = %slug, table = %table, "DvSoftDelete ok");
            IpcResponse::ok_data(row)
        }
        Ok(MutationOutcome::PreconditionFailed) => IpcResponse::err("precondition_failed"),
        Ok(MutationOutcome::NotFound) => IpcResponse::err("not_found"),
        Err(e) => IpcResponse::err(format!("dv_soft_delete: {e}")),
    }
}

/// `DvRestore`.
pub async fn dv_restore(
    ctx: &AppsContext,
    slug: String,
    table: String,
    id: Value,
    if_version: i32,
    identity: Identity,
) -> IpcResponse {
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("dv_restore: {e}")),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => return IpcResponse::err(format!("table '{}' not found", table)),
    };

    let mutation = match build_restore(table_def, &id, if_version, &identity) {
        Ok(m) => m,
        Err(e) => return IpcResponse::err(format!("dv_restore: {e}")),
    };

    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &id,
    )
    .await
    {
        Ok(MutationOutcome::Applied(row)) => {
            audit_after(engine.pool(), &table, &row, AuditOp::Restore, &identity, None).await;
            info!(slug = %slug, table = %table, "DvRestore ok");
            IpcResponse::ok_data(row)
        }
        Ok(MutationOutcome::PreconditionFailed) => IpcResponse::err("precondition_failed"),
        Ok(MutationOutcome::NotFound) => IpcResponse::err("not_found"),
        Err(e) => IpcResponse::err(format!("dv_restore: {e}")),
    }
}

/// `DvAuditList`. Reads `_dv_audit` directly with a small ad-hoc query.
pub async fn dv_audit_list(
    ctx: &AppsContext,
    slug: String,
    table: Option<String>,
    row_id: Option<String>,
    op: Option<String>,
    since: Option<String>,
    top: Option<u32>,
    skip: Option<u32>,
    _identity: Identity,
) -> IpcResponse {
    use sqlx_core::arguments::Arguments;
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let mut sql = String::from(
        "SELECT id, ts, table_name, row_id, op, actor_kind, actor_uuid, actor_label, before, after, diff \
         FROM _dv_audit",
    );
    let mut filters: Vec<String> = Vec::new();
    let mut args = sqlx_postgres::PgArguments::default();
    let mut idx = 1;
    if let Some(t) = table.as_ref() {
        filters.push(format!("table_name = ${}", idx));
        let _ = args.add(t.clone());
        idx += 1;
    }
    if let Some(r) = row_id.as_ref() {
        filters.push(format!("row_id = ${}", idx));
        let _ = args.add(r.clone());
        idx += 1;
    }
    if let Some(o) = op.as_ref() {
        filters.push(format!("op = ${}", idx));
        let _ = args.add(o.clone());
        idx += 1;
    }
    if let Some(s) = since.as_ref() {
        if let Ok(dt) = s.parse::<chrono::DateTime<chrono::Utc>>() {
            filters.push(format!("ts >= ${}", idx));
            let _ = args.add(dt);
            idx += 1;
        }
    }
    if !filters.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&filters.join(" AND "));
    }
    sql.push_str(" ORDER BY id DESC");
    let top = top.unwrap_or(50).min(1000);
    sql.push_str(&format!(" LIMIT {}", top));
    if let Some(skip) = skip {
        if skip > 0 {
            sql.push_str(&format!(" OFFSET {}", skip));
        }
    }
    sql.push(';');
    let _ = idx; // silence unused if no filters

    match sqlx_core::query::query_with(sqlx_core::sql_str::AssertSqlSafe(sql.as_str()), args)
        .fetch_all(engine.pool())
        .await
    {
        Ok(rows) => {
            let entries: Vec<Value> = rows.iter().map(decode_audit_row).collect();
            IpcResponse::ok_data(json!({ "value": entries }))
        }
        Err(e) => IpcResponse::err(format!("dv_audit_list: {e}")),
    }
}

/// SELECT-only forensic read against an app's database, for the surveillance
/// post-mortem scan. Unlike the gateway `dv_*` tools (no JOIN/aggregate/temporal
/// filter), this allows the cross-table correlation + freeze/gap detection a
/// causal post-mortem needs.
///
/// Read-only is enforced two ways: (1) a keyword guard rejecting write verbs and
/// multi-statements, and (2) — the hard guarantee — the user SQL is run as a
/// FROM-subquery (`SELECT to_jsonb(t) FROM ( <sql> ) t`). Postgres forbids
/// INSERT/UPDATE/DELETE/MERGE and data-modifying CTEs inside a sub-SELECT, so
/// even a guard miss cannot mutate. `statement_timeout` is inherited from the
/// pool's session defaults. Rows come back schema-less as JSON (`to_jsonb`), so
/// arbitrary projections decode without per-type handling. Exposed to the
/// surveillance scope as `pm_query`, and reused as the backend of the project
/// scope's `db_query` (the app agent's read path — same SELECT-only guarantee).
pub async fn pm_query(ctx: &AppsContext, slug: String, sql: String, limit: u32) -> IpcResponse {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return IpcResponse::err("pm_query: empty sql");
    }
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if head != "SELECT" && head != "WITH" {
        return IpcResponse::err("pm_query is SELECT-only (must start with SELECT or WITH)");
    }
    if trimmed.contains(';') {
        return IpcResponse::err("pm_query: a single statement only (no ';')");
    }
    let upper = format!(" {} ", trimmed.to_ascii_uppercase());
    const FORBIDDEN: &[&str] = &[
        " INSERT ", " UPDATE ", " DELETE ", " DROP ", " ALTER ", " TRUNCATE ", " CREATE ",
        " GRANT ", " REVOKE ", " COPY ", " CALL ", " MERGE ", " DO ", " VACUUM ", " LOCK ",
    ];
    if let Some(kw) = FORBIDDEN.iter().find(|kw| upper.contains(**kw)) {
        return IpcResponse::err(format!(
            "pm_query: forbidden keyword '{}' — SELECT-only",
            kw.trim()
        ));
    }
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let limit = limit.clamp(1, 5000);
    let wrapped = format!("SELECT to_jsonb(t) AS j FROM ( {trimmed} ) t LIMIT {limit}");
    match sqlx_core::query::query_with(
        sqlx_core::sql_str::AssertSqlSafe(wrapped.as_str()),
        sqlx_postgres::PgArguments::default(),
    )
    .fetch_all(engine.pool())
    .await
    {
        Ok(rows) => {
            use sqlx_core::row::Row as _;
            let out: Vec<Value> = rows
                .iter()
                .filter_map(|r| {
                    r.try_get::<sqlx_core::types::Json<Value>, _>("j")
                        .ok()
                        .map(|j| j.0)
                })
                .collect();
            let count = out.len();
            IpcResponse::ok_data(json!({ "rows": out, "count": count }))
        }
        Err(e) => IpcResponse::err(format!("pm_query: {e}")),
    }
}

/// `DvRotateToken` — admin op. Mints a fresh gateway token, then syncs
/// the app's `.env` so the new value lands on disk for the next restart.
/// (The currently-running process keeps the old token until it restarts.)
pub async fn dv_rotate_token(ctx: &AppsContext, slug: String) -> IpcResponse {
    let mgr = match ctx.dataverse_manager.as_ref() {
        Some(m) => m.clone(),
        None => return IpcResponse::err("dataverse_manager not configured"),
    };
    let token = match mgr.rotate_token(&slug) {
        Ok(t) => t,
        Err(e) => return IpcResponse::err(format!("dv_rotate_token: {e}")),
    };
    // Re-render the `.env` so the new platform-tier `HR_DV_TOKEN` lands on disk
    // for the next restart (the running process keeps the old token until then).
    if let Err(e) = ctx.reconcile_app_env(&slug, false).await {
        warn!(slug, error = %e, "rotate_token: env reconcile failed (token rotated, .env stale)");
    }
    IpcResponse::ok_data(json!({"token": token}))
}

/// `DvVerifyToken` — turns a bearer token into the app's uuid (or
/// errors with `unauthorized`). Called by hr-api as a precondition
/// before issuing the actual data IPC.
pub async fn dv_verify_token(ctx: &AppsContext, slug: String, token: String) -> IpcResponse {
    let mgr = match ctx.dataverse_manager.as_ref() {
        Some(m) => m.clone(),
        None => return IpcResponse::err("dataverse_manager not configured"),
    };
    match mgr.verify_token(&slug, &token) {
        Ok(uuid) => IpcResponse::ok_data(json!({"app_uuid": uuid.to_string(), "slug": slug})),
        Err(_) => IpcResponse::err("unauthorized"),
    }
}

// ── helpers ────────────────────────────────────────────────────────────

fn find_table<'a>(schema: &'a DatabaseSchema, name: &str) -> Option<&'a TableDefinition> {
    schema.tables.iter().find(|t| t.name == name)
}

fn schema_to_public_json(schema: &DatabaseSchema) -> Value {
    // Reasonable initial shape; future iterations can hide the base
    // columns under an `@meta` block per the plan.
    serde_json::to_value(schema).unwrap_or(Value::Null)
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
        _ => QueryParam::Text(v.to_string()),
    }
}

/// Best-effort audit insert. Logged on failure but does not propagate
/// the error — at the gateway boundary, an audit-only failure must not
/// roll back a successful data mutation. (Future hardening: bring
/// the audit insert inside the same transaction as the mutation.)
async fn audit_after(
    pool: &sqlx_postgres::PgPool,
    table: &str,
    after_row: &Value,
    op: AuditOp,
    identity: &Identity,
    before_row: Option<&Value>,
) {
    let row_id = after_row
        .get("id")
        .cloned()
        .unwrap_or(Value::String(String::new()));
    let before_for_audit = match op {
        AuditOp::Insert => None,
        _ => before_row,
    };
    let after_for_audit = match op {
        AuditOp::Delete => None,
        _ => Some(after_row),
    };

    let compiled = build_audit_insert(
        table,
        &row_id,
        op,
        identity,
        before_for_audit,
        after_for_audit,
    );

    let args = match atelier_dataverse::dv_io::bind_all(&compiled.params) {
        Ok(a) => a,
        Err(e) => {
            warn!(table, ?op, error = %e, "audit bind failed — skipping audit row");
            return;
        }
    };
    if let Err(e) = sqlx_core::query::query_with(sqlx_core::sql_str::AssertSqlSafe(compiled.sql.as_str()), args)
        .execute(pool)
        .await
    {
        warn!(table, ?op, error = %e, "audit insert failed — proceeding");
    }
}

fn decode_audit_row(row: &sqlx_postgres::PgRow) -> Value {
    use sqlx_core::row::Row as _;
    let id: i64 = row.try_get("id").unwrap_or(0);
    let ts: chrono::DateTime<chrono::Utc> = row
        .try_get("ts")
        .unwrap_or_else(|_| chrono::Utc::now());
    let table_name: String = row.try_get("table_name").unwrap_or_default();
    let row_id: String = row.try_get("row_id").unwrap_or_default();
    let op: String = row.try_get("op").unwrap_or_default();
    let actor_kind: String = row.try_get("actor_kind").unwrap_or_default();
    let actor_uuid: Option<uuid::Uuid> = row.try_get("actor_uuid").ok().flatten();
    let actor_label: Option<String> = row.try_get("actor_label").ok().flatten();
    let before: Option<sqlx_core::types::Json<Value>> = row.try_get("before").ok().flatten();
    let after: Option<sqlx_core::types::Json<Value>> = row.try_get("after").ok().flatten();
    let diff: Option<sqlx_core::types::Json<Value>> = row.try_get("diff").ok().flatten();

    json!({
        "id": id,
        "ts": ts.to_rfc3339(),
        "table_name": table_name,
        "row_id": row_id,
        "op": op,
        "actor_kind": actor_kind,
        "actor_uuid": actor_uuid.map(|u| u.to_string()),
        "actor_label": actor_label,
        "before": before.map(|j| j.0),
        "after": after.map(|j| j.0),
        "diff": diff.map(|j| j.0),
    })
}
