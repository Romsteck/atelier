//! Dataverse Gateway routes — full read + write surface.
//!
//! Atelier connecte en LAN aux Postgres apps via `state.dv: DataverseManager`.
//! Le DataverseManager gère pool admin, secrets, schema introspection.
//!
//! Routes exposées :
//! - GET    /dv/{slug}/$schema                          → schema introspection
//! - GET    /dv/{slug}/{table}                          → list (OData $filter/$select/$orderby/$top/$skip/$count)
//! - GET    /dv/{slug}/{table}/{id}                     → get single row
//! - POST   /dv/{slug}/{table}                          → insert (returns the new row)
//! - PATCH  /dv/{slug}/{table}/{id}  + If-Match: <ver>  → update (optimistic locking)
//! - DELETE /dv/{slug}/{table}/{id}  + If-Match: <ver>  → soft-delete
//! - POST   /dv/{slug}/{table}/$restore/{id} + If-Match → restore from soft-delete
//!
//! Toutes les mutations passent par `hr_dataverse::dv_io::run_mutation` qui
//! exécute la mutation + l'insert d'audit dans la même transaction.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hr_common::Identity;
use hr_dataverse::{
    DatabaseSchema, TableDefinition,
    audit::{AuditOp, build_audit_insert},
    crud::{build_get, build_insert, build_restore, build_soft_delete, build_update},
    dv_io::{MutationOutcome, run_count, run_get, run_list, run_mutation},
    query::{ListQuery, QueryParam, build_count_sql, build_list_sql},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/$schema", get(get_schema))
        .route("/{slug}/$repair", post(repair_schema))
        .route("/{slug}/{table}", get(list_rows).post(insert_row))
        .route(
            "/{slug}/{table}/{id}",
            get(get_row).patch(update_row).delete(soft_delete_row),
        )
        .route("/{slug}/{table}/$restore/{id}", post(restore_row))
}

// ── Identity / auth ────────────────────────────────────────────────────

async fn extract_identity(
    headers: &HeaderMap,
    state: &ApiState,
    slug: &str,
) -> Result<Identity, Response> {
    let dv = match state.dv.as_ref() {
        Some(m) => m,
        None => {
            return Err(error_resp(
                StatusCode::SERVICE_UNAVAILABLE,
                "dataverse manager not initialised",
            ));
        }
    };

    if let Some(auth) = headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = auth.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ").map(str::trim) {
                if !token.is_empty() {
                    return match dv.verify_token(slug, token) {
                        Ok(uuid) => Ok(Identity::app(uuid, slug.to_string())),
                        Err(_) => Err(error_resp(
                            StatusCode::UNAUTHORIZED,
                            "invalid bearer token",
                        )),
                    };
                }
            }
        }
    }

    if let Some(uid) = headers
        .get("x-remote-user-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
    {
        let username = headers
            .get("x-remote-user")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        return Ok(Identity::user(uid, username));
    }

    Err(error_resp(StatusCode::UNAUTHORIZED, "auth required"))
}

// ── Helpers ────────────────────────────────────────────────────────────

fn validate_slug(slug: &str) -> Result<(), Response> {
    let ok = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase())
            .unwrap_or(false)
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(error_resp(StatusCode::BAD_REQUEST, "invalid slug"))
    }
}

fn validate_table(table: &str) -> Result<(), Response> {
    let ok = !table.is_empty()
        && table.len() <= 64
        && table
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(error_resp(StatusCode::BAD_REQUEST, "invalid table name"))
    }
}

fn error_resp(code: StatusCode, msg: &str) -> Response {
    (
        code,
        Json(json!({"error": {"code": code_label(code), "message": msg}})),
    )
        .into_response()
}

/// Map an internal dataverse error to an HTTP response while:
///   1. logging the full error server-side (with a fresh correlation id),
///   2. returning a generic message + the correlation id to the client.
///
/// Use for any error that may originate from Postgres (`sqlx::Error`),
/// schema introspection, or migration — exposing those raw to the client
/// leaks internal table/relation names, constraint identifiers, and
/// schema layout (cf. audit P1 #8).
///
/// Stick to `error_resp` directly for user-facing validation errors that
/// the caller actually needs to read.
fn db_error_resp(context: &str, e: impl std::fmt::Display + std::fmt::Debug) -> Response {
    let correlation_id = uuid::Uuid::new_v4();
    tracing::error!(
        correlation_id = %correlation_id,
        context = %context,
        error = ?e,
        "dataverse internal error"
    );
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "error": {
                "code": "INTERNAL",
                "message": "database error",
                "correlation_id": correlation_id,
            }
        })),
    )
        .into_response()
}

fn code_label(code: StatusCode) -> &'static str {
    match code.as_u16() {
        400 => "BAD_REQUEST",
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        405 => "METHOD_NOT_ALLOWED",
        409 => "CONFLICT",
        412 => "PRECONDITION_FAILED",
        422 => "UNPROCESSABLE",
        503 => "SERVICE_UNAVAILABLE",
        _ => "INTERNAL",
    }
}

fn find_table<'a>(schema: &'a DatabaseSchema, name: &str) -> Option<&'a TableDefinition> {
    schema.tables.iter().find(|t| t.name == name)
}

fn parse_orderby(s: Option<&str>) -> Vec<Value> {
    let Some(s) = s else { return vec![] };
    s.split(',')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let mut parts = item.split_whitespace();
            let col = parts.next()?;
            let direction = match parts.next().map(str::to_ascii_lowercase).as_deref() {
                Some("desc") => "desc",
                _ => "asc",
            };
            Some(json!({"column": col, "direction": direction}))
        })
        .collect()
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

fn parse_if_match(headers: &HeaderMap) -> Result<i32, Response> {
    let raw = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            error_resp(
                StatusCode::BAD_REQUEST,
                "If-Match header is required (integer version)",
            )
        })?;
    let trimmed = raw.trim().trim_matches('"');
    trimmed.parse::<i32>().map_err(|_| {
        error_resp(
            StatusCode::BAD_REQUEST,
            "If-Match header must be an integer version",
        )
    })
}

fn parse_id_value(id: String) -> Value {
    // The id path segment can be int (i64) or uuid/string. We let the
    // CRUD builder cast via the table's `id_cast` so we don't have to
    // know the type here — just pass JSON.
    if let Ok(n) = id.parse::<i64>() {
        Value::Number(n.into())
    } else {
        Value::String(id)
    }
}

/// Best-effort audit insert. Logged on failure but does not propagate
/// the error — at the gateway boundary, an audit-only failure must not
/// roll back a successful data mutation.
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

    let args = match hr_dataverse::dv_io::bind_all(&compiled.params) {
        Ok(a) => a,
        Err(e) => {
            warn!(table, ?op, error = %e, "audit bind failed — skipping audit row");
            return;
        }
    };
    if let Err(e) = sqlx_core::query::query_with(&compiled.sql, args)
        .execute(pool)
        .await
    {
        warn!(table, ?op, error = %e, "audit insert failed — proceeding");
    }
}

// ── Schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct RepairParams {
    /// When false, sync_schema performs active cleanup (drops orphaned
    /// metadata, recreates missing triggers). Defaults to `true` (report
    /// only) so accidental hits don't mutate state.
    #[serde(default = "default_true", rename = "dry_run")]
    dry_run: bool,
}

fn default_true() -> bool { true }

/// POST /{slug}/$repair?dry_run=true
///
/// Surface for the engine-level drift detection (`sync_schema`). Listed
/// in the audit as D3: lets an operator inspect / fix orphans left by a
/// partial DDL mutation (cf. P0 #1 in the audit).
///
/// Requires the same auth as the schema endpoint — only users (not apps)
/// should call this, but the policy split lives in the gateway, not here.
async fn repair_schema(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(params): Query<RepairParams>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    let identity = match extract_identity(&headers, &state, &slug).await {
        Ok(id) => id,
        Err(r) => return r,
    };
    // App tokens are scoped to their own data — repair is an admin op.
    if matches!(identity, Identity::App { .. }) {
        return error_resp(
            StatusCode::FORBIDDEN,
            "repair requires user identity, not app token",
        );
    }
    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_repair", e),
    };
    match engine.sync_schema(params.dry_run).await {
        Ok(report) => {
            tracing::info!(slug = %slug, dry_run = params.dry_run, "dv_repair completed");
            (StatusCode::OK, Json(serde_json::to_value(report).unwrap_or(Value::Null)))
                .into_response()
        }
        Err(e) => db_error_resp("dv_repair", e),
    }
}

async fn get_schema(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = extract_identity(&headers, &state, &slug).await {
        return r;
    }
    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    match engine.get_schema().await {
        Ok(schema) => Json(serde_json::to_value(schema).unwrap_or(Value::Null)).into_response(),
        Err(e) => db_error_resp("dv_schema", e),
    }
}

// ── List ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct ListParams {
    #[serde(default, rename = "$filter")]
    filter: Option<String>,
    #[serde(default, rename = "$select")]
    select: Option<String>,
    #[serde(default, rename = "$orderby")]
    orderby: Option<String>,
    #[serde(default, rename = "$top")]
    top: Option<u32>,
    #[serde(default, rename = "$skip")]
    skip: Option<u32>,
    #[serde(default, rename = "$includeDeleted")]
    include_deleted: Option<bool>,
    #[serde(default, rename = "$count")]
    count: Option<bool>,
}

async fn list_rows(
    State(state): State<ApiState>,
    Path((slug, table)): Path<(String, String)>,
    Query(params): Query<ListParams>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table(&table) {
        return r;
    }
    let identity = match extract_identity(&headers, &state, &slug).await {
        Ok(i) => i,
        Err(r) => return r,
    };

    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return db_error_resp("dv_list", e),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("table '{}' not found", table),
            );
        }
    };

    let select: Vec<Value> = params
        .select
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|p| Value::String(p.trim().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let orderby = parse_orderby(params.orderby.as_deref());
    let mut q = serde_json::Map::new();
    if let Some(f) = params.filter {
        q.insert("filter".into(), Value::String(f));
    }
    q.insert("select".into(), Value::Array(select));
    q.insert("orderby".into(), Value::Array(orderby));
    if let Some(t) = params.top {
        q.insert("top".into(), Value::Number(t.into()));
    }
    if let Some(s) = params.skip {
        q.insert("skip".into(), Value::Number(s.into()));
    }
    q.insert(
        "include_deleted".into(),
        Value::Bool(params.include_deleted.unwrap_or(false)),
    );
    q.insert("count".into(), Value::Bool(params.count.unwrap_or(false)));
    let lq: ListQuery = match serde_json::from_value(Value::Object(q)) {
        Ok(v) => v,
        Err(e) => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("invalid $query: {e}"),
            );
        }
    };

    let compiled = match build_list_sql(table_def, &lq, &identity) {
        Ok(c) => c,
        Err(e) => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("$filter: {e}"),
            );
        }
    };
    let rows = match run_list(engine.pool(), table_def, &compiled).await {
        Ok(r) => r,
        Err(e) => {
            error!(?e, "dv_list run failed");
            return db_error_resp("dv_list", e);
        }
    };

    let mut envelope = serde_json::Map::new();
    envelope.insert("value".into(), Value::Array(rows));

    if lq.count {
        match build_count_sql(table_def, &lq, &identity) {
            Ok((sql, params)) => match run_count(engine.pool(), &sql, &params).await {
                Ok(n) => {
                    envelope.insert("@count".into(), json!(n));
                }
                Err(e) => {
                    return db_error_resp("dv_count", e);
                }
            },
            Err(e) => {
                return error_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("$count: {e}"),
                );
            }
        }
    }

    info!(slug = %slug, table = %table, "DvList ok");
    Json(Value::Object(envelope)).into_response()
}

// ── Get single ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct GetParams {
    #[serde(default, rename = "$includeDeleted")]
    include_deleted: Option<bool>,
}

async fn get_row(
    State(state): State<ApiState>,
    Path((slug, table, id)): Path<(String, String, String)>,
    Query(params): Query<GetParams>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table(&table) {
        return r;
    }
    if let Err(r) = extract_identity(&headers, &state, &slug).await {
        return r;
    }

    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return db_error_resp("dv_get", e),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => {
            return error_resp(
                StatusCode::NOT_FOUND,
                &format!("table '{}' not found", table),
            );
        }
    };

    let id_value = parse_id_value(id);
    let compiled = build_get(table_def, &id_value, params.include_deleted.unwrap_or(false));
    match run_get(engine.pool(), table_def, &compiled.sql, &compiled.params).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => error_resp(StatusCode::NOT_FOUND, "not found"),
        Err(e) => db_error_resp("dv_get", e),
    }
}

// ── Insert ─────────────────────────────────────────────────────────────

async fn insert_row(
    State(state): State<ApiState>,
    Path((slug, table)): Path<(String, String)>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table(&table) {
        return r;
    }
    let identity = match extract_identity(&headers, &state, &slug).await {
        Ok(i) => i,
        Err(r) => return r,
    };

    let payload_map: BTreeMap<String, Value> = match payload {
        Value::Object(o) => o.into_iter().collect(),
        _ => {
            return error_resp(
                StatusCode::BAD_REQUEST,
                "insert payload must be a JSON object",
            );
        }
    };

    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return db_error_resp("dv_insert", e),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("table '{}' not found", table),
            );
        }
    };

    let mutation = match build_insert(table_def, &payload_map, &identity) {
        Ok(m) => m,
        Err(e) => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("dv_insert: {e}"),
            );
        }
    };

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
            (StatusCode::CREATED, Json(row)).into_response()
        }
        Ok(other) => {
            tracing::error!(slug = %slug, table = %table, ?other, "dv_insert unexpected outcome");
            db_error_resp("dv_insert_unexpected", format!("{:?}", other))
        }
        Err(e) => db_error_resp("dv_insert", e),
    }
}

// ── Update ─────────────────────────────────────────────────────────────

async fn update_row(
    State(state): State<ApiState>,
    Path((slug, table, id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table(&table) {
        return r;
    }
    let identity = match extract_identity(&headers, &state, &slug).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let if_version = match parse_if_match(&headers) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let payload_map: BTreeMap<String, Value> = match payload {
        Value::Object(o) => o.into_iter().collect(),
        _ => {
            return error_resp(
                StatusCode::BAD_REQUEST,
                "update payload must be a JSON object",
            );
        }
    };

    let id_value = parse_id_value(id);

    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return db_error_resp("dv_update", e),
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("table '{}' not found", table),
            );
        }
    };

    // Snapshot before — for audit diff.
    let before = run_get(
        engine.pool(),
        table_def,
        &build_get(table_def, &id_value, true).sql,
        &[json_to_query_param(&id_value)],
    )
    .await
    .ok()
    .flatten();

    let mutation = match build_update(table_def, &id_value, if_version, &payload_map, &identity) {
        Ok(m) => m,
        Err(e) => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("dv_update: {e}"),
            );
        }
    };

    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &id_value,
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
            Json(row).into_response()
        }
        Ok(MutationOutcome::PreconditionFailed) => {
            error_resp(StatusCode::PRECONDITION_FAILED, "precondition_failed")
        }
        Ok(MutationOutcome::NotFound) => error_resp(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => db_error_resp("dv_update", e),
    }
}

// ── Soft-delete ────────────────────────────────────────────────────────

async fn soft_delete_row(
    State(state): State<ApiState>,
    Path((slug, table, id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table(&table) {
        return r;
    }
    let identity = match extract_identity(&headers, &state, &slug).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let if_version = match parse_if_match(&headers) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let id_value = parse_id_value(id);

    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => {
            return db_error_resp("dv_soft_delete_schema", e);
        }
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("table '{}' not found", table),
            );
        }
    };

    let mutation = match build_soft_delete(table_def, &id_value, if_version, &identity) {
        Ok(m) => m,
        Err(e) => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("dv_soft_delete: {e}"),
            );
        }
    };

    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &id_value,
    )
    .await
    {
        Ok(MutationOutcome::Applied(row)) => {
            audit_after(engine.pool(), &table, &row, AuditOp::Delete, &identity, None).await;
            info!(slug = %slug, table = %table, "DvSoftDelete ok");
            Json(row).into_response()
        }
        Ok(MutationOutcome::PreconditionFailed) => {
            error_resp(StatusCode::PRECONDITION_FAILED, "precondition_failed")
        }
        Ok(MutationOutcome::NotFound) => error_resp(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => db_error_resp("dv_soft_delete", e),
    }
}

// ── Restore ────────────────────────────────────────────────────────────

async fn restore_row(
    State(state): State<ApiState>,
    Path((slug, table, id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table(&table) {
        return r;
    }
    let identity = match extract_identity(&headers, &state, &slug).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let if_version = match parse_if_match(&headers) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let id_value = parse_id_value(id);

    let dv = state.dv.as_ref().expect("dv manager checked above");
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => return db_error_resp("dv_internal", e),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => {
            return db_error_resp("dv_restore_schema", e);
        }
    };
    let table_def = match find_table(&schema, &table) {
        Some(t) => t,
        None => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("table '{}' not found", table),
            );
        }
    };

    let mutation = match build_restore(table_def, &id_value, if_version, &identity) {
        Ok(m) => m,
        Err(e) => {
            return error_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("dv_restore: {e}"),
            );
        }
    };

    match run_mutation(
        engine.pool(),
        table_def,
        &mutation.sql,
        &mutation.params,
        None,
        &[],
        &id_value,
    )
    .await
    {
        Ok(MutationOutcome::Applied(row)) => {
            audit_after(engine.pool(), &table, &row, AuditOp::Restore, &identity, None).await;
            info!(slug = %slug, table = %table, "DvRestore ok");
            Json(row).into_response()
        }
        Ok(MutationOutcome::PreconditionFailed) => {
            error_resp(StatusCode::PRECONDITION_FAILED, "precondition_failed")
        }
        Ok(MutationOutcome::NotFound) => error_resp(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => db_error_resp("dv_restore", e),
    }
}

// Silence unused import warnings — `Map` is part of the JSON ergonomics
// we may need later (e.g. for typed envelope building). Same for the
// soft-delete restore module dependency chain.
#[allow(dead_code)]
fn _ergonomics(_: Map<String, Value>) {}
