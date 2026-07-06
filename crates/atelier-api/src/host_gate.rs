//! Host-gate middleware — serves apps on their dedicated public hostnames.
//!
//! An assigned hostname (`{sub}.mynetwk.biz`, created via the Homeroute
//! integration) targets Atelier's own HTTP port, NOT the app port: the apps
//! are built with the absolute base `/apps/{slug}/` (Vite base, PWA scope,
//! service-worker registration, router basename), so they only work when the
//! browser URL lives under that exact path — which this gate guarantees:
//!
//! - `/apps/{slug}[/...]` → pass through (the existing path-proxy does the
//!   strip/no-strip proxying, unchanged);
//! - any other `/apps/...` → 404 (a public app hostname must not expose the
//!   OTHER apps);
//! - everything else (`/`, SPA deep-links, `/api/...`) → 307 to
//!   `/apps/{slug}{path}` — the Atelier API/UI/Studio/MCP are therefore
//!   unreachable through app hostnames (reduced public surface).
//!
//! Requests whose host is not an assigned hostname (atelier.mynetwk.biz, LAN
//! `127.0.0.1:4100`, …) are untouched.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};

use crate::state::ApiState;

/// Effective public host of the request: `X-Forwarded-Host` (set by hr-proxy
/// with the host the client actually used — the `Host` it forwards is the
/// upstream target) with `Host` as the direct-access fallback. First entry if
/// comma-separated, port stripped (IPv6 literals handled), lowercased.
fn effective_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))?
        .to_str()
        .ok()?;
    let first = raw.split(',').next()?.trim();
    let no_port = if let Some(rest) = first.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        first.split(':').next().unwrap_or(first)
    };
    if no_port.is_empty() {
        return None;
    }
    Some(no_port.to_ascii_lowercase())
}

/// Self-destructing service worker served at the ROOT SW paths of gated hosts.
///
/// WHY: before path-routing (pre 2026-05), `{slug}.mynetwk.biz` served the apps
/// at `/` with a PWA service worker registered at scope `/`. Browsers that
/// visited back then still hold that SW: it intercepts every navigation on the
/// origin and serves the long-gone build from its cache — the site looks broken
/// forever, network never consulted. The SW *update* fetch, however, always
/// bypasses the old SW: by serving this killer at the same script URL, the
/// zombie updates itself into a no-op that unregisters and reloads its tabs
/// (GoogleChromeLabs "self-destroying service worker" pattern). The app's REAL
/// SW lives at `/apps/{slug}/sw.js` (scope `/apps/{slug}/`) and is not touched.
/// A SW script response must be 200 (redirects are rejected), hence this
/// special case instead of the 307.
const SW_KILLER_JS: &str = "\
self.addEventListener('install', () => { self.skipWaiting(); });\n\
self.addEventListener('activate', () => {\n\
  // Purge Cache Storage too: the broken 06-25→07-06 window let old runtime\n\
  // caches store text/html under asset URLs — unregistering alone would leave\n\
  // that poison behind for any future SW on the origin.\n\
  caches.keys()\n\
    .then((keys) => Promise.all(keys.map((k) => caches.delete(k))))\n\
    .then(() => self.registration.unregister())\n\
    .then(() => self.clients.matchAll({ type: 'window' }))\n\
    .then((clients) => clients.forEach((c) => c.navigate(c.url)));\n\
});\n";

/// Routing decision for a request on a gated host. Pure — unit-tested below.
#[derive(Debug, PartialEq)]
enum Gate {
    Pass,
    NotFound,
    Redirect(String),
    KillerSw,
}

fn decide(slug: &str, path: &str, query: Option<&str>) -> Gate {
    let prefix = format!("/apps/{slug}");
    if path == prefix || path.starts_with(&format!("{prefix}/")) {
        return Gate::Pass;
    }
    if path == "/apps" || path.starts_with("/apps/") {
        return Gate::NotFound;
    }
    // Legacy root-scope SW script URLs (vite-plugin-pwa / CRA conventions).
    if path == "/sw.js" || path == "/service-worker.js" {
        return Gate::KillerSw;
    }
    let location = match query {
        Some(q) => format!("{prefix}{path}?{q}"),
        None => format!("{prefix}{path}"),
    };
    Gate::Redirect(location)
}

pub async fn host_gate(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    let Some(host) = effective_host(req.headers()) else {
        return next.run(req).await;
    };
    let Some(slug) = state.homeroute.slug_for_host(&host) else {
        return next.run(req).await;
    };
    match decide(&slug, req.uri().path(), req.uri().query()) {
        Gate::Pass => next.run(req).await,
        Gate::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
        // 307: preserves method+body (app API POSTs) and is not permanently
        // cached by browsers (308 would survive a hostname re-assignment).
        Gate::Redirect(location) => Redirect::temporary(&location).into_response(),
        // no-store: the update check must always fetch the current script (a
        // cached killer would keep re-installing itself).
        Gate::KillerSw => (
            StatusCode::OK,
            [
                ("content-type", "text/javascript"),
                ("cache-control", "no-store"),
            ],
            SW_KILLER_JS,
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn effective_host_prefers_x_forwarded_host() {
        let h = headers(&[("host", "127.0.0.1:4100"), ("x-forwarded-host", "myfrigo.mynetwk.biz")]);
        assert_eq!(effective_host(&h).as_deref(), Some("myfrigo.mynetwk.biz"));
    }

    #[test]
    fn effective_host_falls_back_to_host() {
        let h = headers(&[("host", "wallet.mynetwk.biz")]);
        assert_eq!(effective_host(&h).as_deref(), Some("wallet.mynetwk.biz"));
    }

    #[test]
    fn effective_host_strips_port_and_lowercases() {
        let h = headers(&[("x-forwarded-host", "WALLET.MYNETWK.BIZ:443")]);
        assert_eq!(effective_host(&h).as_deref(), Some("wallet.mynetwk.biz"));
    }

    #[test]
    fn effective_host_takes_first_of_list() {
        let h = headers(&[("x-forwarded-host", "a.example.com, b.example.com")]);
        assert_eq!(effective_host(&h).as_deref(), Some("a.example.com"));
    }

    #[test]
    fn effective_host_handles_ipv6_literal() {
        let h = headers(&[("host", "[::1]:4100")]);
        assert_eq!(effective_host(&h).as_deref(), Some("::1"));
    }

    #[test]
    fn effective_host_none_when_absent() {
        assert_eq!(effective_host(&HeaderMap::new()), None);
    }

    #[test]
    fn decide_passes_app_prefix() {
        assert_eq!(decide("myfrigo", "/apps/myfrigo", None), Gate::Pass);
        assert_eq!(decide("myfrigo", "/apps/myfrigo/", None), Gate::Pass);
        assert_eq!(
            decide("myfrigo", "/apps/myfrigo/assets/index-abc.js", None),
            Gate::Pass
        );
    }

    #[test]
    fn decide_blocks_other_apps_and_bare_apps() {
        assert_eq!(decide("myfrigo", "/apps/wallet/", None), Gate::NotFound);
        // Sibling slug sharing the prefix as a string must NOT pass.
        assert_eq!(decide("myfrigo", "/apps/myfrigo2/x", None), Gate::NotFound);
        assert_eq!(decide("myfrigo", "/apps", None), Gate::NotFound);
        assert_eq!(decide("myfrigo", "/apps/", None), Gate::NotFound);
    }

    #[test]
    fn decide_redirects_root_and_deep_links() {
        assert_eq!(
            decide("myfrigo", "/", None),
            Gate::Redirect("/apps/myfrigo/".into())
        );
        assert_eq!(
            decide("myfrigo", "/recettes/42", None),
            Gate::Redirect("/apps/myfrigo/recettes/42".into())
        );
        assert_eq!(
            decide("myfrigo", "/api/items", Some("tab=a&x=1")),
            Gate::Redirect("/apps/myfrigo/api/items?tab=a&x=1".into())
        );
    }

    #[test]
    fn decide_serves_killer_sw_at_legacy_root_paths() {
        assert_eq!(decide("myfrigo", "/sw.js", None), Gate::KillerSw);
        assert_eq!(decide("myfrigo", "/service-worker.js", None), Gate::KillerSw);
        // The app's real SW under its prefix is untouched.
        assert_eq!(decide("myfrigo", "/apps/myfrigo/sw.js", None), Gate::Pass);
    }

    #[test]
    fn decide_shields_atelier_surface() {
        assert_eq!(
            decide("wallet", "/api/apps", None),
            Gate::Redirect("/apps/wallet/api/apps".into())
        );
        assert_eq!(
            decide("wallet", "/studio/wallet", None),
            Gate::Redirect("/apps/wallet/studio/wallet".into())
        );
        assert_eq!(
            decide("wallet", "/mcp", None),
            Gate::Redirect("/apps/wallet/mcp".into())
        );
    }
}
