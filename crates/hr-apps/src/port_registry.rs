use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

const REGISTRY_PATH: &str = "/opt/homeroute/data/port-registry.json";
const BASE_PORT: u16 = 3001;

#[derive(Debug, Serialize, Deserialize)]
struct RegistryFile {
    base_port: u16,
    assignments: BTreeMap<String, u16>,
}

/// Persistent map of `app_slug -> port`. Assignments are stable across restarts.
#[derive(Clone)]
pub struct PortRegistry {
    path: PathBuf,
    base_port: u16,
    assignments: Arc<RwLock<BTreeMap<String, u16>>>,
}

impl PortRegistry {
    /// Load the registry from the default path (`/opt/homeroute/data/port-registry.json`).
    pub async fn load() -> Result<Self> {
        Self::load_from(PathBuf::from(REGISTRY_PATH), BASE_PORT).await
    }

    /// Load the registry from a custom path with a custom base port.
    pub async fn load_from(path: PathBuf, base_port: u16) -> Result<Self> {
        let (base_port, assignments) = match tokio::fs::read(&path).await {
            Ok(bytes) if bytes.is_empty() => (base_port, BTreeMap::new()),
            Ok(bytes) => {
                let file: RegistryFile = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))?;
                (file.base_port, file.assignments)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(path = %path.display(), "port registry not found, starting empty");
                (base_port, BTreeMap::new())
            }
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", path.display()));
            }
        };

        info!(
            path = %path.display(),
            base_port,
            count = assignments.len(),
            "PortRegistry loaded"
        );
        Ok(Self {
            path,
            base_port,
            assignments: Arc::new(RwLock::new(assignments)),
        })
    }

    /// Idempotently assign a port to a slug. Existing slugs return their previous port.
    pub async fn assign(&self, slug: &str) -> Result<u16> {
        let mut assignments = self.assignments.write().await;
        if let Some(&port) = assignments.get(slug) {
            return Ok(port);
        }

        let used: HashSet<u16> = assignments.values().copied().collect();
        let mut next = self.base_port;
        let port = loop {
            if !used.contains(&next) {
                break next;
            }
            next = next
                .checked_add(1)
                .context("port registry exhausted (u16 overflow)")?;
        };

        assignments.insert(slug.to_string(), port);
        Self::persist(&self.path, self.base_port, &assignments).await?;
        info!(slug = %slug, port, "PortRegistry assign");
        Ok(port)
    }

    /// Release the port assigned to a slug, if any.
    pub async fn release(&self, slug: &str) -> Result<()> {
        let mut assignments = self.assignments.write().await;
        if assignments.remove(slug).is_some() {
            Self::persist(&self.path, self.base_port, &assignments).await?;
            info!(slug = %slug, "PortRegistry release");
        }
        Ok(())
    }

    /// Get the assigned port for a slug, without creating one.
    pub async fn get(&self, slug: &str) -> Option<u16> {
        self.assignments.read().await.get(slug).copied()
    }

    /// Snapshot the full assignment map.
    pub async fn snapshot(&self) -> BTreeMap<String, u16> {
        self.assignments.read().await.clone()
    }

    async fn persist(
        path: &PathBuf,
        base_port: u16,
        assignments: &BTreeMap<String, u16>,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let file = RegistryFile {
            base_port,
            assignments: assignments.clone(),
        };
        let json = serde_json::to_string_pretty(&file).context("serializing port registry")?;
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
    use tempfile::TempDir;

    #[tokio::test]
    async fn assign_is_idempotent_and_persistent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ports.json");

        let reg = PortRegistry::load_from(path.clone(), 3001).await.unwrap();
        let p1 = reg.assign("alpha").await.unwrap();
        let p2 = reg.assign("beta").await.unwrap();
        let p1_again = reg.assign("alpha").await.unwrap();
        assert_eq!(p1, 3001);
        assert_eq!(p2, 3002);
        assert_eq!(p1, p1_again);

        let reg2 = PortRegistry::load_from(path, 3001).await.unwrap();
        assert_eq!(reg2.get("alpha").await, Some(3001));
        assert_eq!(reg2.get("beta").await, Some(3002));
    }

    #[tokio::test]
    async fn release_frees_port_for_reuse() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ports.json");
        let reg = PortRegistry::load_from(path, 3001).await.unwrap();

        reg.assign("alpha").await.unwrap();
        let beta = reg.assign("beta").await.unwrap();
        assert_eq!(beta, 3002);

        reg.release("alpha").await.unwrap();
        assert_eq!(reg.get("alpha").await, None);

        let gamma = reg.assign("gamma").await.unwrap();
        assert_eq!(gamma, 3001);
    }
}
