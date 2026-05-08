//! Read-only Git routes (Phase 4).
//!
//! Atelier consomme un mirror rsync des bare repos depuis Medion :
//! /var/lib/atelier/git/repos/{slug}.git. Toutes les routes ci-dessous lisent
//! directement le filesystem via hr_git::GitService.
//!
//! Mutations restent côté homeroute (Medion) :
//! - sync mirror, sync-all : écrivent les repos depuis GitHub
//! - ssh-key, config       : écrivent ~/.ssh/ et config.json
//! - git-receive-pack      : push (modifie les bare repos)
//!
//! La route /git-receive-pack est volontairement omise. Le clone et le fetch
//! HTTP fonctionnent via /info/refs?service=git-upload-pack et /git-upload-pack.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tracing::error;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/repos", get(list_repos))
        .route("/repos/{slug}", get(get_repo))
        .route("/repos/{slug}/commits", get(get_commits))
        .route("/repos/{slug}/branches", get(get_branches))
        // Smart HTTP read-only (clone / fetch)
        .route("/repos/{slug_git}/info/refs", get(git_info_refs))
        .route("/repos/{slug_git}/git-upload-pack", post(git_upload_pack))
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

#[derive(Deserialize)]
struct InfoRefsQuery {
    service: String,
}

async fn git_info_refs(
    State(state): State<ApiState>,
    Path(slug_git): Path<String>,
    Query(q): Query<InfoRefsQuery>,
) -> impl IntoResponse {
    // Phase 4 read-only : refuse explicitly receive-pack (push) advertisements.
    if q.service == "git-receive-pack" {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            Json(json!({
                "error": "git push not supported on Atelier — push via proxy.mynetwk.biz"
            })),
        )
            .into_response();
    }

    let slug = slug_git.strip_suffix(".git").unwrap_or(&slug_git);

    if !state.git.repo_exists(slug) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path_info = format!("/{slug}.git/info/refs");
    let query_string = format!("service={}", q.service);

    match hr_git::cgi::git_cgi(
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
    let slug = slug_git.strip_suffix(".git").unwrap_or(&slug_git);

    if !state.git.repo_exists(slug) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let path_info = format!("/{slug}.git/git-upload-pack");

    match hr_git::cgi::git_cgi(
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
            error!(?e, "git-upload-pack error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
