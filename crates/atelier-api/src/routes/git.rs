//! Git routes — Atelier est autoritaire sur les bare repos depuis le
//! rapatriement Medion (2026-05-09). hr-orchestrator ne sert plus rien.
//!
//! Lecture seule : `/repos`, `/repos/{slug}`, `/repos/{slug}/commits`,
//! `/repos/{slug}/branches`, `/info/refs?service=git-upload-pack`,
//! `/git-upload-pack`.
//!
//! Écriture : `/info/refs?service=git-receive-pack`, `/git-receive-pack`
//! (push), `/ssh-key` (GET/POST), `/config` (GET/PUT), `/repos/{slug}/mirror`
//! (POST/DELETE), `/repos/{slug}/mirror/sync` (POST), `/repos/sync-all` (POST).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use atelier_git::types::{GitConfig, MirrorConfig};
use serde::Deserialize;
use serde_json::json;
use tracing::{error, info, instrument, warn};

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/repos", get(list_repos))
        .route("/repos/{slug}", get(get_repo))
        .route("/repos/{slug}/commits", get(get_commits))
        .route("/repos/{slug}/commits/{sha}", get(get_commit_detail))
        .route("/repos/{slug}/activity", get(get_activity))
        .route("/repos/{slug}/branches", get(get_branches))
        // Smart HTTP (clone / fetch / push).
        // Les routes pack transportent le packfile dans le corps de requête : sans
        // override, l'extracteur `Bytes` plafonne à 2 Mo (défaut axum) → tout push
        // > 2 Mo est rejeté en 413 avant même d'atteindre le handler. On lève la
        // limite uniquement sur ces deux routes ; le reste de l'API garde le défaut.
        .route("/repos/{slug_git}/info/refs", get(git_info_refs))
        .route(
            "/repos/{slug_git}/git-upload-pack",
            post(git_upload_pack).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/repos/{slug_git}/git-receive-pack",
            post(git_receive_pack).layer(DefaultBodyLimit::disable()),
        )
        // SSH key
        .route("/ssh-key", get(get_ssh_key).post(generate_ssh_key))
        // Config (token + org + mirrors index)
        .route("/config", get(get_config).put(update_config))
        // Mirror enable/disable + sync
        .route("/repos/{slug}/mirror", post(enable_mirror).delete(disable_mirror))
        .route("/repos/{slug}/mirror/sync", post(mirror_sync))
        .route("/repos/sync-all", post(sync_all))
}

fn err500(e: impl std::fmt::Display) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": format!("{e}")})),
    )
        .into_response()
}

async fn list_repos(State(state): State<ApiState>) -> impl IntoResponse {
    match state.git.list_repos().await {
        Ok(repos) => Json(json!({"repos": repos})).into_response(),
        Err(e) => err500(e),
    }
}

async fn get_repo(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    match state.git.get_repo(&slug).await {
        Ok(Some(repo)) => Json(json!({"repo": repo})).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Repository not found"})),
        )
            .into_response(),
        Err(e) => err500(e),
    }
}

#[derive(Deserialize)]
struct CommitsQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    50
}

async fn get_commits(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<CommitsQuery>,
) -> impl IntoResponse {
    match state.git.get_commits(&slug, q.limit).await {
        Ok(commits) => Json(json!({"commits": commits})).into_response(),
        Err(e) => err500(e),
    }
}

async fn get_branches(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    match state.git.get_branches(&slug).await {
        Ok(branches) => Json(json!({"branches": branches})).into_response(),
        Err(e) => err500(e),
    }
}

#[derive(Deserialize, Debug)]
struct ActivityQuery {
    #[serde(default = "default_activity_days")]
    days: u32,
}
fn default_activity_days() -> u32 {
    365
}

#[instrument(skip(state, q))]
async fn get_activity(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<ActivityQuery>,
) -> impl IntoResponse {
    let days = q.days.clamp(1, 1825); // cap ~5 ans
    match state.git.get_commit_activity(&slug, days).await {
        Ok(activity) => Json(json!({"activity": activity})).into_response(),
        Err(e) => err500(e),
    }
}

#[instrument(skip(state))]
async fn get_commit_detail(
    State(state): State<ApiState>,
    Path((slug, sha)): Path<(String, String)>,
) -> impl IntoResponse {
    // Validation hex-only en amont → 400 (la méthode re-valide en défense
    // en profondeur). Bloque toute injection d'argument git.
    let valid_sha = (4..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid_sha {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid commit sha"})),
        )
            .into_response();
    }
    match state.git.get_commit_detail(&slug, &sha).await {
        Ok(commit) => Json(json!({"commit": commit})).into_response(),
        Err(e) => {
            // SHA bien formé mais introuvable → 404, sinon 500.
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
            } else {
                err500(e)
            }
        }
    }
}

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: String,
}

async fn git_info_refs(
    State(state): State<ApiState>,
    Path(slug_git): Path<String>,
    Query(q): Query<InfoRefsQuery>,
) -> impl IntoResponse {
    let slug = slug_git.strip_suffix(".git").unwrap_or(&slug_git);

    if !state.git.repo_exists(slug) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path_info = format!("/{slug}.git/info/refs");
    let query_string = format!("service={}", q.service);

    match atelier_git::cgi::git_cgi(
        state.git.repos_dir(),
        &path_info,
        &query_string,
        "GET",
        "",
        &[],
    )
    .await
    {
        Ok(resp) => {
            let mut builder = axum::http::Response::builder().status(resp.status);
            builder = builder.header(header::CONTENT_TYPE, &resp.content_type);
            builder = builder.header(header::CACHE_CONTROL, "no-cache");
            for (k, v) in &resp.headers {
                let lower = k.to_lowercase();
                if lower != "content-type" && lower != "status" {
                    builder = builder.header(k.as_str(), v.as_str());
                }
            }
            builder
                .body(axum::body::Body::from(resp.body))
                .unwrap()
                .into_response()
        }
        Err(e) => {
            error!(?e, "git-http-backend info/refs error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn git_upload_pack(
    State(state): State<ApiState>,
    Path(slug_git): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    git_pack_cgi(state, slug_git, headers, body, "git-upload-pack").await
}

async fn git_receive_pack(
    State(state): State<ApiState>,
    Path(slug_git): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    git_pack_cgi(state, slug_git, headers, body, "git-receive-pack").await
}

async fn git_pack_cgi(
    state: ApiState,
    slug_git: String,
    headers: axum::http::HeaderMap,
    body: Bytes,
    service: &str,
) -> axum::response::Response {
    let slug = slug_git.strip_suffix(".git").unwrap_or(&slug_git);

    if !state.git.repo_exists(slug) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let path_info = format!("/{slug}.git/{service}");

    match atelier_git::cgi::git_cgi(
        state.git.repos_dir(),
        &path_info,
        "",
        "POST",
        content_type,
        &body,
    )
    .await
    {
        Ok(resp) => {
            let mut builder = axum::http::Response::builder().status(resp.status);
            builder = builder.header(header::CONTENT_TYPE, &resp.content_type);
            for (k, v) in &resp.headers {
                let lower = k.to_lowercase();
                if lower != "content-type" && lower != "status" {
                    builder = builder.header(k.as_str(), v.as_str());
                }
            }
            builder
                .body(axum::body::Body::from(resp.body))
                .unwrap()
                .into_response()
        }
        Err(e) => {
            error!(?e, service, "git-http-backend pack error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// --- SSH key ---------------------------------------------------------------

async fn get_ssh_key(State(state): State<ApiState>) -> impl IntoResponse {
    match state.git.get_ssh_key().await {
        Ok(info) => Json(info).into_response(),
        Err(e) => err500(e),
    }
}

async fn generate_ssh_key(State(state): State<ApiState>) -> impl IntoResponse {
    match state.git.generate_ssh_key().await {
        Ok(info) => Json(info).into_response(),
        Err(e) => err500(e),
    }
}

// --- Config (token + org) --------------------------------------------------

const TOKEN_MASK_PREFIX: &str = "***...";

fn mask_token(t: &str) -> String {
    let tail: String = t.chars().rev().take(4).collect::<String>().chars().rev().collect();
    format!("{TOKEN_MASK_PREFIX}{tail}")
}

async fn get_config(State(state): State<ApiState>) -> impl IntoResponse {
    match state.git.load_config().await {
        Ok(mut cfg) => {
            if let Some(t) = cfg.github_token.as_ref() {
                if !t.is_empty() {
                    cfg.github_token = Some(mask_token(t));
                }
            }
            Json(cfg).into_response()
        }
        Err(e) => err500(e),
    }
}

async fn update_config(
    State(state): State<ApiState>,
    Json(payload): Json<GitConfig>,
) -> impl IntoResponse {
    // Preserve existing token if frontend re-sent the mask.
    let existing = state.git.load_config().await.unwrap_or_default();
    let token = match payload.github_token {
        Some(ref t) if t.starts_with(TOKEN_MASK_PREFIX) => existing.github_token.clone(),
        Some(t) if t.is_empty() => existing.github_token.clone(),
        other => other,
    };
    let merged = GitConfig {
        github_token: token,
        github_org: payload.github_org,
        mirrors: if payload.mirrors.is_empty() {
            existing.mirrors
        } else {
            payload.mirrors
        },
    };
    match state.git.save_config(&merged).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err500(e),
    }
}

// --- Mirror enable / disable / sync ---------------------------------------

#[derive(Deserialize)]
struct EnableMirrorBody {
    #[serde(default)]
    org: Option<String>,
}

async fn enable_mirror(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<EnableMirrorBody>,
) -> impl IntoResponse {
    let cfg = state.git.load_config().await.unwrap_or_default();
    let org = body
        .org
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.github_org.clone());
    if org.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "github_org missing — provide body.org or set /config first"})),
        )
            .into_response();
    }

    if let Err(e) = state.git.enable_mirror(&slug, &org).await {
        return err500(e);
    }

    // Persist mirror entry in config
    let mut cfg = state.git.load_config().await.unwrap_or_default();
    cfg.mirrors.insert(
        slug.clone(),
        MirrorConfig {
            enabled: true,
            github_ssh_url: Some(format!("git@github.com:{org}/{slug}.git")),
            visibility: atelier_git::types::RepoVisibility::Private,
            last_sync: None,
            last_error: None,
        },
    );
    if let Err(e) = state.git.save_config(&cfg).await {
        warn!(slug, error = %e, "mirror enabled but config save failed");
    }
    info!(slug, org, "mirror enabled");
    Json(json!({"ok": true})).into_response()
}

async fn disable_mirror(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = state.git.disable_mirror(&slug).await {
        return err500(e);
    }
    let mut cfg = state.git.load_config().await.unwrap_or_default();
    cfg.mirrors.remove(&slug);
    if let Err(e) = state.git.save_config(&cfg).await {
        warn!(slug, error = %e, "mirror disabled but config save failed");
    }
    info!(slug, "mirror disabled");
    Json(json!({"ok": true})).into_response()
}

async fn mirror_sync(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    match state.git.trigger_sync(&slug).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err500(e),
    }
}

async fn sync_all(State(state): State<ApiState>) -> impl IntoResponse {
    let cfg = state.git.load_config().await.unwrap_or_default();
    let mut results = Vec::new();
    for (slug, mirror) in &cfg.mirrors {
        if !mirror.enabled {
            continue;
        }
        let outcome = match state.git.trigger_sync(slug).await {
            Ok(()) => json!({"slug": slug, "ok": true}),
            Err(e) => json!({"slug": slug, "ok": false, "error": format!("{e}")}),
        };
        results.push(outcome);
    }
    Json(json!({"results": results})).into_response()
}
