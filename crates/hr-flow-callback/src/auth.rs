//! Bearer-token middleware for the callback router.
//!
//! Constant-time comparison; no logging of the token even on mismatch.

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct CallbackBearer {
    pub bearer: Arc<String>,
}

pub(crate) async fn require_bearer(
    State(b): State<CallbackBearer>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = header.strip_prefix("Bearer ").unwrap_or("");
    if presented.is_empty() || !constant_time_eq(presented.as_bytes(), b.bearer.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // No early return on a length mismatch — fold the difference into `diff`
    // and iterate over the longer slice so the comparison never branches on
    // secret content.
    let mut diff: u8 = (a.len() != b.len()) as u8;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}
