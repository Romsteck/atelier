//! Orchestration layer for the Homeroute reverse-proxy integration.
//!
//! Combines the control-plane store (settings + slug→host mapping) with the
//! HTTP client to Homeroute's hr-api. Homeroute's live config is the source of
//! truth; the stored uuid is a cache that is always re-resolved by `subdomain`
//! before a mutation. Held on `ApiState` (mirrors `BackupService`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use atelier_apps::{AppRegistry, Visibility};
use atelier_common::events::{EventBus, HomerouteRoutesEvent};
use atelier_common::homeroute::{
    FullSettings, HomerouteSettings, HomerouteStore, NewSettings, RouteRow, effective_env_name,
    effective_public_url,
};

use super::homeroute::{CreateHost, HomerouteClient, HomerouteError};

/// Service-layer error, mapped to an HTTP status by the route handlers.
#[derive(Debug)]
pub enum HrServiceError {
    /// Control-plane Postgres unavailable — cannot persist settings/mapping.
    Unavailable,
    /// Integration toggle is off.
    Disabled,
    /// App slug unknown.
    NotFound(String),
    /// App cannot get a subdomain route in v1 (no-strip Next.js basePath).
    Ineligible(String),
    /// Bad input.
    BadRequest(String),
    /// Homeroute returned an error / was unreachable.
    Upstream(String),
    /// Unexpected internal (DB) error.
    Internal(String),
}

impl HrServiceError {
    /// HTTP status code for this error.
    pub fn code(&self) -> u16 {
        match self {
            Self::Unavailable => 503,
            Self::Disabled => 409,
            Self::NotFound(_) => 404,
            Self::Ineligible(_) | Self::BadRequest(_) => 400,
            Self::Upstream(_) => 502,
            Self::Internal(_) => 500,
        }
    }
}

impl std::fmt::Display for HrServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(f, "homeroute integration unavailable (postgres down)"),
            Self::Disabled => write!(
                f,
                "liaison Homeroute non configurée — renseignez le token dans les Paramètres"
            ),
            Self::NotFound(m) | Self::Ineligible(m) | Self::BadRequest(m) | Self::Upstream(m)
            | Self::Internal(m) => write!(f, "{m}"),
        }
    }
}

/// The link is "active" iff a bearer token is configured. No separate flag.
fn linked(s: &FullSettings) -> bool {
    s.bearer_token.as_deref().map(|t| !t.is_empty()).unwrap_or(false)
}

fn upstream(e: HomerouteError) -> HrServiceError {
    HrServiceError::Upstream(e.to_string())
}
fn internal(e: anyhow::Error) -> HrServiceError {
    HrServiceError::Internal(e.to_string())
}

/// Result of `POST /api/homeroute/test`.
#[derive(Debug, Serialize)]
pub struct TestResult {
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of `POST /api/homeroute/register`.
#[derive(Debug, Serialize)]
pub struct RegistrationStatus {
    pub registered: bool,
    pub environment_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
}

/// One app's hostname state in the Settings table.
#[derive(Debug, Serialize)]
pub struct AppRouteView {
    pub slug: String,
    pub name: String,
    pub port: u16,
    pub visibility: String,
    /// False for no-strip apps (Next.js basePath) — can't get a subdomain in v1.
    pub eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ineligible_reason: Option<String>,
    pub assigned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// Live `enabled` flag from Homeroute (None when config not fetched).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_auth: Option<bool>,
    /// True when the live host disagrees with the app's current port / mapping.
    pub drift: bool,
}

/// Response of `GET /api/homeroute/app-routes`.
#[derive(Debug, Serialize)]
pub struct AppRoutesResponse {
    /// True when a bearer token is configured (the link is "active"). There is
    /// no separate enable flag — configured ⇒ active.
    pub linked: bool,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub apps: Vec<AppRouteView>,
}

/// Body of `POST /api/homeroute/app-routes/{slug}`.
#[derive(Debug, Default, Deserialize)]
pub struct AssignBody {
    /// Subdomain to use (defaults to the app slug).
    #[serde(default)]
    pub subdomain: Option<String>,
    /// Whether Homeroute should enforce its forward-auth (default false in v1).
    #[serde(default)]
    pub require_auth: bool,
}

#[derive(Clone)]
pub struct HomerouteService {
    store: HomerouteStore,
    http: reqwest::Client,
    registry: AppRegistry,
    events: Arc<EventBus>,
    /// No-strip app slugs (Next.js basePath) — ineligible for subdomain routing.
    preserve_prefix_slugs: HashSet<String>,
}

impl HomerouteService {
    pub fn new(
        store: HomerouteStore,
        registry: AppRegistry,
        events: Arc<EventBus>,
        preserve_prefix_slugs: HashSet<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .expect("reqwest client");
        Self {
            store,
            http,
            registry,
            events,
            preserve_prefix_slugs,
        }
    }

    /// Whether the control-plane pool is available (gates the endpoints → 503).
    pub fn is_available(&self) -> bool {
        self.store.is_available()
    }

    fn client(&self, settings: &FullSettings) -> HomerouteClient {
        HomerouteClient::new(
            self.http.clone(),
            settings.base_url.clone(),
            settings.bearer_token.clone(),
        )
    }

    async fn full_settings(&self) -> Result<FullSettings, HrServiceError> {
        self.store
            .get_settings_full()
            .await
            .map_err(internal)?
            .ok_or(HrServiceError::Unavailable)
    }

    fn broadcast(&self, slug: &str, action: &str) {
        let _ = self.events.homeroute_routes.send(HomerouteRoutesEvent {
            slug: slug.to_string(),
            action: action.to_string(),
        });
    }

    /// Redacted link settings. `environment_name` / `public_url` are filled with
    /// their effective defaults so the UI always shows concrete values.
    pub async fn settings(&self) -> Result<HomerouteSettings, HrServiceError> {
        let mut s = self
            .store
            .get_settings_redacted()
            .await
            .map_err(internal)?
            .ok_or(HrServiceError::Unavailable)?;
        s.environment_name = Some(effective_env_name(s.environment_name.as_deref()));
        s.public_url = Some(effective_public_url(s.public_url.as_deref()));
        Ok(s)
    }

    /// Register (or refresh) this environment with Homeroute (`POST /api/atelier/register`).
    /// Requires a bearer token configured (the link is "active"). Stamps `registered_at`.
    pub async fn register(&self) -> Result<RegistrationStatus, HrServiceError> {
        let settings = self.full_settings().await?;
        if !linked(&settings) {
            return Err(HrServiceError::BadRequest(
                "token de liaison manquant — générez-le dans Homeroute → Environnements \
                 puis collez-le ici"
                    .into(),
            ));
        }
        let name = effective_env_name(settings.environment_name.as_deref());
        let url = effective_public_url(settings.public_url.as_deref());
        let res = self
            .client(&settings)
            .register(&name, Some(&url), env!("CARGO_PKG_VERSION"))
            .await
            .map_err(upstream)?;
        self.store.touch_registered().await.map_err(internal)?;
        self.broadcast("", "registered");
        Ok(RegistrationStatus {
            registered: true,
            environment_name: name,
            base_domain: Some(res.base_domain).filter(|s| !s.is_empty()),
            environment_id: res.environment_id,
        })
    }

    /// Periodic registration loop (boot + every ~5 min). Silent no-op when the
    /// link is disabled / no token / Postgres down; warns only on real upstream
    /// failures. Spawned from `main` so this environment shows up "online" in
    /// Homeroute's Environnements page.
    pub async fn heartbeat_loop(&self) {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        loop {
            match self.register().await {
                Ok(s) => debug!(env = %s.environment_name, "homeroute heartbeat ok"),
                Err(HrServiceError::Disabled)
                | Err(HrServiceError::BadRequest(_))
                | Err(HrServiceError::Unavailable) => {}
                Err(e) => warn!(error = %e, "homeroute heartbeat failed"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    }

    /// Update link settings (validated). Keeps the bearer token if absent.
    pub async fn set_settings(&self, body: &NewSettings) -> Result<(), HrServiceError> {
        if !self.store.is_available() {
            return Err(HrServiceError::Unavailable);
        }
        body.validate().map_err(HrServiceError::BadRequest)?;
        self.store.upsert_settings(body).await.map_err(internal)?;
        self.broadcast("", "settings");
        Ok(())
    }

    /// Probe the configured Homeroute base URL. Returns Ok with `reachable:false`
    /// (not an error) when unreachable — the UI shows the message inline.
    pub async fn test(&self) -> Result<TestResult, HrServiceError> {
        let settings = self.full_settings().await?;
        match self.client(&settings).get_config().await {
            Ok(cfg) => Ok(TestResult {
                reachable: true,
                base_domain: Some(cfg.base_domain),
                host_count: Some(cfg.hosts.len()),
                error: None,
            }),
            Err(e) => Ok(TestResult {
                reachable: false,
                base_domain: None,
                host_count: None,
                error: Some(e.to_string()),
            }),
        }
    }

    /// List every Atelier app with its hostname state, reconciled against
    /// Homeroute's live config when the link is configured (token present).
    pub async fn list_app_routes(&self) -> Result<AppRoutesResponse, HrServiceError> {
        let settings = self.full_settings().await?;
        let apps = self.registry.list().await;
        let local: HashMap<String, RouteRow> = self
            .store
            .list_routes()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.slug.clone(), r))
            .collect();

        let is_linked = linked(&settings);
        let (reachable, base_domain, cfg, error) = if is_linked {
            match self.client(&settings).get_config().await {
                Ok(c) => (true, Some(c.base_domain.clone()), Some(c), None),
                Err(e) => (false, None, None, Some(e.to_string())),
            }
        } else {
            (false, None, None, None)
        };

        let mut views = Vec::with_capacity(apps.len());
        for app in &apps {
            let eligible = !self.preserve_prefix_slugs.contains(&app.slug);
            let ineligible_reason = (!eligible).then(|| {
                format!(
                    "servie en path (Next.js basePath, no-strip) — accès via /apps/{}/",
                    app.slug
                )
            });
            let local_row = local.get(&app.slug);
            let subdomain = local_row
                .map(|r| r.subdomain.clone())
                .unwrap_or_else(|| app.slug.clone());

            let mut view = AppRouteView {
                slug: app.slug.clone(),
                name: app.name.clone(),
                port: app.port,
                visibility: visibility_str(app.visibility),
                eligible,
                ineligible_reason,
                assigned: local_row.is_some(),
                subdomain: Some(subdomain.clone()),
                hostname: local_row.map(|r| r.hostname.clone()),
                host_id: local_row.map(|r| r.host_id.clone()),
                enabled: None,
                require_auth: local_row.map(|r| r.require_auth),
                drift: false,
            };

            if let Some(cfg) = &cfg {
                match cfg.find_by_subdomain(&subdomain) {
                    Some(host) => {
                        view.assigned = true;
                        view.host_id = Some(host.id.clone());
                        view.hostname = Some(format!("{}.{}", subdomain, cfg.base_domain));
                        view.enabled = Some(host.enabled);
                        view.require_auth = Some(host.require_auth);
                        let port_drift = host.target_port.map(|p| p != app.port).unwrap_or(false);
                        let id_drift = local_row.map(|r| r.host_id != host.id).unwrap_or(false);
                        view.drift = port_drift || id_drift;
                    }
                    None => {
                        // Reachable config but no live host: not assigned. A stale
                        // local mapping is surfaced as drift so the UI can clean it.
                        view.assigned = false;
                        view.hostname = None;
                        view.host_id = None;
                        view.enabled = None;
                        view.drift = local_row.is_some();
                    }
                }
            }

            views.push(view);
        }

        Ok(AppRoutesResponse {
            linked: is_linked,
            reachable,
            base_domain,
            error,
            apps: views,
        })
    }

    /// Assign (or re-sync) a hostname for `slug` — upsert-by-subdomain so a
    /// re-assignment never duplicates a Homeroute host.
    pub async fn assign(
        &self,
        slug: &str,
        body: AssignBody,
    ) -> Result<AppRouteView, HrServiceError> {
        let settings = self.full_settings().await?;
        if !linked(&settings) {
            return Err(HrServiceError::Disabled);
        }
        let app = self
            .registry
            .get(slug)
            .await
            .ok_or_else(|| HrServiceError::NotFound(format!("app not found: {slug}")))?;
        if self.preserve_prefix_slugs.contains(slug) {
            return Err(HrServiceError::Ineligible(format!(
                "app '{slug}' est servie en path (Next.js basePath, no-strip) ; \
                 le routage par sous-domaine n'est pas supporté en v1"
            )));
        }
        let subdomain = body
            .subdomain
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(slug)
            .to_string();
        validate_subdomain(&subdomain)?;
        let require_auth = body.require_auth;
        // Label stamped on the host so Homeroute shows it as Atelier-managed.
        let env_name = effective_env_name(settings.environment_name.as_deref());

        let client = self.client(&settings);
        let cfg = client.get_config().await.map_err(upstream)?;

        let host_id = if let Some(existing) = cfg.find_by_subdomain(&subdomain) {
            client
                .update_host(
                    &existing.id,
                    &serde_json::json!({
                        "targetHost": "127.0.0.1",
                        "targetPort": app.port,
                        "requireAuth": require_auth,
                        "enabled": true,
                        "managedBy": "atelier",
                        "environmentName": env_name,
                    }),
                )
                .await
                .map_err(upstream)?;
            existing.id.clone()
        } else {
            client
                .create_host(&CreateHost {
                    subdomain: subdomain.clone(),
                    target_host: "127.0.0.1".to_string(),
                    target_port: app.port,
                    require_auth,
                    enabled: true,
                    managed_by: "atelier".to_string(),
                    environment_name: env_name.clone(),
                })
                .await
                .map_err(upstream)?
        };

        let hostname = format!("{}.{}", subdomain, cfg.base_domain);
        self.store
            .upsert_route(slug, &host_id, &subdomain, &hostname, app.port, require_auth)
            .await
            .map_err(internal)?;
        self.broadcast(slug, "assigned");

        Ok(AppRouteView {
            slug: slug.to_string(),
            name: app.name.clone(),
            port: app.port,
            visibility: visibility_str(app.visibility),
            eligible: true,
            ineligible_reason: None,
            assigned: true,
            subdomain: Some(subdomain),
            hostname: Some(hostname),
            host_id: Some(host_id),
            enabled: Some(true),
            require_auth: Some(require_auth),
            drift: false,
        })
    }

    /// Remove the hostname for `slug` (re-resolves the uuid by subdomain first).
    pub async fn remove(&self, slug: &str) -> Result<(), HrServiceError> {
        let settings = self.full_settings().await?;
        if !linked(&settings) {
            return Err(HrServiceError::Disabled);
        }
        let subdomain = self.subdomain_for(slug).await?;
        let client = self.client(&settings);
        let cfg = client.get_config().await.map_err(upstream)?;
        if let Some(host) = cfg.find_by_subdomain(&subdomain) {
            client.delete_host(&host.id).await.map_err(upstream)?;
        }
        self.store.delete_route(slug).await.map_err(internal)?;
        self.broadcast(slug, "removed");
        Ok(())
    }

    /// Toggle the live `enabled` flag of `slug`'s hostname.
    pub async fn toggle(&self, slug: &str) -> Result<(), HrServiceError> {
        let settings = self.full_settings().await?;
        if !linked(&settings) {
            return Err(HrServiceError::Disabled);
        }
        let subdomain = self.subdomain_for(slug).await?;
        let client = self.client(&settings);
        let cfg = client.get_config().await.map_err(upstream)?;
        let host = cfg.find_by_subdomain(&subdomain).ok_or_else(|| {
            HrServiceError::BadRequest(format!("app '{slug}' n'a pas de hostname attribué"))
        })?;
        client.toggle_host(&host.id).await.map_err(upstream)?;
        self.broadcast(slug, "toggled");
        Ok(())
    }

    /// Best-effort cleanup when an app is deleted: remove its Homeroute host and
    /// local mapping. Never fails the caller (app deletion must proceed).
    pub async fn cleanup_on_delete(&self, slug: &str) {
        if !self.store.is_available() {
            return;
        }
        let local = match self.store.get_route(slug).await {
            Ok(Some(r)) => r,
            // No local mapping → nothing assigned by Atelier; don't touch Homeroute.
            _ => return,
        };
        if let Ok(Some(settings)) = self.store.get_settings_full().await {
            if linked(&settings) {
                let client = self.client(&settings);
                if let Ok(cfg) = client.get_config().await {
                    if let Some(host) = cfg.find_by_subdomain(&local.subdomain) {
                        if let Err(e) = client.delete_host(&host.id).await {
                            warn!(slug, error = %e, "homeroute: cleanup delete_host failed (non-fatal)");
                        }
                    }
                }
            }
        }
        let _ = self.store.delete_route(slug).await;
        self.broadcast(slug, "removed");
    }

    /// Subdomain a slug maps to: the stored one, else the slug itself.
    async fn subdomain_for(&self, slug: &str) -> Result<String, HrServiceError> {
        Ok(self
            .store
            .get_route(slug)
            .await
            .map_err(internal)?
            .map(|r| r.subdomain)
            .unwrap_or_else(|| slug.to_string()))
    }
}

fn visibility_str(v: Visibility) -> String {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
    }
    .to_string()
}

fn validate_subdomain(s: &str) -> Result<(), HrServiceError> {
    if s.is_empty() || s.len() > 63 {
        return Err(HrServiceError::BadRequest(
            "subdomain doit faire 1..63 caractères".into(),
        ));
    }
    let ok = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-');
    if !ok {
        return Err(HrServiceError::BadRequest(
            "subdomain: lettres/chiffres/tirets uniquement (label DNS)".into(),
        ));
    }
    Ok(())
}
