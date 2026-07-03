//! Path-based reverse proxy for apps under `app.mynetwk.biz/apps/{slug}/...`.
//!
//! Atelier devient le routeur applicatif autonome (la séparation Studio→Atelier
//! visait précisément ce path-based access depuis un domaine unique). Le router
//! `/apps` est mounté à la racine du Router HTTP, AVANT le SPA fallback, pour
//! intercepter les requêtes apps avant qu'elles soient absorbées par l'index.html.
//!
//! # Comportement
//!
//! Deux modes, selon que le slug est inscrit dans `ApiState::preserve_prefix_slugs`
//! (cf. `upstream_path`) :
//!
//! - **strip** (défaut — SPA Vite / Axum) : l'app vit à la racine, on retire le
//!   préfixe. `/apps/{slug}` → 308 vers `/apps/{slug}/` ; `/apps/{slug}/{rest}`
//!   → `127.0.0.1:port/{rest}`.
//! - **no-strip** (Next.js à `basePath`, ex. `www`) : l'app attend le préfixe sur
//!   chaque requête, on le transmet tel quel. `/apps/{slug}/{rest}` →
//!   `127.0.0.1:port/apps/{slug}/{rest}` ; pas de 308 à la racine (Next gère son
//!   propre trailing-slash).
//!
//! Dans les deux modes : `Upgrade: websocket` → bridge bidirectionnel WS ; slug
//! inconnu / app stoppée (absente de `port_registry`) → 404.
//!
//! # WebSocket bridging
//!
//! Le proxy détecte les upgrades WS et établit un tunnel raw bytes via :
//!   browser  ⇄  axum::extract::ws  ⇄  tokio_tungstenite  ⇄  upstream app
//!
//! - Les subprotocols (`Sec-WebSocket-Protocol`) sont propagés bout-à-bout.
//! - Les cookies, `X-Forwarded-{Host,Proto,For,Prefix}` sont injectés.
//! - Les close frames (code + reason) sont fidèlement traduits dans les 2 sens.
//! - Connexion upstream avec timeout 10s ; durée + raison de fermeture loggés.
//! - Si upstream tombe : close du browser avec code 1011 (server error).

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::Body,
    extract::{
        FromRequestParts, Path, Request, State, WebSocketUpgrade,
        ws::{CloseFrame as AxCloseFrame, Message as AxMsg, WebSocket},
    },
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::any,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use tokio_tungstenite::tungstenite::{
    Bytes as TBytes, Message as TMsg, Utf8Bytes as TUtf8,
    client::IntoClientRequest,
    protocol::{CloseFrame as TCloseFrame, frame::coding::CloseCode as TCloseCode},
};
use tracing::{info, instrument, warn};

use crate::state::ApiState;

/// Shared upstream client. `connect_timeout` bounds the dial to a dead/hung
/// port; there is deliberately NO total timeout — request and response bodies
/// are streamed and may legitimately outlive any fixed budget (SSE, large
/// downloads/uploads). Hang detection is per-request via
/// [`UPSTREAM_HEADERS_TIMEOUT`] around `send()`.
static PROXY_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builder with static config cannot fail")
});

/// Budget for the upstream to produce response *headers*. `send()` resolves as
/// soon as headers arrive, so streaming responses (SSE, downloads) are not
/// bounded by this; only an app that accepted the connection but never answers
/// trips it. Generous because a streamed request body (upload) must also fit
/// inside this window.
const UPSTREAM_HEADERS_TIMEOUT: Duration = Duration::from_secs(300);

/// Upstream WS connect timeout. Beyond this, we close the browser with 1011.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Close code we send the browser when the upstream is unreachable / errors out.
/// 1011 = "Internal server error" (per RFC 6455 §7.4.1).
const CLOSE_CODE_UPSTREAM_ERROR: u16 = 1011;

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}", any(proxy_root_redirect))
        .route("/{slug}/", any(proxy_root))
        .route("/{slug}/{*rest}", any(proxy_with_path))
}

/// Safety net for any NextJS asset requested at the public root `/_next/...`
/// instead of under the app's `assetPrefix`. With `assetPrefix:"/apps/www"` set,
/// www now emits its chunks under `/apps/www/_next/...` (served by
/// `proxy_with_path`), so this fallback is rarely hit — but if a bare `/_next/...`
/// request slips through, it is routed to a NextJS app slug (default `www`,
/// override via `ATELIER_NEXTJS_FALLBACK_SLUG`). The forward is preserve-aware:
/// for a no-strip slug it targets `/apps/{slug}/_next/...`, where the app
/// actually serves its chunks.
pub async fn next_fallback_handler(State(state): State<ApiState>, req: Request) -> Response {
    let slug = std::env::var("ATELIER_NEXTJS_FALLBACK_SLUG").unwrap_or_else(|_| "www".to_string());
    let path = req.uri().path().to_string();
    let rest = path.trim_start_matches('/').to_string();
    // Preserve-aware: a no-strip Next.js app serves its framework chunks under
    // `/apps/{slug}/_next/...` (its `assetPrefix`), so forward there rather than
    // to the bare `/_next/...` the app no longer exposes.
    let up = upstream_path(preserve(&state, &slug), &slug, &rest);
    forward(slug, up, state, req).await
}

/// Whether `slug`'s `/apps/{slug}` prefix must be preserved (no-strip).
/// See [`ApiState::preserve_prefix_slugs`].
fn preserve(state: &ApiState, slug: &str) -> bool {
    state.preserve_prefix_slugs.contains(slug)
}

/// Path forwarded upstream (no leading `/`).
///
/// - strip mode (SPA/Vite, Axum): the app lives at the server root, so we drop
///   the `/apps/{slug}` prefix and forward only `rest`.
/// - no-strip mode (Next.js `basePath`): the app expects the prefix on every
///   request, so we forward `apps/{slug}/{rest}` verbatim. At the root we emit
///   `apps/{slug}` WITHOUT a trailing slash — Next.js (`trailingSlash=false`)
///   treats that as canonical and serves the home page directly; forwarding
///   `apps/{slug}/` would make it 308 back to `apps/{slug}`, an extra hop.
fn upstream_path(preserve: bool, slug: &str, rest: &str) -> String {
    if !preserve {
        rest.to_string()
    } else if rest.is_empty() {
        format!("apps/{slug}")
    } else {
        format!("apps/{slug}/{rest}")
    }
}

async fn proxy_root_redirect(
    Path(slug): Path<String>,
    State(state): State<ApiState>,
    req: Request,
) -> Response {
    if preserve(&state, &slug) {
        // Next.js owns its own trailing-slash policy; forward `/apps/{slug}`
        // verbatim. A 308 to `/apps/{slug}/` here would loop against Next's own
        // `/apps/{slug}/` → `/apps/{slug}` redirect.
        let up = upstream_path(true, &slug, "");
        return forward(slug, up, state, req).await;
    }
    // SPA: normalize to a trailing slash so relative asset URLs resolve.
    let target = match req.uri().query() {
        Some(q) => format!("/apps/{slug}/?{q}"),
        None => format!("/apps/{slug}/"),
    };
    Redirect::permanent(&target).into_response()
}

async fn proxy_root(
    Path(slug): Path<String>,
    State(state): State<ApiState>,
    req: Request,
) -> Response {
    let up = upstream_path(preserve(&state, &slug), &slug, "");
    forward(slug, up, state, req).await
}

async fn proxy_with_path(
    Path((slug, rest)): Path<(String, String)>,
    State(state): State<ApiState>,
    req: Request,
) -> Response {
    let up = upstream_path(preserve(&state, &slug), &slug, &rest);
    forward(slug, up, state, req).await
}

#[instrument(skip(state, req), fields(method = %req.method(), uri = %req.uri()))]
async fn forward(slug: String, upstream_path: String, state: ApiState, req: Request) -> Response {
    let port = match state.port_registry.get(&slug).await {
        Some(p) => p,
        None => {
            warn!(slug = %slug, "apps_proxy: unknown slug");
            return (StatusCode::NOT_FOUND, format!("unknown app: {slug}")).into_response();
        }
    };

    if is_websocket_upgrade(req.headers()) {
        // Hand the head to the WebSocketUpgrade extractor; the body is empty
        // for a valid WS handshake so we drop it.
        let (parts, _body) = req.into_parts();
        return ws_forward(slug, upstream_path, port, parts).await;
    }
    http_forward(slug, upstream_path, port, req).await
}

// ─── HTTP path ────────────────────────────────────────────────────────────────

async fn http_forward(slug: String, upstream_path: String, port: u16, req: Request) -> Response {
    let method = req.method().clone();
    let original_uri = req.uri().clone();
    let original_headers = req.headers().clone();

    let mut upstream_url = format!("http://127.0.0.1:{port}/{upstream_path}");
    if let Some(q) = original_uri.query() {
        upstream_url = format!("{upstream_url}?{q}");
    }

    let upstream_method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return (StatusCode::METHOD_NOT_ALLOWED, "bad method").into_response();
        }
    };

    // Stream the request body through instead of buffering it (no size cap,
    // no RAM proportional to the upload). reqwest sends it chunked, so the
    // original content-length must NOT be forwarded alongside (protocol
    // mismatch); hyper recomputes framing itself.
    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());

    let mut fwd_headers = reqwest::header::HeaderMap::new();
    for (name, value) in original_headers.iter() {
        if is_hop_by_hop(name.as_str())
            || name.as_str().eq_ignore_ascii_case("host")
            || name.as_str().eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            fwd_headers.insert(n, v);
        }
    }
    inject_forwarded_headers_reqwest(&mut fwd_headers, &slug, &original_headers);

    let send = PROXY_CLIENT
        .request(upstream_method, &upstream_url)
        .headers(fwd_headers)
        .body(body)
        .send();
    let upstream_resp = match tokio::time::timeout(UPSTREAM_HEADERS_TIMEOUT, send).await {
        Ok(Ok(r)) => r,
        Ok(Err(err)) => {
            warn!(slug = %slug, port, url = %upstream_url, error = %err, "apps_proxy: upstream connection failed");
            return (StatusCode::BAD_GATEWAY, format!("upstream {slug} unreachable"))
                .into_response();
        }
        Err(_) => {
            warn!(
                slug = %slug, port, url = %upstream_url,
                timeout_secs = UPSTREAM_HEADERS_TIMEOUT.as_secs(),
                "apps_proxy: upstream did not answer in time"
            );
            return (StatusCode::GATEWAY_TIMEOUT, format!("upstream {slug} timed out"))
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream_resp.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp_headers = HeaderMap::new();
    for (name, value) in upstream_resp.headers().iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            resp_headers.append(n, v);
        }
    }

    // Stream the response body: bytes reach the browser as the app emits them
    // (SSE/chunked work, no full-buffer in RAM). A mid-stream upstream error
    // surfaces as a truncated body — the status line is already gone.
    let mut response = Response::new(Body::from_stream(upstream_resp.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = resp_headers;
    response
}

fn inject_forwarded_headers_reqwest(
    fwd: &mut reqwest::header::HeaderMap,
    slug: &str,
    src: &HeaderMap,
) {
    if let Some(host) = src.get("host") {
        if let Ok(v) = reqwest::header::HeaderValue::from_bytes(host.as_bytes()) {
            fwd.insert(
                reqwest::header::HeaderName::from_static("x-forwarded-host"),
                v,
            );
        }
    }
    fwd.insert(
        reqwest::header::HeaderName::from_static("x-forwarded-proto"),
        reqwest::header::HeaderValue::from_static("https"),
    );
    if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("/apps/{slug}")) {
        fwd.insert(
            reqwest::header::HeaderName::from_static("x-forwarded-prefix"),
            v,
        );
    }
}

// ─── WebSocket path ───────────────────────────────────────────────────────────

/// `Upgrade: websocket` AND `Connection` header containing `upgrade` (case-insensitive).
fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let upgrade_ok = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let connection_ok = headers
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tok| tok.trim().eq_ignore_ascii_case("upgrade"))
        });
    upgrade_ok && connection_ok
}

async fn ws_forward(
    slug: String,
    upstream_path: String,
    port: u16,
    mut parts: axum::http::request::Parts,
) -> Response {
    let query = parts.uri.query().map(|s| s.to_string());
    let mut upstream_url = format!("ws://127.0.0.1:{port}/{upstream_path}");
    if let Some(q) = query {
        upstream_url = format!("{upstream_url}?{q}");
    }

    let original_headers = parts.headers.clone();
    let requested_protocols: Vec<String> = original_headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Extract the WebSocketUpgrade from the request parts. `&()` is fine
    // here because WebSocketUpgrade doesn't depend on the app state.
    let ws_upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(u) => u,
        Err(rej) => {
            warn!(slug = %slug, "apps_proxy: WebSocketUpgrade extractor rejected request");
            return rej.into_response();
        }
    };

    info!(slug = %slug, port, url = %upstream_url, ?requested_protocols, "apps_proxy: ws upgrade");

    ws_upgrade
        .on_upgrade(move |client_ws| async move {
            run_ws_bridge(slug, client_ws, upstream_url, original_headers).await;
        })
        .into_response()
}

async fn run_ws_bridge(
    slug: String,
    client_ws: WebSocket,
    upstream_url: String,
    src_headers: HeaderMap,
) {
    // Build the upstream WS request and copy relevant headers.
    let mut upstream_req = match upstream_url.as_str().into_client_request() {
        Ok(r) => r,
        Err(err) => {
            warn!(slug = %slug, %err, "ws bridge: bad upstream url");
            close_client(client_ws, CLOSE_CODE_UPSTREAM_ERROR, "bad upstream url").await;
            return;
        }
    };
    {
        let h = upstream_req.headers_mut();
        copy_header(h, &src_headers, "cookie");
        copy_header(h, &src_headers, "authorization");
        copy_header(h, &src_headers, "user-agent");
        copy_header(h, &src_headers, "sec-websocket-protocol");
        copy_header(h, &src_headers, "sec-websocket-extensions");
        if let Some(host) = src_headers.get("host") {
            if let Ok(v) = HeaderValue::from_bytes(host.as_bytes()) {
                h.insert("x-forwarded-host", v);
            }
        }
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        if let Ok(v) = HeaderValue::from_str(&format!("/apps/{slug}")) {
            h.insert("x-forwarded-prefix", v);
        }
        // X-Forwarded-For: append (or set) the immediate client. Best-effort —
        // the public client IP is hidden behind hr-edge anyway, so we only
        // forward what we already received from upstream of us.
        copy_header(h, &src_headers, "x-forwarded-for");
    }

    // Connect upstream with a timeout.
    let connect = tokio::time::timeout(
        WS_CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(upstream_req),
    )
    .await;

    let upstream_ws = match connect {
        Ok(Ok((ws, _resp))) => ws,
        Ok(Err(err)) => {
            warn!(slug = %slug, %err, "ws bridge: upstream connect failed");
            close_client(client_ws, CLOSE_CODE_UPSTREAM_ERROR, "upstream unreachable").await;
            return;
        }
        Err(_) => {
            warn!(slug = %slug, timeout_secs = WS_CONNECT_TIMEOUT.as_secs(), "ws bridge: upstream connect timeout");
            close_client(client_ws, CLOSE_CODE_UPSTREAM_ERROR, "upstream timeout").await;
            return;
        }
    };

    let started = Instant::now();
    let (close_reason, sent_close) = bridge_loop(client_ws, upstream_ws).await;
    info!(
        slug = %slug,
        duration_secs = started.elapsed().as_secs_f64(),
        reason = %close_reason,
        sent_close,
        "ws bridge: closed"
    );
}

/// Close reason — used only for logging.
#[derive(Debug, Clone, Copy)]
enum CloseReason {
    ClientHangup,
    UpstreamHangup,
    ClientError,
    UpstreamError,
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ClientHangup => "client_hangup",
            Self::UpstreamHangup => "upstream_hangup",
            Self::ClientError => "client_error",
            Self::UpstreamError => "upstream_error",
        })
    }
}

/// Bridge the two streams until one side closes. Returns the close reason
/// and whether a Close frame was actually exchanged (vs. abrupt drop).
async fn bridge_loop(
    client_ws: WebSocket,
    upstream_ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> (CloseReason, bool) {
    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut up_tx, mut up_rx) = upstream_ws.split();
    let mut sent_close = false;

    let reason = loop {
        tokio::select! {
            // browser → upstream
            msg = client_rx.next() => match msg {
                Some(Ok(m)) => {
                    let is_close = matches!(m, AxMsg::Close(_));
                    let translated = ax_to_tung(m);
                    if is_close {
                        sent_close = true;
                    }
                    if up_tx.send(translated).await.is_err() {
                        break CloseReason::UpstreamError;
                    }
                    if is_close {
                        break CloseReason::ClientHangup;
                    }
                }
                Some(Err(_)) => break CloseReason::ClientError,
                None => break CloseReason::ClientHangup,
            },
            // upstream → browser
            msg = up_rx.next() => match msg {
                Some(Ok(m)) => {
                    let is_close = matches!(m, TMsg::Close(_));
                    if let Some(translated) = tung_to_ax(m) {
                        if is_close {
                            sent_close = true;
                        }
                        if client_tx.send(translated).await.is_err() {
                            break CloseReason::ClientError;
                        }
                        if is_close {
                            break CloseReason::UpstreamHangup;
                        }
                    }
                }
                Some(Err(_)) => break CloseReason::UpstreamError,
                None => break CloseReason::UpstreamHangup,
            },
        }
    };
    (reason, sent_close)
}

async fn close_client(ws: WebSocket, code: u16, reason: &str) {
    let mut s = ws;
    let frame = AxCloseFrame {
        code,
        reason: reason.to_string().into(),
    };
    let _ = s.send(AxMsg::Close(Some(frame))).await;
    let _ = s.close().await;
}

fn copy_header(
    dst: &mut tokio_tungstenite::tungstenite::http::HeaderMap,
    src: &HeaderMap,
    name: &'static str,
) {
    if let Some(v) = src.get(name) {
        if let Ok(hv) =
            tokio_tungstenite::tungstenite::http::HeaderValue::from_bytes(v.as_bytes())
        {
            dst.insert(name, hv);
        }
    }
}

// ─── Message translation ──────────────────────────────────────────────────────
//
// Pure functions kept here so they're trivially unit-testable without spinning
// up the rest of the proxy. Round-trip property: `tung_to_ax(ax_to_tung(m)) ≈ m`
// (modulo Close-frame canonicalisation; see tests).

fn ax_to_tung(m: AxMsg) -> TMsg {
    match m {
        AxMsg::Text(t) => TMsg::Text(TUtf8::from(t.as_str())),
        AxMsg::Binary(b) => TMsg::Binary(TBytes::copy_from_slice(&b)),
        AxMsg::Ping(p) => TMsg::Ping(TBytes::copy_from_slice(&p)),
        AxMsg::Pong(p) => TMsg::Pong(TBytes::copy_from_slice(&p)),
        AxMsg::Close(Some(cf)) => TMsg::Close(Some(TCloseFrame {
            code: TCloseCode::from(cf.code),
            reason: TUtf8::from(cf.reason.as_str()),
        })),
        AxMsg::Close(None) => TMsg::Close(None),
    }
}

/// Returns `None` for tungstenite-only frame types that have no axum equivalent
/// (currently only `Frame(_)` — raw frames are never produced by tungstenite's
/// high-level reader, only by direct frame construction).
fn tung_to_ax(m: TMsg) -> Option<AxMsg> {
    match m {
        TMsg::Text(t) => Some(AxMsg::Text(t.as_str().to_string().into())),
        TMsg::Binary(b) => Some(AxMsg::Binary(b.to_vec().into())),
        TMsg::Ping(p) => Some(AxMsg::Ping(p.to_vec().into())),
        TMsg::Pong(p) => Some(AxMsg::Pong(p.to_vec().into())),
        TMsg::Close(Some(cf)) => Some(AxMsg::Close(Some(AxCloseFrame {
            code: cf.code.into(),
            reason: cf.reason.as_str().to_string().into(),
        }))),
        TMsg::Close(None) => Some(AxMsg::Close(None)),
        TMsg::Frame(_) => None,
    }
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(name: &str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn upstream_path_strip_mode() {
        // strip: prefix removed, only `rest` forwarded.
        assert_eq!(upstream_path(false, "files", ""), "");
        assert_eq!(upstream_path(false, "files", "about"), "about");
        assert_eq!(
            upstream_path(false, "files", "_next/static/x.js"),
            "_next/static/x.js"
        );
    }

    #[test]
    fn upstream_path_no_strip_mode() {
        // no-strip: full prefix preserved; root has NO trailing slash.
        assert_eq!(upstream_path(true, "www", ""), "apps/www");
        assert_eq!(upstream_path(true, "www", "about"), "apps/www/about");
        assert_eq!(
            upstream_path(true, "www", "_next/static/x.js"),
            "apps/www/_next/static/x.js"
        );
        assert_eq!(upstream_path(true, "www", "api/health"), "apps/www/api/health");
    }

    #[test]
    fn detects_websocket_upgrade_basic() {
        let mut h = HeaderMap::new();
        h.insert("upgrade", HeaderValue::from_static("websocket"));
        h.insert("connection", HeaderValue::from_static("Upgrade"));
        assert!(is_websocket_upgrade(&h));
    }

    #[test]
    fn detects_websocket_upgrade_compound_connection() {
        let mut h = HeaderMap::new();
        h.insert("upgrade", HeaderValue::from_static("WebSocket"));
        h.insert(
            "connection",
            HeaderValue::from_static("keep-alive, Upgrade"),
        );
        assert!(is_websocket_upgrade(&h));
    }

    #[test]
    fn rejects_non_upgrade() {
        assert!(!is_websocket_upgrade(&HeaderMap::new()));
        assert!(!is_websocket_upgrade(&hdr("upgrade", "websocket")));
        assert!(!is_websocket_upgrade(&hdr("connection", "Upgrade")));
        let mut h = HeaderMap::new();
        h.insert("upgrade", HeaderValue::from_static("h2c"));
        h.insert("connection", HeaderValue::from_static("Upgrade"));
        assert!(!is_websocket_upgrade(&h));
    }

    #[test]
    fn translate_text_round_trip() {
        let original = AxMsg::Text("hello \u{1F44B}".to_string().into());
        let t = ax_to_tung(original);
        let back = tung_to_ax(t).unwrap();
        match back {
            AxMsg::Text(s) => assert_eq!(s.as_str(), "hello \u{1F44B}"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn translate_binary_round_trip() {
        let original = AxMsg::Binary(vec![0u8, 1, 2, 255].into());
        let t = ax_to_tung(original);
        let back = tung_to_ax(t).unwrap();
        match back {
            AxMsg::Binary(b) => assert_eq!(&*b, &[0u8, 1, 2, 255]),
            other => panic!("expected binary, got {other:?}"),
        }
    }

    #[test]
    fn translate_close_with_reason_round_trip() {
        let original = AxMsg::Close(Some(AxCloseFrame {
            code: 1000,
            reason: "bye".to_string().into(),
        }));
        let t = ax_to_tung(original);
        let back = tung_to_ax(t).unwrap();
        match back {
            AxMsg::Close(Some(cf)) => {
                assert_eq!(cf.code, 1000);
                assert_eq!(cf.reason.as_str(), "bye");
            }
            other => panic!("expected Close(Some), got {other:?}"),
        }
    }

    #[test]
    fn translate_close_without_reason() {
        let t = ax_to_tung(AxMsg::Close(None));
        assert!(matches!(t, TMsg::Close(None)));
        let back = tung_to_ax(t).unwrap();
        assert!(matches!(back, AxMsg::Close(None)));
    }

    #[test]
    fn translate_ping_pong_round_trip() {
        for orig in [
            AxMsg::Ping(vec![1u8, 2, 3].into()),
            AxMsg::Pong(vec![9u8, 8, 7].into()),
        ] {
            let kind = std::mem::discriminant(&orig);
            let t = ax_to_tung(orig);
            let back = tung_to_ax(t).unwrap();
            assert_eq!(std::mem::discriminant(&back), kind);
        }
    }
}
