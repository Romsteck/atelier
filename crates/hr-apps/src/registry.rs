use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::types::Application;

const REGISTRY_PATH: &str = "/opt/homeroute/data/apps.json";

/// In-memory registry of HomeRoute applications, persisted to a JSON file.
#[derive(Clone)]
pub struct AppRegistry {
    path: PathBuf,
    apps: Arc<RwLock<Vec<Application>>>,
}

impl AppRegistry {
    /// Load the registry from the default path (`/opt/homeroute/data/apps.json`).
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
    pub async fn upsert(&self, mut app: Application) -> Result<()> {
        app.updated_at = Utc::now();
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
}
