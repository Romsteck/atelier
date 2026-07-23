//! Canal de remontée des frictions **plateforme** (surface HTTP historique).
//!
//! Les chats Claude Code des apps (Studio) rencontrent parfois des soucis qui
//! relèvent d'Atelier (tool MCP qui bug/manque, doc trompeuse, build/deploy/
//! dataverse/agent qui déraille à cause de la plateforme). Plutôt que de
//! contourner en silence, ils appellent cet endpoint (via la skill
//! `0-report-issue`, qui passe en pratique par le tool MCP `issue_report`).
//!
//! Depuis 2026-07-23 une remontée n'est PLUS stockée telle quelle : elle est
//! **enfilée pour triage** ([`atelier_pilot::PilotService::report_issue`]) — une
//! instance headless du chef de projet l'investigue et en fait un item de
//! backlog planifié. Les endpoints de liste/triage (`GET/PATCH/DELETE
//! /api/issues`) ont donc disparu : le suivi se fait dans le Pilote.
//!
//! Endpoint (non authentifié, confiance LAN comme les siblings
//! `build-event`/`ship`) :
//!   POST /api/apps/{slug}/issues  {title, kind?, area?, severity?, context?, tried?}

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, instrument};

use crate::state::ApiState;

/// Router per-app : uniquement le POST de signalement (slug dans l'URL). Monté
/// sous `/api/apps` — c'est ce que la skill générée `0-report-issue` appelle.
pub fn app_router() -> Router<ApiState> {
    Router::new().route("/{slug}/issues", post(post_issue))
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

/// `POST /api/apps/{slug}/issues` — enfile une remontée pour triage. Seul
/// `title` est requis ; le slug (identité de l'app émettrice) vient de l'URL.
/// Renvoie `{queued, triage_id}`. La forme diffère de l'ancien retour (entrée
/// `iss-…`) mais aucun appelant ne lisait `data.id` (skill = tool MCP).
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
    let payload = atelier_pilot::TriagePayload {
        title: b.title.unwrap_or_default().trim().to_string(),
        kind: b.kind.unwrap_or_else(|| "error".into()),
        area: b.area.unwrap_or_else(|| "other".into()),
        severity: b.severity.unwrap_or_else(|| "medium".into()),
        context: b.context.unwrap_or_default(),
        tried: b.tried.unwrap_or_default(),
    };
    if payload.title.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "title requis");
    }
    match state.pilot.report_issue(&slug, payload).await {
        Ok(triage_id) => {
            info!(slug = %slug, triage_id, "AppIssueReport → triage enqueued");
            ok(json!({"queued": true, "triage_id": triage_id}))
        }
        Err(e) => fail(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
