use std::net::SocketAddr;

use atelier_pilot::schedule::SchedulePatch;
use atelier_pilot::{AtelierWorkerReport, BacklogPatch, NewBacklogItem};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/state", get(state))
        .route("/backlog", get(list).post(create))
        .route("/backlog/{id}", get(get_one).patch(update).delete(remove))
        .route("/backlog/{id}/move", post(move_item))
        .route("/backlog/{id}/runs", get(item_runs))
        .route("/backlog/{id}/run", post(run_item))
        .route("/attention", get(attention))
        .route("/repos", get(repos))
        .route("/runs/{id}/transcript", get(transcript))
        .route("/runs/{id}/cancel", post(cancel_run))
        .route("/schedule", get(get_schedule).put(put_schedule))
        .route("/night", get(get_night).post(start_night))
        .route("/night/cancel", post(cancel_night))
        .route("/atelier-report", post(atelier_report))
}

fn ok(data: impl serde::Serialize) -> axum::response::Response {
    Json(json!({"success":true,"data":data})).into_response()
}
fn fail(status: StatusCode, message: impl Into<String>) -> axum::response::Response {
    (
        status,
        Json(json!({"success":false,"error":message.into()})),
    )
        .into_response()
}
fn stores(
    state: &ApiState,
) -> Result<(atelier_pilot::BacklogStore, atelier_pilot::RunsStore), axum::response::Response> {
    match (state.pilot.backlog(), state.pilot.runs()) {
        (Some(b), Some(r)) => Ok((b, r)),
        _ => Err(fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Pilote indisponible (atelier_meta)",
        )),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    scope: Option<String>,
    lane: Option<String>,
}

async fn state(State(state): State<ApiState>) -> impl IntoResponse {
    let claude = state.agent_auth.status().await;
    let codex = state.codex_auth.status().await;
    let codex_home = std::env::var("ATELIER_AGENT_CODEX_HOME")
        .unwrap_or_else(|_| "/var/lib/hr-studio/.codex".into());
    let codex_auth_file = std::path::Path::new(&codex_home)
        .join("auth.json")
        .is_file();
    let codex_runner = std::env::var("ATELIER_CODEX_RUNNER")
        .unwrap_or_else(|_| "/opt/atelier/runner/src/codex.js".into());
    let codex_binary = std::path::Path::new(&codex_runner)
        .parent()
        .and_then(std::path::Path::parent)
        .map(|p| {
            p.join(
                "node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex",
            )
            .is_file()
        })
        .unwrap_or(false);
    ok(json!({
        "enabled":state.pilot.is_enabled(),
        "busy":state.pilot.is_busy(),
        "engines":{
            "claude":claude.get("configured").and_then(Value::as_bool).unwrap_or(false),
            "codex":codex_binary && (codex_auth_file || codex.get("configured").and_then(Value::as_bool).unwrap_or(false)),
            "codex_worker":state.pilot.codex_worker_enabled(),
            "auto_router":state.pilot.codex_worker_enabled()
        }
    }))
}

async fn list(State(state): State<ApiState>, Query(q): Query<ListQuery>) -> impl IntoResponse {
    let Ok((b, _)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match b.list(q.scope.as_deref(), q.lane.as_deref()).await {
        Ok(v) => ok(v),
        Err(e) => crate::routes::internal_err("pilot list", e),
    }
}
async fn attention(State(state): State<ApiState>) -> impl IntoResponse {
    let Ok((b, _)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match b.list(None, Some("attention")).await {
        Ok(v) => ok(v),
        Err(e) => crate::routes::internal_err("pilot attention", e),
    }
}
/// État git agrégé des dépôts (apps + Atelier) — bande « État des dépôts »
/// du Backlog : fichiers en attente de commit, commits en attente de push.
async fn repos(State(state): State<ApiState>) -> impl IntoResponse {
    ok(state.pilot.repos_overview().await)
}
async fn get_one(State(state): State<ApiState>, Path(id): Path<i64>) -> impl IntoResponse {
    let Ok((b, _)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match b.get(id).await {
        Ok(Some(v)) => ok(v),
        Ok(None) => fail(StatusCode::NOT_FOUND, "item introuvable"),
        Err(e) => crate::routes::internal_err("pilot get", e),
    }
}
async fn create(
    State(state): State<ApiState>,
    Json(mut body): Json<NewBacklogItem>,
) -> impl IntoResponse {
    if body.scope != "atelier" && state.app_registry.get(&body.scope).await.is_none() {
        return fail(StatusCode::BAD_REQUEST, "scope app introuvable");
    }
    if body.created_by != "assistant" && body.created_by != "scan" && body.created_by != "system" {
        body.created_by = "user".into();
        // Capture BRUTE (hors chef de projet : future dictée vocale, intégration
        // externe) : la demande n'est ni reformulée ni scorée, elle ne doit pas
        // partir en exécution nocturne telle quelle. La lane `inbox` ayant été
        // supprimée, on la parque en `attention` — Romain (ou le CP) la reprend.
        if !body.needs_user {
            body.needs_user = true;
            body.needs_user_reason
                .get_or_insert_with(|| "capture brute — à cadrer avec le chef de projet".into());
        }
        body.lane = "attention".into();
    }
    let Ok((b, _)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match b.insert(body).await {
        Ok(v) => {
            state.pilot.publish("created", Some(v.clone()), Some(v.id));
            ok(v)
        }
        Err(e) => fail(StatusCode::BAD_REQUEST, e.to_string()),
    }
}
/// Le store fusionne « n'existe pas » et « exécution active » dans le même
/// `None`/`false` : on lève l'ambiguïté par un get préalable — id inconnu → 404,
/// le 409 reste réservé au conflit exec-actif. (Course get→mutation possible si
/// l'item est supprimé entre-temps : bénin, on répond alors 409 au lieu de 404.)
async fn ensure_exists(
    b: &atelier_pilot::BacklogStore,
    id: i64,
    what: &'static str,
) -> Result<(), axum::response::Response> {
    match b.get(id).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(fail(StatusCode::NOT_FOUND, "item introuvable")),
        Err(e) => Err(crate::routes::internal_err(what, e)),
    }
}

async fn update(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Json(body): Json<BacklogPatch>,
) -> impl IntoResponse {
    let Ok((b, _)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    if let Err(resp) = ensure_exists(&b, id, "pilot update get").await {
        return resp;
    }
    match b.update(id, body).await {
        Ok(Some(v)) => {
            state.pilot.publish("updated", Some(v.clone()), Some(id));
            ok(v)
        }
        Ok(None) => fail(StatusCode::CONFLICT, "exécution active : modification refusée"),
        Err(e) => fail(StatusCode::BAD_REQUEST, e.to_string()),
    }
}
async fn remove(State(state): State<ApiState>, Path(id): Path<i64>) -> impl IntoResponse {
    let Ok((b, _)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    if let Err(resp) = ensure_exists(&b, id, "pilot delete get").await {
        return resp;
    }
    match b.delete(id).await {
        Ok(true) => {
            state.pilot.publish("deleted", None, Some(id));
            ok(json!({"deleted":id}))
        }
        Ok(false) => fail(StatusCode::CONFLICT, "exécution active : suppression refusée"),
        Err(e) => crate::routes::internal_err("pilot delete", e),
    }
}

#[derive(Deserialize)]
struct MoveBody {
    lane: String,
    position: Option<f64>,
}
async fn move_item(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Json(body): Json<MoveBody>,
) -> impl IntoResponse {
    update(
        State(state),
        Path(id),
        Json(BacklogPatch {
            lane: Some(body.lane),
            position: body.position,
            ..Default::default()
        }),
    )
    .await
    .into_response()
}
async fn item_runs(State(state): State<ApiState>, Path(id): Path<i64>) -> impl IntoResponse {
    let Ok((_, r)) = stores(&state) else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match r.list_for_item(id).await {
        Ok(v) => ok(v),
        Err(e) => crate::routes::internal_err("pilot item runs", e),
    }
}

#[derive(Deserialize, Default)]
struct RunBody {
    #[serde(default)]
    confirm: bool,
}
async fn run_item(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    body: Option<Json<RunBody>>,
) -> impl IntoResponse {
    let item = match state.pilot.backlog() {
        Some(b) => match b.get(id).await {
            Ok(Some(v)) => v,
            Ok(None) => return fail(StatusCode::NOT_FOUND, "item introuvable"),
            Err(e) => return crate::routes::internal_err("pilot run get", e),
        },
        None => return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible"),
    };
    if item.scope == "atelier" && !body.map(|Json(b)| b.confirm).unwrap_or(false) {
        return fail(
            StatusCode::PRECONDITION_REQUIRED,
            "confirmation explicite requise pour le scope Atelier",
        );
    }
    // File d'attente : le plafond global max_concurrent + Atelier-en-dernier
    // s'appliquent aussi le jour (le dispatcher lance quand un créneau se libère).
    match state.pilot.enqueue_manual(id).await {
        Ok(item) => (
            StatusCode::ACCEPTED,
            Json(json!({"success":true,"data":item})),
        )
            .into_response(),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}
async fn transcript(State(state): State<ApiState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    ok(state.pilot.transcript(id))
}
async fn cancel_run(State(state): State<ApiState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    if state.pilot.cancel_run(id) {
        ok(json!({"cancelled":true}))
    } else {
        fail(StatusCode::NOT_FOUND, "run non actif")
    }
}

async fn get_schedule(State(state): State<ApiState>) -> impl IntoResponse {
    match state.pilot.schedule().await {
        Ok(v) => ok(v),
        Err(e) => fail(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
async fn put_schedule(
    State(state): State<ApiState>,
    Json(body): Json<SchedulePatch>,
) -> impl IntoResponse {
    let Some(s) = state.pilot.schedules() else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match s.update(body).await {
        Ok(v) => ok(v),
        Err(e) => fail(StatusCode::BAD_REQUEST, e.to_string()),
    }
}
async fn get_night(State(state): State<ApiState>) -> impl IntoResponse {
    match state.pilot.night().await {
        Ok(v) => ok(v),
        Err(e) => fail(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
async fn start_night(State(state): State<ApiState>) -> impl IntoResponse {
    match state.pilot.start_night("manual").await {
        Ok(v) => (StatusCode::ACCEPTED, Json(json!({"success":true,"data":v}))).into_response(),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}
async fn cancel_night(State(state): State<ApiState>) -> impl IntoResponse {
    if state.pilot.cancel_night() {
        ok(json!({"cancelled":true}))
    } else {
        fail(StatusCode::CONFLICT, "aucune nuit active")
    }
}

async fn atelier_report(
    State(state): State<ApiState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<AtelierWorkerReport>,
) -> impl IntoResponse {
    if !addr.ip().is_loopback() {
        return fail(StatusCode::FORBIDDEN, "loopback uniquement");
    }
    let Some(s) = state.pilot.schedules() else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "Pilote indisponible");
    };
    match s.secret_matches(&body.secret).await {
        Ok(true) => {}
        Ok(false) => return fail(StatusCode::UNAUTHORIZED, "secret de nuit invalide"),
        Err(e) => return crate::routes::internal_err("pilot report auth", e),
    }
    match state.pilot.accept_atelier_report(body).await {
        Ok(item) => ok(item),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}
