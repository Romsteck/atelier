//! Issues report routes — canal de remontée des frictions **plateforme**.
//!
//! Les chats Claude Code des apps (Studio) rencontrent parfois des soucis qui
//! relèvent d'Atelier (tool MCP qui bug/manque, doc trompeuse, build/deploy/
//! dataverse/agent qui déraille à cause de la plateforme). Plutôt que de
//! contourner en silence, ils appellent ces endpoints (via la skill
//! `0-report-issue`).
//!
//! WHY store centralisé : la feature concerne des bugs de la **plateforme**, pas
//! des apps. Le store vit donc dans le control-plane Postgres `atelier_meta`
//! (table `platform_issues`, cf. [`atelier_common::issue_store`]) — **plus** dans
//! l'arbre source de chaque app (l'ancien `CLAUDE_ISSUES.json` per-app a été
//! rapatrié puis supprimé). Romain consomme/triage en session dev Atelier (skill
//! `/collect-issues`).
//!
//! Endpoints (non authentifiés, confiance LAN comme les siblings
//! `build-event`/`ship`) :
//!   POST   /api/apps/{slug}/issues   {title, kind?, area?, severity?, context?, tried?}  — report (slug dans l'URL)
//!   GET    /api/issues               ?status=open|resolved|dismissed&app=<slug>   — liste agrégée (dev)
//!   PATCH  /api/issues/{id}          {status?, note?}                             — triage (id global)
//!   DELETE /api/issues/{id}                                                       — purge

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, instrument, warn};

use crate::state::ApiState;

/// Router per-app : uniquement le POST de signalement (slug dans l'URL). Monté
/// sous `/api/apps` — c'est ce que la skill générée `0-report-issue` appelle.
pub fn app_router() -> Router<ApiState> {
    Router::new().route("/{slug}/issues", post(post_issue))
}

/// Router platform-level : liste/triage/purge agrégés. L'`id` est globalement
/// unique → plus besoin du slug. Monté sous `/api/issues`.
pub fn platform_router() -> Router<ApiState> {
    Router::new()
        .route("/", get(list_issues))
        .route("/{id}", patch(patch_issue).delete(delete_issue))
}

fn ok(data: Value) -> axum::response::Response {
    Json(json!({"success": true, "data": data})).into_response()
}

fn fail(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"success": false, "error": msg.into()}))).into_response()
}

#[derive(Deserialize, Default)]
struct PostIssueBody {
    title: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    area: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    tried: Option<String>,
}

/// `POST /api/apps/{slug}/issues` — ajoute une remontée. Le serveur estampe
/// `id`/`ts`/`status:open` ; seul `title` est requis. La colonne `slug` provient
/// de l'URL (identité de l'app émettrice).
#[instrument(skip_all)]
async fn post_issue(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    body: Option<Json<PostIssueBody>>,
) -> impl IntoResponse {
    if !atelier_apps::valid_slug(&slug) {
        return fail(StatusCode::BAD_REQUEST, "slug invalide");
    }
    let Json(b) = body.unwrap_or_default();
    let title = b.title.unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "title requis");
    }
    // Défauts appliqués ici pour le log ; le store reste l'autorité des enums
    // (coerce kind/area/severity vers leur défaut si valeur inconnue).
    let kind = b.kind.unwrap_or_else(|| "error".to_string());
    let area = b.area.unwrap_or_else(|| "other".to_string());
    let severity = b.severity.unwrap_or_else(|| "medium".to_string());
    let context = b.context.unwrap_or_default();
    let tried = b.tried.unwrap_or_default();

    match state
        .issues
        .insert(&slug, &kind, &area, &severity, &title, &context, &tried)
        .await
    {
        Ok(entry) => {
            let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
            info!(slug = %slug, id = %id, kind = %kind, area = %area, severity = %severity, "AppIssueReport");
            ok(entry)
        }
        Err(e) => {
            warn!(slug = %slug, err = %e, "platform_issues insert failed");
            crate::routes::internal_err("insert issue", e)
        }
    }
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    app: Option<String>,
}

/// `GET /api/issues` — liste agrégée toutes apps, filtres optionnels `?status=`
/// et `?app=`. Tri serveur (sévérité puis slug puis date).
#[instrument(skip_all)]
async fn list_issues(State(state): State<ApiState>, Query(q): Query<ListQuery>) -> impl IntoResponse {
    let data = state
        .issues
        .list(q.status.as_deref(), q.app.as_deref())
        .await;
    ok(json!(data))
}

#[derive(Deserialize, Default)]
struct PatchIssueBody {
    status: Option<String>,
    note: Option<String>,
}

/// `PATCH /api/issues/{id}` — met à jour le statut / ajoute une note (côté dev
/// Atelier pour marquer une remontée traitée).
#[instrument(skip_all)]
async fn patch_issue(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    body: Option<Json<PatchIssueBody>>,
) -> impl IntoResponse {
    let Json(b) = body.unwrap_or_default();
    match state
        .issues
        .update(&id, b.status.as_deref(), b.note.as_deref())
        .await
    {
        Ok(Some(entry)) => {
            info!(id = %id, "AppIssuePatch");
            ok(entry)
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "issue id introuvable"),
        Err(e) => {
            warn!(id = %id, err = %e, "platform_issues update failed");
            crate::routes::internal_err("update issue", e)
        }
    }
}

/// `DELETE /api/issues/{id}` — retire une remontée (purge après traitement).
/// 404 si l'id est absent.
#[instrument(skip_all)]
async fn delete_issue(State(state): State<ApiState>, Path(id): Path<String>) -> impl IntoResponse {
    match state.issues.delete(&id).await {
        Ok(true) => {
            info!(id = %id, "AppIssueDelete");
            ok(json!({"deleted": id}))
        }
        Ok(false) => fail(StatusCode::NOT_FOUND, "issue id introuvable"),
        Err(e) => {
            warn!(id = %id, err = %e, "platform_issues delete failed");
            crate::routes::internal_err("delete issue", e)
        }
    }
}
