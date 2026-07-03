use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// 500 générique + correlation_id : l'erreur complète part dans les logs, le
/// client ne voit JAMAIS le message brut (les erreurs sqlx/anyhow/FS fuient des
/// noms de tables, contraintes et chemins internes — même motif que
/// `dv::db_error_resp`, audit P1 #8). À réserver aux erreurs internes ; les
/// erreurs de validation lisibles par l'utilisateur restent en clair.
pub(crate) fn internal_err(context: &str, e: impl std::fmt::Display) -> Response {
    let correlation_id = uuid::Uuid::new_v4();
    tracing::error!(
        correlation_id = %correlation_id,
        context = %context,
        error = %e,
        "internal error"
    );
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "success": false,
            "error": format!("erreur interne ({context}) — réf. {correlation_id}"),
        })),
    )
        .into_response()
}

pub mod agent;
pub mod apps;
pub mod apps_db;
pub mod apps_proxy;
pub mod backup;
pub mod docs;
pub mod dv;
pub mod git;
pub mod health;
pub mod homeroute;
pub mod hooks;
pub mod issues;
pub mod logs;
pub mod mcp;
pub mod source;
pub mod surveillance;
pub mod tasks;
pub mod ws;
