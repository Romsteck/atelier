//! Dataverse Gateway read-only routes (Phase 7).
//!
//! Atelier connecte en LAN aux mêmes Postgres apps que homeroute (10.0.0.254:5432)
//! via les credentials synchronisés depuis `dataverse-secrets.json` (sync-state
//! toutes les 2 min, 0600 sur disque).
//!
//! Mutations restent côté homeroute (Medion) — les écritures passent par le
//! gateway de proxy.mynetwk.biz qui possède le `DataverseManager` complet
//! (audit, write triggers, schema-ops). Atelier expose uniquement :
//! - GET /dv/{slug}/$schema
//! - GET /dv/{slug}/{table}    (OData $filter/$select/$orderby/$top/$skip/$count)
//! - GET /dv/{slug}/{table}/{id}

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use hr_common::Identity;
use hr_dataverse::{
    DatabaseSchema, TableDefinition,
    crud::build_get,
    dv_io::{run_count, run_get, run_list},
    query::{ListQuery, build_count_sql, build_list_sql},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{error, info};
use uuid::Uuid;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/$schema", get(get_schema))
        .route("/{slug}/{table}", get(list_rows))
        .route("/{slug}/{table}/{id}", get(get_row))
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

fn code_label(code: StatusCode) -> &'static str {
    match code.as_u16() {
        400 => "BAD_REQUEST",
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
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

// ── Schema ─────────────────────────────────────────────────────────────

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
        Err(e) => return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
    };
    match engine.get_schema().await {
        Ok(schema) => Json(serde_json::to_value(schema).unwrap_or(Value::Null)).into_response(),
        Err(e) => error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("dv_schema: {e}")),
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
        Err(e) => return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("dv_list: {e}")),
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
            return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("dv_list: {e}"));
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
                    return error_resp(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("dv_count: {e}"),
                    );
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
        Err(e) => return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("dv_get: {e}")),
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

    let id_value = Value::String(id);
    let compiled = build_get(table_def, &id_value, params.include_deleted.unwrap_or(false));
    match run_get(engine.pool(), table_def, &compiled.sql, &compiled.params).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => error_resp(StatusCode::NOT_FOUND, "not found"),
        Err(e) => error_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("dv_get: {e}")),
    }
}
