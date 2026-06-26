//! Low-level HTTP client for Homeroute's EXISTING reverse-proxy API (`hr-api`,
//! default `http://127.0.0.1:4000`, no auth in v1).
//!
//! Endpoints (camelCase wire shape):
//! - `GET    /api/reverseproxy/config` → `{success, config:{baseDomain, hosts:[…]}}`
//! - `POST   /api/reverseproxy/hosts`  → `{success, host:{id, …}}`  (id = uuid v4)
//! - `PUT    /api/reverseproxy/hosts/{id}` (merge) → `{success}`
//! - `DELETE /api/reverseproxy/hosts/{id}` → `{success}`
//! - `POST   /api/reverseproxy/hosts/{id}/toggle` → `{success}`
//!
//! Creating a host makes Homeroute hot-reload the proxy, auto-create the DNS A
//! record, and serve TLS via the pre-provisioned `*.mynetwk.biz` wildcard cert.

use serde::Serialize;
use serde_json::Value;

/// Failure modes mapped by the route layer to 502 (upstream / Homeroute error).
#[derive(Debug)]
pub enum HomerouteError {
    /// reqwest could not reach Homeroute (connection refused, timeout, DNS…).
    Connect(String),
    /// Homeroute answered with a non-2xx status.
    Http(u16, String),
    /// Body was not the expected JSON shape.
    Decode(String),
}

impl std::fmt::Display for HomerouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "homeroute unreachable: {e}"),
            Self::Http(code, body) => write!(f, "homeroute returned HTTP {code}: {body}"),
            Self::Decode(e) => write!(f, "homeroute response decode error: {e}"),
        }
    }
}

/// A reverse-proxy host as seen in Homeroute's live config. Parsed defensively
/// from the raw JSON so one odd host can't break the whole list.
#[derive(Debug, Clone)]
pub struct RpHost {
    pub id: String,
    pub subdomain: Option<String>,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    pub enabled: bool,
    pub require_auth: bool,
}

impl RpHost {
    fn from_value(v: &Value) -> Option<Self> {
        let id = v.get("id")?.as_str()?.to_string();
        let subdomain = v
            .get("subdomain")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let target_host = v
            .get("targetHost")
            .and_then(|x| x.as_str())
            .map(String::from);
        // Homeroute stores the port as a number; tolerate a string just in case.
        let target_port = v.get("targetPort").and_then(|x| {
            x.as_u64()
                .map(|n| n as u16)
                .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
        });
        let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
        let require_auth = v
            .get("requireAuth")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        Some(Self {
            id,
            subdomain,
            target_host,
            target_port,
            enabled,
            require_auth,
        })
    }
}

/// Homeroute reverse-proxy config (the bits Atelier needs).
#[derive(Debug, Clone)]
pub struct RpConfig {
    pub base_domain: String,
    pub hosts: Vec<RpHost>,
}

impl RpConfig {
    /// Find the live host whose `subdomain` matches (the join key Atelier uses).
    pub fn find_by_subdomain(&self, subdomain: &str) -> Option<&RpHost> {
        self.hosts
            .iter()
            .find(|h| h.subdomain.as_deref() == Some(subdomain))
    }
}

/// Body of `POST /api/reverseproxy/hosts` (camelCase).
///
/// `managed_by`/`environment_name` are extra keys Homeroute persists verbatim
/// (its `add_host` does `let mut host = body`) and the proxy/edge ignore — they
/// only flag the host as Atelier-owned so Homeroute's UI can badge + lock it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateHost {
    pub subdomain: String,
    pub target_host: String,
    pub target_port: u16,
    pub require_auth: bool,
    pub enabled: bool,
    /// Always `"atelier"`.
    pub managed_by: String,
    /// This environment's label (never empty — resolved by the service).
    pub environment_name: String,
}

#[derive(Clone)]
pub struct HomerouteClient {
    http: reqwest::Client,
    base_url: String,
    bearer: Option<String>,
}

impl HomerouteClient {
    pub fn new(http: reqwest::Client, base_url: String, bearer: Option<String>) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            bearer,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/reverseproxy{}", self.base_url, path)
    }

    /// Send a request, attach the bearer if configured, and parse the JSON body.
    /// Non-2xx → `Http`, transport failure → `Connect`, bad body → `Decode`.
    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<Value, HomerouteError> {
        let rb = match &self.bearer {
            Some(t) if !t.is_empty() => rb.bearer_auth(t),
            _ => rb,
        };
        let resp = rb
            .send()
            .await
            .map_err(|e| HomerouteError::Connect(e.to_string()))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| HomerouteError::Connect(e.to_string()))?;
        if !status.is_success() {
            return Err(HomerouteError::Http(status.as_u16(), truncate(&body)));
        }
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&body).map_err(|e| HomerouteError::Decode(e.to_string()))
    }

    /// `GET /api/reverseproxy/config` — base domain + live hosts.
    pub async fn get_config(&self) -> Result<RpConfig, HomerouteError> {
        let v = self.send(self.http.get(self.url("/config"))).await?;
        let cfg = v.get("config").unwrap_or(&v);
        let base_domain = cfg
            .get("baseDomain")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let hosts = cfg
            .get("hosts")
            .and_then(|x| x.as_array())
            .map(|arr| arr.iter().filter_map(RpHost::from_value).collect())
            .unwrap_or_default();
        Ok(RpConfig { base_domain, hosts })
    }

    /// `POST /api/reverseproxy/hosts` — returns the new host's uuid.
    pub async fn create_host(&self, body: &CreateHost) -> Result<String, HomerouteError> {
        let v = self
            .send(self.http.post(self.url("/hosts")).json(body))
            .await?;
        v.get("host")
            .and_then(|h| h.get("id"))
            .and_then(|x| x.as_str())
            .map(String::from)
            .ok_or_else(|| HomerouteError::Decode("missing host.id in response".into()))
    }

    /// `PUT /api/reverseproxy/hosts/{id}` — merge-patch the host fields.
    pub async fn update_host(&self, id: &str, patch: &Value) -> Result<(), HomerouteError> {
        self.send(self.http.put(self.url(&format!("/hosts/{id}"))).json(patch))
            .await
            .map(|_| ())
    }

    /// `DELETE /api/reverseproxy/hosts/{id}`.
    pub async fn delete_host(&self, id: &str) -> Result<(), HomerouteError> {
        self.send(self.http.delete(self.url(&format!("/hosts/{id}"))))
            .await
            .map(|_| ())
    }

    /// `POST /api/reverseproxy/hosts/{id}/toggle` — flip the enabled flag.
    pub async fn toggle_host(&self, id: &str) -> Result<(), HomerouteError> {
        self.send(self.http.post(self.url(&format!("/hosts/{id}/toggle"))))
            .await
            .map(|_| ())
    }

    /// `POST /api/atelier/register` — announce this environment (token-gated by
    /// Homeroute ⇒ 401 here if no/invalid bearer). Doubles as a heartbeat.
    pub async fn register(
        &self,
        name: &str,
        url: Option<&str>,
        version: &str,
    ) -> Result<RegisterResult, HomerouteError> {
        let endpoint = format!("{}/api/atelier/register", self.base_url);
        let body = serde_json::json!({ "name": name, "url": url, "version": version });
        let v = self.send(self.http.post(endpoint).json(&body)).await?;
        Ok(RegisterResult {
            environment_id: v
                .get("environment")
                .and_then(|e| e.get("id"))
                .and_then(|x| x.as_str())
                .map(String::from),
            base_domain: v
                .get("baseDomain")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }
}

/// Result of `POST /api/atelier/register`.
#[derive(Debug, Clone)]
pub struct RegisterResult {
    pub environment_id: Option<String>,
    pub base_domain: String,
}

fn truncate(s: &str) -> String {
    const MAX: usize = 300;
    if s.len() > MAX {
        format!("{}…", &s[..MAX])
    } else {
        s.to_string()
    }
}
