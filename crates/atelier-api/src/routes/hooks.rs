//! Webhooks publiés par les `post-receive` des bare repos git.
//!
//! Stub minimaliste : log + 200 OK. Le vrai handler (rebuild app sur push) est
//! un follow-up — l'idée ici est juste d'absorber le `curl` injecté par
//! `hr_git::GitService::setup_pipeline_hook` pour éviter qu'il échoue
//! silencieusement contre un port mort (l'ancien hr-orchestrator :4001).

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tracing::info;

use crate::state::ApiState;

pub fn router() -> Router<ApiState> {
    Router::new().route("/git-push", post(git_push))
}

#[derive(Debug, Deserialize)]
struct GitPushBody {
    slug: Option<String>,
    #[serde(rename = "ref")]
    ref_name: Option<String>,
    commit: Option<String>,
}

async fn git_push(
    State(_state): State<ApiState>,
    Json(body): Json<GitPushBody>,
) -> impl IntoResponse {
    info!(
        slug = body.slug.as_deref().unwrap_or("?"),
        ref_name = body.ref_name.as_deref().unwrap_or("?"),
        commit = body.commit.as_deref().unwrap_or("?"),
        "git-push hook received (stub — no pipeline action yet)"
    );
    Json(json!({"ok": true}))
}
