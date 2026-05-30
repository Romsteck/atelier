//! Read-only REST routes for the v2 docs system (mirrored from homeroute hr-api).
//!
//! Mutations (create, update, delete, diagram_set) live exclusively on the MCP side — the
//! frontend is read-only by design. The agent is the sole writer.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use atelier_docs::{DocType, Store, validate_app_id, validate_entry_name};
use serde::Deserialize;
use serde_json::json;

use crate::state::ApiState;

fn store(state: &ApiState) -> Store {
    Store::new(state.docs_dir.clone())
}

fn parse_type(s: &str) -> Option<DocType> {
    DocType::from_str(s)
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_apps))
        .route("/search", get(search))
        .route("/{app_id}/overview", get(get_overview))
        .route("/{app_id}/entries", get(list_entries))
        .route("/{app_id}/completeness", get(completeness))
        .route("/{app_id}/{doc_type}/{name}", get(get_entry))
        .route("/{app_id}/{doc_type}/{name}/diagram", get(get_diagram))
}

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(json!({ "success": false, "error": msg })))
}

async fn list_apps(State(state): State<ApiState>) -> impl IntoResponse {
    let store = store(&state);
    let app_ids = match store.list_app_ids() {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    };
    let mut apps = Vec::with_capacity(app_ids.len());
    for app_id in app_ids {
        let Ok(meta) = store.read_meta(&app_id) else {
            continue;
        };
        let stats = store.overview(&app_id).map(|o| o.stats).unwrap_or_default();
        apps.push(json!({
            "app_id": app_id,
            "name": meta.name,
            "stack": meta.stack,
            "description": meta.description,
            "logo": meta.logo,
            "schema_version": meta.schema_version,
            "stats": stats,
            "has_overview": stats.has_overview,
        }));
    }
    Json(json!({ "success": true, "apps": apps })).into_response()
}

async fn get_overview(
    State(state): State<ApiState>,
    Path(app_id): Path<String>,
) -> impl IntoResponse {
    if !validate_app_id(&app_id) {
        return err(StatusCode::BAD_REQUEST, "Invalid app_id").into_response();
    }
    match store(&state).overview(&app_id) {
        Ok(ov) => Json(json!({ "success": true, "data": ov })).into_response(),
        Err(atelier_docs::StoreError::AppNotFound(_)) => {
            err(StatusCode::NOT_FOUND, "App not found").into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    }
}

#[derive(Deserialize)]
struct ListEntriesQuery {
    #[serde(default, rename = "type")]
    doc_type: Option<String>,
}

async fn list_entries(
    State(state): State<ApiState>,
    Path(app_id): Path<String>,
    Query(q): Query<ListEntriesQuery>,
) -> impl IntoResponse {
    if !validate_app_id(&app_id) {
        return err(StatusCode::BAD_REQUEST, "Invalid app_id").into_response();
    }
    let doc_type = match q.doc_type.as_deref() {
        None => None,
        Some(s) => match parse_type(s) {
            Some(t) => Some(t),
            None => {
                return err(StatusCode::BAD_REQUEST, &format!("Invalid type '{s}'"))
                    .into_response();
            }
        },
    };
    match store(&state).list_entries(&app_id, doc_type) {
        Ok(entries) => Json(json!({
            "success": true,
            "app_id": app_id,
            "entries": entries,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    }
}

async fn get_entry(
    State(state): State<ApiState>,
    Path((app_id, doc_type, name)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if !validate_app_id(&app_id) {
        return err(StatusCode::BAD_REQUEST, "Invalid app_id").into_response();
    }
    let Some(doc_type_e) = parse_type(&doc_type) else {
        return err(StatusCode::BAD_REQUEST, &format!("Invalid type '{doc_type}'"))
            .into_response();
    };
    if doc_type_e != DocType::Overview && !validate_entry_name(&name) {
        return err(StatusCode::BAD_REQUEST, "Invalid name").into_response();
    }
    let s = store(&state);
    match s.read_entry(&app_id, doc_type_e, &name) {
        Ok(entry) => {
            let diagram = s
                .read_diagram(&app_id, doc_type_e, &entry.name)
                .ok()
                .flatten();
            Json(json!({
                "success": true,
                "data": {
                    "app_id": entry.app_id,
                    "type": entry.doc_type.as_str(),
                    "name": entry.name,
                    "frontmatter": entry.frontmatter,
                    "body": entry.body,
                    "diagram": diagram,
                }
            }))
            .into_response()
        }
        Err(atelier_docs::StoreError::EntryNotFound { .. }) => {
            err(StatusCode::NOT_FOUND, "Entry not found").into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    }
}

async fn get_diagram(
    State(state): State<ApiState>,
    Path((app_id, doc_type, name)): Path<(String, String, String)>,
) -> impl IntoResponse {
    if !validate_app_id(&app_id) {
        return err(StatusCode::BAD_REQUEST, "Invalid app_id").into_response();
    }
    let Some(doc_type_e) = parse_type(&doc_type) else {
        return err(StatusCode::BAD_REQUEST, &format!("Invalid type '{doc_type}'"))
            .into_response();
    };
    match store(&state).read_diagram(&app_id, doc_type_e, &name) {
        Ok(opt) => Json(json!({
            "success": true,
            "app_id": app_id,
            "type": doc_type,
            "name": name,
            "mermaid": opt,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default, rename = "type")]
    doc_type: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

async fn search(State(state): State<ApiState>, Query(q): Query<SearchQuery>) -> impl IntoResponse {
    if let Some(ref a) = q.app_id {
        if !validate_app_id(a) {
            return err(StatusCode::BAD_REQUEST, "Invalid app_id").into_response();
        }
    }
    let doc_type = match q.doc_type.as_deref() {
        None => None,
        Some(s) => match parse_type(s) {
            Some(t) => Some(t),
            None => {
                return err(StatusCode::BAD_REQUEST, &format!("Invalid type '{s}'"))
                    .into_response();
            }
        },
    };
    let Some(idx) = state.docs_index.as_ref() else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "Docs index unavailable (init failed at boot)",
        )
        .into_response();
    };
    match idx.search(&q.q, q.app_id.as_deref(), doc_type, q.limit) {
        Ok(hits) => Json(json!({
            "success": true,
            "query": q.q,
            "count": hits.len(),
            "results": hits,
        }))
        .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    }
}

async fn completeness(
    State(state): State<ApiState>,
    Path(app_id): Path<String>,
) -> impl IntoResponse {
    if !validate_app_id(&app_id) {
        return err(StatusCode::BAD_REQUEST, "Invalid app_id").into_response();
    }
    let s = store(&state);
    let overview = match s.overview(&app_id) {
        Ok(o) => o,
        Err(atelier_docs::StoreError::AppNotFound(_)) => {
            return err(StatusCode::NOT_FOUND, "App not found").into_response();
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")).into_response(),
    };
    let mut missing_summaries: Vec<String> = Vec::new();
    let mut missing_diagrams: Vec<String> = Vec::new();
    for group in [
        &overview.index.screens,
        &overview.index.features,
        &overview.index.components,
    ] {
        for e in group {
            let key = format!("{}:{}", e.doc_type.as_str(), e.name);
            if e.summary
                .as_deref()
                .map(|x| x.trim().is_empty())
                .unwrap_or(true)
            {
                missing_summaries.push(key.clone());
            }
            if !e.has_diagram {
                missing_diagrams.push(key);
            }
        }
    }
    Json(json!({
        "success": true,
        "app_id": app_id,
        "has_overview": overview.stats.has_overview,
        "counts": {
            "screens": overview.stats.screens,
            "features": overview.stats.features,
            "components": overview.stats.components,
            "with_diagram": overview.stats.with_diagram,
        },
        "missing_summaries": missing_summaries,
        "missing_diagrams": missing_diagrams,
    }))
    .into_response()
}
