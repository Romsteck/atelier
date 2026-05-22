use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::types::Application;

// Atelier's canonical registry location post-rapatriement (2026-05-09).
// `main.rs` always passes an explicit path via `load_from`; this default only
// applies to a bare `load()` call.
const REGISTRY_PATH: &str = "/opt/atelier/data/apps.json";

/// In-memory registry of HomeRoute applications, persisted to a JSON file.
#[derive(Clone)]
pub struct AppRegistry {
    path: PathBuf,
    apps: Arc<RwLock<Vec<Application>>>,
}

impl AppRegistry {
    /// Load the registry from the default path (`/opt/atelier/data/apps.json`).
    pub async fn load() -> Result<Self> {
        Self::load_from(PathBuf::from(REGISTRY_PATH)).await
    }

    /// Load the registry from a custom path.
    pub async fn load_from(path: PathBuf) -> Result<Self> {
        let apps: Vec<Application> = if path.exists() {
            let bytes = tokio::fs::read(&path)
                .await
                .with_context(|| format!("reading {}", path.display()))?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))?
            }
        } else {
            warn!(path = %path.display(), "app registry not found, starting empty");
            Vec::new()
        };

        info!(path = %path.display(), count = apps.len(), "AppRegistry loaded");
        Ok(Self {
            path,
            apps: Arc::new(RwLock::new(apps)),
        })
    }

    /// Snapshot the current set of applications.
    pub async fn list(&self) -> Vec<Application> {
        self.apps.read().await.clone()
    }

    /// Look up an application by slug.
    pub async fn get(&self, slug: &str) -> Option<Application> {
        self.apps
            .read()
            .await
            .iter()
            .find(|a| a.slug == slug)
            .cloned()
    }

    /// Insert a new app or replace an existing one with the same slug.
    ///
    /// Defensively provisions the flow callback fields when missing so every
    /// app is daemon-ready by default (Phase 4+, hr-flowd shared daemon) :
    /// - `flow_callback_token` : 32-byte hex generated via `rand::rng()`
    /// - `flow_callback_url`   : `http://127.0.0.1:<port>` for non-NextJS
    ///   stacks ; `http://127.0.0.1:<port>/apps/<slug>` for NextJS (which
    ///   path-routes at `/apps/<slug>` via `next.config.basePath`). Once
    ///   the port is assigned (port==0 keeps the URL absent — token alone
    ///   won't reach the daemon registry until the port is set on a
    ///   follow-up upsert).
    ///
    /// This makes future apps flow-ready without an explicit
    /// `regenerate_flow_token` call. The endpoint remains useful for
    /// rotating an existing token.
    pub async fn upsert(&self, mut app: Application) -> Result<()> {
        app.updated_at = Utc::now();
        if app.flow_callback_token.is_none() {
            app.flow_callback_token = Some(generate_flow_token());
            info!(slug = %app.slug, "AppRegistry: flow_callback_token auto-generated");
        }
        if app.flow_callback_url.is_none() && app.port != 0 {
            app.flow_callback_url = Some(default_callback_url(&app));
        }
        let mut apps = self.apps.write().await;
        let action = if let Some(pos) = apps.iter().position(|a| a.slug == app.slug) {
            apps[pos] = app.clone();
            "updated"
        } else {
            apps.push(app.clone());
            "inserted"
        };
        Self::persist(&self.path, &apps).await?;
        info!(slug = %app.slug, action, "AppRegistry upsert");
        Ok(())
    }

    /// Remove an app by slug. Returns true if an entry was removed.
    pub async fn remove(&self, slug: &str) -> Result<bool> {
        let mut apps = self.apps.write().await;
        let before = apps.len();
        apps.retain(|a| a.slug != slug);
        let removed = apps.len() < before;
        if removed {
            Self::persist(&self.path, &apps).await?;
            info!(slug = %slug, "AppRegistry remove");
        }
        Ok(removed)
    }

    async fn persist(path: &PathBuf, apps: &[Application]) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(apps).context("serializing app registry")?;
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, &json)
            .await
            .with_context(|| format!("writing {}", tmp.display()))?;
        tokio::fs::rename(&tmp, path)
            .await
            .with_context(|| format!("renaming to {}", path.display()))?;
        Ok(())
    }
}

/// Generate a fresh 32-byte hex token suitable for `flow_callback_token`.
/// Public so the Atelier API endpoint can rotate without re-implementing
/// the same crypto.
pub fn generate_flow_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Compute the daemon → app callback URL based on stack conventions.
///
/// NextJS apps use `next.config.basePath = "/apps/<slug>"` so all routes
/// (including the flow catchall `/_flow/...`) are served under that prefix.
/// Rust / axum apps merge their callback router at root, so the URL is
/// just the loopback host:port.
pub fn default_callback_url(app: &crate::types::Application) -> String {
    use crate::types::AppStack;
    match app.stack {
        AppStack::NextJs => format!("http://127.0.0.1:{}/apps/{}", app.port, app.slug),
        _ => format!("http://127.0.0.1:{}", app.port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AppStack;
    use tempfile::TempDir;

    #[tokio::test]
    async fn upsert_get_remove_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("apps.json");
        let reg = AppRegistry::load_from(path.clone()).await.unwrap();

        let app = Application::new("trader".into(), "Trader".into(), AppStack::AxumVite);
        reg.upsert(app).await.unwrap();
        assert!(reg.get("trader").await.is_some());
        assert_eq!(reg.list().await.len(), 1);

        let reg2 = AppRegistry::load_from(path).await.unwrap();
        assert!(reg2.get("trader").await.is_some());

        assert!(reg2.remove("trader").await.unwrap());
        assert!(reg2.get("trader").await.is_none());
    }

    #[tokio::test]
    async fn upsert_auto_provisions_flow_callback_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("apps.json");
        let reg = AppRegistry::load_from(path).await.unwrap();

        // Token only (port=0 → no URL yet)
        let mut app = Application::new("foo".into(), "Foo".into(), AppStack::AxumVite);
        assert!(app.flow_callback_token.is_none());
        assert!(app.flow_callback_url.is_none());
        reg.upsert(app.clone()).await.unwrap();
        let stored = reg.get("foo").await.unwrap();
        assert!(stored.flow_callback_token.is_some());
        assert_eq!(stored.flow_callback_token.as_ref().unwrap().len(), 64); // 32 bytes hex
        assert!(stored.flow_callback_url.is_none());

        // Re-upsert with port → URL gets filled, token preserved (was already set)
        app = stored;
        app.port = 3009;
        reg.upsert(app).await.unwrap();
        let stored2 = reg.get("foo").await.unwrap();
        assert_eq!(
            stored2.flow_callback_url.as_deref(),
            Some("http://127.0.0.1:3009")
        );
    }

    #[tokio::test]
    async fn upsert_nextjs_callback_url_includes_basepath() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("apps.json");
        let reg = AppRegistry::load_from(path).await.unwrap();

        let mut app = Application::new("blog".into(), "Blog".into(), AppStack::NextJs);
        app.port = 3005;
        reg.upsert(app).await.unwrap();
        let stored = reg.get("blog").await.unwrap();
        // NextJS path-routes via basePath=/apps/<slug> — daemon callback URL
        // must include it so /_flow/... resolves on the right route.
        assert_eq!(
            stored.flow_callback_url.as_deref(),
            Some("http://127.0.0.1:3005/apps/blog")
        );
    }
}
