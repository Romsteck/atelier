use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use atelier_common::control_db::sqlx::{PgPool, PgRow, Row, query};

/// Port registry: maps app slug → stable TCP port.
///
/// Backed by the `port` column of the `applications` table in `atelier_meta`
/// (shared with [`crate::AppRegistry`]). A small in-memory `BTreeMap` cache
/// serves the hot `get`/`snapshot` paths (the proxy resolves a port on every
/// request) and is used to allocate the next free port. Because app + port now
/// live in the same row, the cross-file desync that required the old boot-time
/// `reconcile` pass can no longer happen.
#[derive(Clone)]
pub struct PortRegistry {
    pool: PgPool,
    base_port: u16,
    assignments: Arc<RwLock<BTreeMap<String, u16>>>,
}

impl PortRegistry {
    /// Build over the shared `atelier_meta` pool and warm the cache from the
    /// `applications` table (rows with a non-NULL port).
    pub async fn new(pool: PgPool, base_port: u16) -> anyhow::Result<Self> {
        let assignments = load_assignments(&pool).await?;
        Ok(Self {
            pool,
            base_port,
            assignments: Arc::new(RwLock::new(assignments)),
        })
    }

    /// Get the port for a slug, if assigned (from the cache).
    pub async fn get(&self, slug: &str) -> Option<u16> {
        self.assignments.read().await.get(slug).copied()
    }

    /// Snapshot all port assignments (from the cache).
    pub async fn snapshot(&self) -> BTreeMap<String, u16> {
        self.assignments.read().await.clone()
    }

    /// Assign a port to a slug (idempotent). Returns the existing port if already
    /// assigned, else allocates the first free port ≥ base_port. The assignment
    /// is persisted to `applications.port` when the row exists; otherwise it is
    /// held in the cache and committed when the app row is upserted (which binds
    /// the same port). `UNIQUE(port)` guarantees no two apps share a port.
    pub async fn assign(&self, slug: &str) -> anyhow::Result<u16> {
        {
            let assignments = self.assignments.read().await;
            if let Some(&port) = assignments.get(slug) {
                return Ok(port);
            }
        }
        let mut assignments = self.assignments.write().await;
        if let Some(&port) = assignments.get(slug) {
            return Ok(port);
        }
        let used: std::collections::HashSet<u16> = assignments.values().copied().collect();
        let mut port = self.base_port;
        while used.contains(&port) {
            port = port
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("no free port available"))?;
        }
        // Persist to the row if it already exists (no-op row count otherwise —
        // the port is then committed by the subsequent AppRegistry::upsert).
        query("UPDATE applications SET port = $2, updated_at = now() WHERE slug = $1")
            .bind(slug)
            .bind(port as i32)
            .execute(&self.pool)
            .await?;
        assignments.insert(slug.to_string(), port);
        Ok(port)
    }

    /// Release a slug's port assignment (sets the column to NULL).
    pub async fn release(&self, slug: &str) -> anyhow::Result<()> {
        let mut assignments = self.assignments.write().await;
        if assignments.remove(slug).is_some() {
            query("UPDATE applications SET port = NULL, updated_at = now() WHERE slug = $1")
                .bind(slug)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
}

async fn load_assignments(pool: &PgPool) -> anyhow::Result<BTreeMap<String, u16>> {
    let rows: Vec<PgRow> = query("SELECT slug, port FROM applications WHERE port IS NOT NULL")
        .fetch_all(pool)
        .await?;
    let mut map = BTreeMap::new();
    for row in &rows {
        let slug: String = row.get("slug");
        let port: i32 = row.get("port");
        map.insert(slug, port as u16);
    }
    Ok(map)
}
