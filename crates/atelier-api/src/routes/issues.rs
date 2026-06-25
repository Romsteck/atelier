//! Issues report routes — canal de remontée des frictions **plateforme**.
//!
//! Les chats Claude Code des apps (Studio) rencontrent parfois des soucis qui
//! relèvent d'Atelier (tool MCP qui bug/manque, doc trompeuse, build/deploy/
//! dataverse/agent qui déraille à cause de la plateforme). Plutôt que de
//! contourner en silence, ils appellent ces endpoints (via la skill
//! `0-report-issue`) ; Atelier écrit/relit le fichier `CLAUDE_ISSUES.json` à la
//! racine du source de l'app.
//!
//! WHY côté serveur : l'agent ne mute JAMAIS le JSON lui-même — c'est Atelier
//! qui fait le read-modify-write (atomique + sérialisé), pour qu'un append
//! maladroit ne corrompe pas le tableau et que l'agent « ne se complique pas la
//! vie ». Romain consomme ces fichiers en session dev Atelier (skill
//! `/collect-issues`).
//!
//! Endpoints (non authentifiés, confiance LAN comme les siblings
//! `build-event`/`ship`), montés sous `/api/apps` :
//!   POST   /api/apps/{slug}/issues          {title, area?, severity?, context?, tried?}
//!   GET    /api/apps/{slug}/issues          ?status=open|resolved|dismissed
//!   PATCH  /api/apps/{slug}/issues/{id}     {status?, note?}
//!   DELETE /api/apps/{slug}/issues/{id}

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, patch};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, instrument, warn};

use crate::state::ApiState;

const ISSUES_FILE: &str = "CLAUDE_ISSUES.json";

/// Sérialise tous les read-modify-write du fichier d'issues (toutes apps
/// confondues). Volume faible et la section critique est 100 % synchrone
/// (aucun `.await` tenu pendant le lock) → un `std::sync::Mutex` convient et
/// évite les pertes de mise à jour concurrentes.
static ISSUES_LOCK: Mutex<()> = Mutex::new(());

pub fn app_router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/issues", get(get_issues).post(post_issue))
        .route(
            "/{slug}/issues/{id}",
            patch(patch_issue).delete(delete_issue),
        )
}

fn ok(data: Value) -> axum::response::Response {
    Json(json!({"success": true, "data": data})).into_response()
}

fn fail(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"success": false, "error": msg.into()}))).into_response()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Résout le chemin de `CLAUDE_ISSUES.json` à la racine du source de l'app.
/// 400 si slug invalide, 404 si le dossier source est absent.
fn issues_path(state: &ApiState, slug: &str) -> Result<PathBuf, axum::response::Response> {
    if !atelier_apps::valid_slug(slug) {
        return Err(fail(StatusCode::BAD_REQUEST, "slug invalide"));
    }
    let src = state.apps_src_root.join(slug).join("src");
    if !src.is_dir() {
        return Err(fail(StatusCode::NOT_FOUND, "source d'app introuvable"));
    }
    Ok(src.join(ISSUES_FILE))
}

/// Lit le tableau d'issues. Fichier absent ou vide → `[]`. Un JSON invalide
/// remonte une erreur (on NE réécrit jamais par-dessus un fichier corrompu).
fn read_issues(path: &FsPath) -> io::Result<Vec<Value>> {
    match fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Vec::new()),
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Écrit le tableau via temp + rename (atomique). Perms `0o664` (lisible par
/// tous → l'agent `hr-studio` peut le Read même si l'owner est root) + chown
/// best-effort vers le groupe `hr-studio`. Tout échec de perms est fail-soft
/// (l'écriture du contenu prime).
fn write_issues_atomic(path: &FsPath, issues: &[Value]) -> io::Result<()> {
    let body = serde_json::to_string_pretty(issues)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body.as_bytes())?;
    if let Err(e) = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o664)) {
        warn!(path = %tmp.display(), err = %e, "set_permissions issues tmp failed");
    }
    if let Some(gid) = rules_group_gid() {
        if let Err(e) = std::os::unix::fs::chown(&tmp, None, Some(gid)) {
            warn!(path = %tmp.display(), gid, err = %e, "chown issues tmp failed");
        }
    }
    fs::rename(&tmp, path)
}

/// GID du groupe `hr-studio` (surchargeable via `ATELIER_RULES_GROUP`), parsé
/// depuis `/etc/group`. Même contrat fail-soft que `context.rs`.
fn rules_group_gid() -> Option<u32> {
    let group = std::env::var("ATELIER_RULES_GROUP").unwrap_or_else(|_| "hr-studio".to_string());
    let content = fs::read_to_string("/etc/group").ok()?;
    for line in content.lines() {
        let mut fields = line.split(':');
        if fields.next() == Some(group.as_str()) {
            let _passwd = fields.next();
            return fields.next().and_then(|g| g.parse().ok());
        }
    }
    None
}

#[derive(Deserialize, Default)]
struct PostIssueBody {
    title: Option<String>,
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
/// `id`/`ts`/`app`/`status:open` ; seul `title` est requis.
#[instrument(skip_all)]
async fn post_issue(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    body: Option<Json<PostIssueBody>>,
) -> impl IntoResponse {
    let path = match issues_path(&state, &slug) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let Json(b) = body.unwrap_or_default();
    let title = b.title.unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "title requis");
    }
    let area = b.area.unwrap_or_else(|| "other".to_string());
    let severity = b.severity.unwrap_or_else(|| "medium".to_string());
    let id = format!("iss-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let entry = json!({
        "id": id.clone(),
        "ts": now_rfc3339(),
        "app": slug.clone(),
        "area": area.clone(),
        "severity": severity.clone(),
        "title": title,
        "context": b.context.unwrap_or_default(),
        "tried": b.tried.unwrap_or_default(),
        "status": "open",
    });

    let _guard = ISSUES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut issues = match read_issues(&path) {
        Ok(v) => v,
        Err(e) => {
            warn!(slug = %slug, err = %e, "read CLAUDE_ISSUES.json failed");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("read issues: {e}"));
        }
    };
    issues.push(entry.clone());
    if let Err(e) = write_issues_atomic(&path, &issues) {
        warn!(slug = %slug, err = %e, "write CLAUDE_ISSUES.json failed");
        return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("write issues: {e}"));
    }
    info!(slug = %slug, id = %id, area = %area, severity = %severity, "AppIssueReport");
    ok(entry)
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

/// `GET /api/apps/{slug}/issues` — liste, filtre optionnel `?status=`.
#[instrument(skip_all)]
async fn get_issues(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let path = match issues_path(&state, &slug) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let issues = {
        let _guard = ISSUES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match read_issues(&path) {
            Ok(v) => v,
            Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("read issues: {e}")),
        }
    };
    let data = match q.status {
        Some(s) => issues
            .into_iter()
            .filter(|i| i.get("status").and_then(|v| v.as_str()) == Some(s.as_str()))
            .collect::<Vec<_>>(),
        None => issues,
    };
    ok(json!(data))
}

#[derive(Deserialize, Default)]
struct PatchIssueBody {
    status: Option<String>,
    note: Option<String>,
}

/// `PATCH /api/apps/{slug}/issues/{id}` — met à jour le statut / ajoute une
/// note (utilisé côté dev Atelier pour marquer une remontée traitée).
#[instrument(skip_all)]
async fn patch_issue(
    State(state): State<ApiState>,
    Path((slug, id)): Path<(String, String)>,
    body: Option<Json<PatchIssueBody>>,
) -> impl IntoResponse {
    let path = match issues_path(&state, &slug) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let Json(b) = body.unwrap_or_default();

    let _guard = ISSUES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut issues = match read_issues(&path) {
        Ok(v) => v,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("read issues: {e}")),
    };
    let mut updated: Option<Value> = None;
    for i in issues.iter_mut() {
        if i.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
            if let Some(st) = &b.status {
                i["status"] = json!(st);
            }
            if let Some(note) = &b.note {
                i["note"] = json!(note);
            }
            i["updated_at"] = json!(now_rfc3339());
            updated = Some(i.clone());
            break;
        }
    }
    let Some(entry) = updated else {
        return fail(StatusCode::NOT_FOUND, "issue id introuvable");
    };
    if let Err(e) = write_issues_atomic(&path, &issues) {
        warn!(slug = %slug, err = %e, "write CLAUDE_ISSUES.json failed");
        return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("write issues: {e}"));
    }
    info!(slug = %slug, id = %id, "AppIssuePatch");
    ok(entry)
}

/// `DELETE /api/apps/{slug}/issues/{id}` — retire une remontée (purge après
/// traitement). 404 si l'id est absent.
#[instrument(skip_all)]
async fn delete_issue(
    State(state): State<ApiState>,
    Path((slug, id)): Path<(String, String)>,
) -> impl IntoResponse {
    let path = match issues_path(&state, &slug) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let _guard = ISSUES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut issues = match read_issues(&path) {
        Ok(v) => v,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("read issues: {e}")),
    };
    let before = issues.len();
    issues.retain(|i| i.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
    if issues.len() == before {
        return fail(StatusCode::NOT_FOUND, "issue id introuvable");
    }
    if let Err(e) = write_issues_atomic(&path, &issues) {
        warn!(slug = %slug, err = %e, "write CLAUDE_ISSUES.json failed");
        return fail(StatusCode::INTERNAL_SERVER_ERROR, format!("write issues: {e}"));
    }
    info!(slug = %slug, id = %id, remaining = issues.len(), "AppIssueDelete");
    ok(json!({"deleted": id, "remaining": issues.len()}))
}
