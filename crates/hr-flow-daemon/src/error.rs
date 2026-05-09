use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("too many concurrent runs for slug {slug}")]
    Overloaded { slug: String },
    #[error("flow error: {0}")]
    Flow(#[from] hr_flow::FlowError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal: {0}")]
    Internal(String),
}

pub type DaemonResult<T> = Result<T, DaemonError>;

impl IntoResponse for DaemonError {
    fn into_response(self) -> axum::response::Response {
        let (status, code) = match &self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Overloaded { .. } => (StatusCode::TOO_MANY_REQUESTS, "overloaded"),
            Self::Flow(_) => (StatusCode::INTERNAL_SERVER_ERROR, "flow_error"),
            Self::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "io_error"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        let body = Json(json!({ "error": self.to_string(), "code": code }));
        (status, body).into_response()
    }
}
