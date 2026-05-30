//! Apps DB routes — the schema/data surface the Studio + DbExplorer use.
//!
//! Toutes les apps tournent sur `postgres-dataverse` : on délègue au DV engine
//! déjà initialisé dans `ApiState.dv`. Les schémas, la liste des tables et les
//! requêtes sont les mêmes que `/api/dv/...` mais exposés sous le nom hr-api
//! attendu par le frontend.
//!
//! Lecture : `db/schema`, `db/tables`, `db/tables/{t}`, `db/tables/{t}/rows`.
//! Écriture (admin, identité `system`, verrou optimiste géré côté serveur) :
//! `POST db/tables/{t}/insert`, `PATCH/DELETE db/tables/{t}/rows/{id}`. Le SQL
//! brut n'existe plus (postgres-dataverse) — ces endpoints sont la voie admin.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use atelier_common::Identity;
use atelier_dataverse::{
    TableDefinition,
    audit::AuditOp,
    crud::{build_get, build_insert, build_soft_delete, build_update},
    dv_io::{MutationOutcome, run_get, run_list, run_mutation},
    query::{ListQuery, build_list_sql},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tracing::{info, warn};
use uuid::Uuid;

use crate::routes::dv::{audit_after, parse_id_value};
use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/db/schema", get(get_schema))
        .route("/{slug}/db/tables", get(list_tables))
        .route("/{slug}/db/tables/{table}", get(describe_table))
        .route("/{slug}/db/tables/{table}/rows", post(query_rows))
        .route("/{slug}/db/tables/{table}/insert", post(insert_row))
        .route(
            "/{slug}/db/tables/{table}/rows/{id}",
            patch(update_row).delete(delete_row),
        )
}

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
        Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "invalid slug"})),
        )
            .into_response())
    }
}

fn validate_table_name(table: &str) -> Result<(), Response> {
    let ok = !table.is_empty()
        && table.len() <= 64
        && table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"success": false, "error": "invalid table name"})),
        )
            .into_response())
    }
}

async fn db_backend_for(state: &ApiState, slug: &str) -> Option<String> {
    let app = state.app_registry.get(slug).await?;
    Some(format!("{:?}", app.db_backend).to_lowercase().replace('_', "-"))
}

fn legacy_sqlite_response(slug: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "success": false,
            "error": format!(
                "App '{slug}' utilise le backend legacy SQLite. \
                 Atelier expose uniquement les apps postgres-dataverse en read-only. \
                 Pour explorer ce backend, ouvre proxy.mynetwk.biz/database?app={slug}."
            ),
            "backend": "legacy-sqlite",
        })),
    )
        .into_response()
}

async fn require_pg(state: &ApiState, slug: &str) -> Result<(), Response> {
    match db_backend_for(state, slug).await.as_deref() {
        Some("postgres-dataverse") | Some("postgresdataverse") => Ok(()),
        Some(_) | None => Err(legacy_sqlite_response(slug)),
    }
}

fn ok_data<T: serde::Serialize>(data: T) -> Response {
    Json(json!({"success": true, "data": data})).into_response()
}

fn err_response(code: StatusCode, msg: impl Into<String>) -> Response {
    (
        code,
        Json(json!({"success": false, "error": msg.into()})),
    )
        .into_response()
}

/// GET /api/apps/{slug}/db/schema
async fn get_schema(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let dv = match state.dv.as_ref() {
        Some(m) => m,
        None => {
            return err_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "dataverse manager not initialised",
            );
        }
    };
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("dataverse engine: {e}"),
            );
        }
    };
    match engine.get_schema().await {
        Ok(schema) => {
            info!(
                slug = %slug,
                tables = schema.tables.len(),
                relations = schema.relations.len(),
                "AppDb get_schema ok"
            );
            ok_data(schema)
        }
        Err(e) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("get_schema: {e}"),
        ),
    }
}

/// GET /api/apps/{slug}/db/tables
async fn list_tables(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let dv = match state.dv.as_ref() {
        Some(m) => m,
        None => {
            return err_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "dataverse manager not initialised",
            );
        }
    };
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("dataverse engine: {e}"),
            );
        }
    };
    match engine.list_tables().await {
        Ok(tables) => {
            info!(slug = %slug, count = tables.len(), "AppDb list_tables ok");
            ok_data(json!({"tables": tables}))
        }
        Err(e) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("list_tables: {e}"),
        ),
    }
}

/// GET /api/apps/{slug}/db/tables/{table}
async fn describe_table(
    State(state): State<ApiState>,
    Path((slug, table)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table_name(&table) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let dv = match state.dv.as_ref() {
        Some(m) => m,
        None => {
            return err_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "dataverse manager not initialised",
            );
        }
    };
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("dataverse engine: {e}"),
            );
        }
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("get_schema: {e}"),
            );
        }
    };
    let Some(t) = schema.tables.iter().find(|x| x.name == table) else {
        return err_response(StatusCode::NOT_FOUND, format!("table '{table}' not found"));
    };
    let row_count = engine.count_rows(&table).await.unwrap_or(0) as u64;
    // Mirror exact serialization from atelier_ipc::types::AppDbTableColumn:
    // skip choices when empty, skip formula_expression when None.
    let columns: Vec<Value> = t
        .columns
        .iter()
        .map(|c| {
            let mut obj = serde_json::Map::new();
            obj.insert("name".into(), json!(c.name));
            obj.insert("field_type".into(), json!(format!("{:?}", c.field_type)));
            obj.insert("required".into(), json!(c.required));
            obj.insert("unique".into(), json!(c.unique));
            if !c.choices.is_empty() {
                obj.insert("choices".into(), json!(c.choices));
            }
            if c.formula_expression.is_some() {
                obj.insert("formula_expression".into(), json!(c.formula_expression));
            }
            Value::Object(obj)
        })
        .collect();
    let relations: Vec<Value> = schema
        .relations
        .iter()
        .filter(|r| r.from_table == table)
        .map(|r| {
            json!({
                "from_column": r.from_column,
                "to_table": r.to_table,
                "to_column": r.to_column,
                "display_column": "id",
            })
        })
        .collect();
    let mut data = serde_json::Map::new();
    data.insert("name".into(), json!(t.name));
    data.insert("columns".into(), json!(columns));
    if !relations.is_empty() {
        data.insert("relations".into(), json!(relations));
    }
    data.insert("row_count".into(), json!(row_count));
    ok_data(Value::Object(data))
}

#[derive(Debug, Deserialize)]
struct LegacyFilter {
    column: String,
    op: String,
    value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct QueryRowsBody {
    #[serde(default)]
    filters: Vec<LegacyFilter>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    order_by: Option<String>,
    #[serde(default)]
    order_desc: Option<bool>,
    #[serde(default)]
    expand: Vec<String>,
}

fn quote_dvexpr_string(s: &str) -> String {
    // Encadre une chaîne pour dvexpr en doublant les apostrophes.
    format!("'{}'", s.replace('\'', "''"))
}

fn legacy_filter_to_dvexpr(f: &LegacyFilter) -> Option<String> {
    let col = &f.column;
    let val_str = || match &f.value {
        Some(Value::Null) | None => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => quote_dvexpr_string(s),
        Some(other) => quote_dvexpr_string(&other.to_string()),
    };
    let expr = match f.op.as_str() {
        "eq" => format!("{col} == {}", val_str()),
        "neq" | "ne" => format!("{col} != {}", val_str()),
        "gt" => format!("{col} > {}", val_str()),
        "gte" | "ge" => format!("{col} >= {}", val_str()),
        "lt" => format!("{col} < {}", val_str()),
        "lte" | "le" => format!("{col} <= {}", val_str()),
        // dvexpr ne supporte pas LIKE en l'état; on traduit en `contains` si dispo,
        // sinon en eq stricte (best-effort — la plupart des usages côté DbExplorer
        // sont la barre de recherche qui utilise `like '%foo%'`).
        "like" => {
            if let Some(Value::String(s)) = &f.value {
                let trimmed = s.trim_matches('%');
                format!("contains({col}, {})", quote_dvexpr_string(trimmed))
            } else {
                return None;
            }
        }
        "is_null" => format!("{col} == null"),
        "is_not_null" | "not_null" => format!("{col} != null"),
        _ => return None,
    };
    Some(expr)
}

/// POST /api/apps/{slug}/db/tables/{table}/rows
async fn query_rows(
    State(state): State<ApiState>,
    Path((slug, table)): Path<(String, String)>,
    Json(body): Json<QueryRowsBody>,
) -> impl IntoResponse {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table_name(&table) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let dv = match state.dv.as_ref() {
        Some(m) => m,
        None => {
            return err_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "dataverse manager not initialised",
            );
        }
    };
    let engine = match dv.engine_for(&slug).await {
        Ok(e) => e,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("dataverse engine: {e}"),
            );
        }
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => {
            return err_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("get_schema: {e}"),
            );
        }
    };
    let table_def: &TableDefinition =
        match schema.tables.iter().find(|x| x.name == table) {
            Some(t) => t,
            None => {
                return err_response(
                    StatusCode::NOT_FOUND,
                    format!("table '{table}' not found"),
                );
            }
        };

    // Build dvexpr filter string (combine with &&)
    let filter_parts: Vec<String> = body
        .filters
        .iter()
        .filter_map(legacy_filter_to_dvexpr)
        .collect();
    let filter = if filter_parts.is_empty() {
        None
    } else {
        Some(filter_parts.join(" && "))
    };

    // OrderBy
    let orderby = match &body.order_by {
        Some(col) if !col.is_empty() => vec![json!({
            "column": col,
            "direction": if body.order_desc.unwrap_or(false) { "desc" } else { "asc" },
        })],
        _ => vec![],
    };

    // Build ListQuery for both list + total
    let mut q_obj = serde_json::Map::new();
    if let Some(f) = filter.clone() {
        q_obj.insert("filter".into(), Value::String(f));
    }
    q_obj.insert("select".into(), Value::Array(vec![]));
    q_obj.insert("orderby".into(), Value::Array(orderby));
    if let Some(t) = body.limit {
        q_obj.insert("top".into(), Value::Number(t.into()));
    }
    if let Some(s) = body.offset {
        q_obj.insert("skip".into(), Value::Number(s.into()));
    }
    q_obj.insert("include_deleted".into(), Value::Bool(false));
    q_obj.insert("count".into(), Value::Bool(true));
    let lq: ListQuery = match serde_json::from_value(Value::Object(q_obj)) {
        Ok(v) => v,
        Err(e) => {
            return err_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("query: {e}"),
            );
        }
    };

    // Atelier read-only: pas d'identité utilisateur — on injecte un Identity::system()
    // pour passer les checks de atelier-dataverse. Les apps existantes sur Medion sont
    // toujours servies avec une vraie identité — c'est juste l'exploration côté
    // Atelier qui passe en mode système.
    let identity = Identity::system();

    let compiled = match build_list_sql(table_def, &lq, &identity) {
        Ok(c) => c,
        Err(e) => {
            return err_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("$filter: {e}"),
            );
        }
    };
    let raw_rows = match run_list(engine.pool(), table_def, &compiled).await {
        Ok(r) => r,
        Err(e) => {
            warn!(slug = %slug, table = %table, ?e, "AppDbQueryRows failed");
            return err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}"));
        }
    };

    // Total via count query
    let total = match atelier_dataverse::query::build_count_sql(table_def, &lq, &identity) {
        Ok((sql, params)) => atelier_dataverse::dv_io::run_count(engine.pool(), &sql, &params)
            .await
            .unwrap_or(0),
        Err(_) => 0,
    };

    // Strip system bookkeeping columns from rows (DbExplorer doesn't render
    // them — homeroute does the same filtering before sending the wire response).
    const HIDDEN: &[&str] = &["is_deleted", "version"];

    let mut col_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let cleaned_rows: Vec<Value> = raw_rows
        .into_iter()
        .map(|r| {
            if let Value::Object(map) = r {
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    if !HIDDEN.contains(&k.as_str()) {
                        col_set.insert(k.clone());
                        new_map.insert(k, v);
                    }
                }
                Value::Object(new_map)
            } else {
                r
            }
        })
        .collect();
    let columns: Vec<String> = col_set.into_iter().collect();

    let _ = body.expand; // expand non-géré pour l'instant
    let _ = Uuid::nil(); // keep uuid dep

    info!(
        slug = %slug,
        table = %table,
        rows = cleaned_rows.len(),
        total,
        "AppDb query_rows ok"
    );
    ok_data(json!({
        "columns": columns,
        "rows": cleaned_rows,
        "total": total,
    }))
}

// ── Admin writes (postgres-dataverse via DV engine) ─────────────────────
//
// The DbExplorer is an admin tool: it acts as `Identity::system()` and the
// optimistic-lock version is read server-side, so the browser never has to
// track row versions. Mirrors the gateway write path in `routes::dv`.

async fn engine_and_schema(
    state: &ApiState,
    slug: &str,
) -> Result<
    (
        std::sync::Arc<atelier_dataverse::DataverseEngine>,
        atelier_dataverse::DatabaseSchema,
    ),
    Response,
> {
    let dv = state.dv.as_ref().ok_or_else(|| {
        err_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "dataverse manager not initialised",
        )
    })?;
    let engine = dv.engine_for(slug).await.map_err(|e| {
        err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("dataverse engine: {e}"))
    })?;
    let schema = engine
        .get_schema()
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("get_schema: {e}")))?;
    Ok((engine, schema))
}

/// Read the current optimistic-lock version of a live row, server-side.
async fn current_version(
    engine: &atelier_dataverse::DataverseEngine,
    table_def: &TableDefinition,
    id_value: &Value,
) -> Result<i32, Response> {
    let get = build_get(table_def, id_value, false);
    match run_get(engine.pool(), table_def, &get.sql, &get.params).await {
        Ok(Some(row)) => row
            .get("version")
            .and_then(|v| v.as_i64())
            .map(|n| n as i32)
            .ok_or_else(|| err_response(StatusCode::INTERNAL_SERVER_ERROR, "row missing version")),
        Ok(None) => Err(err_response(StatusCode::NOT_FOUND, "row not found")),
        Err(e) => Err(err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("get: {e}"))),
    }
}

/// POST /api/apps/{slug}/db/tables/{table}/insert
async fn insert_row(
    State(state): State<ApiState>,
    Path((slug, table)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table_name(&table) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let Value::Object(obj) = payload else {
        return err_response(StatusCode::BAD_REQUEST, "insert payload must be a JSON object");
    };
    let payload_map: BTreeMap<String, Value> = obj.into_iter().collect();
    let (engine, schema) = match engine_and_schema(&state, &slug).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(table_def) = schema.tables.iter().find(|t| t.name == table) else {
        return err_response(StatusCode::NOT_FOUND, format!("table '{table}' not found"));
    };
    let identity = Identity::system();
    let mutation = match build_insert(table_def, &payload_map, &identity) {
        Ok(m) => m,
        Err(e) => return err_response(StatusCode::UNPROCESSABLE_ENTITY, format!("insert: {e}")),
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
            info!(slug = %slug, table = %table, "AppDb insert ok");
            ok_data(row)
        }
        Ok(other) => err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("insert: unexpected {other:?}"),
        ),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("insert: {e}")),
    }
}

/// PATCH /api/apps/{slug}/db/tables/{table}/rows/{id}
async fn update_row(
    State(state): State<ApiState>,
    Path((slug, table, id)): Path<(String, String, String)>,
    Json(payload): Json<Value>,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table_name(&table) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let Value::Object(obj) = payload else {
        return err_response(StatusCode::BAD_REQUEST, "update payload must be a JSON object");
    };
    let payload_map: BTreeMap<String, Value> = obj.into_iter().collect();
    let id_value = parse_id_value(id);
    let (engine, schema) = match engine_and_schema(&state, &slug).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(table_def) = schema.tables.iter().find(|t| t.name == table) else {
        return err_response(StatusCode::NOT_FOUND, format!("table '{table}' not found"));
    };
    let version = match current_version(&engine, table_def, &id_value).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let before = {
        let get = build_get(table_def, &id_value, true);
        run_get(engine.pool(), table_def, &get.sql, &get.params)
            .await
            .ok()
            .flatten()
    };
    let identity = Identity::system();
    let mutation = match build_update(table_def, &id_value, version, &payload_map, &identity) {
        Ok(m) => m,
        Err(e) => return err_response(StatusCode::UNPROCESSABLE_ENTITY, format!("update: {e}")),
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
            info!(slug = %slug, table = %table, "AppDb update ok");
            ok_data(row)
        }
        Ok(MutationOutcome::PreconditionFailed) => err_response(
            StatusCode::PRECONDITION_FAILED,
            "row changed concurrently — refresh and retry",
        ),
        Ok(MutationOutcome::NotFound) => err_response(StatusCode::NOT_FOUND, "row not found"),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("update: {e}")),
    }
}

/// DELETE /api/apps/{slug}/db/tables/{table}/rows/{id} (soft-delete)
async fn delete_row(
    State(state): State<ApiState>,
    Path((slug, table, id)): Path<(String, String, String)>,
) -> Response {
    if let Err(r) = validate_slug(&slug) {
        return r;
    }
    if let Err(r) = validate_table_name(&table) {
        return r;
    }
    if let Err(r) = require_pg(&state, &slug).await {
        return r;
    }
    let id_value = parse_id_value(id);
    let (engine, schema) = match engine_and_schema(&state, &slug).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(table_def) = schema.tables.iter().find(|t| t.name == table) else {
        return err_response(StatusCode::NOT_FOUND, format!("table '{table}' not found"));
    };
    let version = match current_version(&engine, table_def, &id_value).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let identity = Identity::system();
    let mutation = match build_soft_delete(table_def, &id_value, version, &identity) {
        Ok(m) => m,
        Err(e) => return err_response(StatusCode::UNPROCESSABLE_ENTITY, format!("delete: {e}")),
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
            info!(slug = %slug, table = %table, "AppDb delete ok");
            ok_data(json!({ "deleted": true }))
        }
        Ok(MutationOutcome::PreconditionFailed) => err_response(
            StatusCode::PRECONDITION_FAILED,
            "row changed concurrently — refresh and retry",
        ),
        Ok(MutationOutcome::NotFound) => err_response(StatusCode::NOT_FOUND, "row not found"),
        Err(e) => err_response(StatusCode::INTERNAL_SERVER_ERROR, format!("delete: {e}")),
    }
}
