use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use atelier_common::control_db::sqlx::{PgPool, PgRow, Row, query};

use crate::types::Application;

/// App registry backed by the `applications` table in `atelier_meta`.
///
/// The full `Application` is stored as JSONB in `data`; `slug`, `port` and
/// `state` are mirrored to columns. The `port` column is authoritative (it is
/// also written directly by [`crate::PortRegistry`]) and overrides the value in
/// `data` on read — this single-row design is what lets the old
/// `reconcile_registries` boot hack be removed.
///
/// An in-memory `Arc<RwLock<Vec<Application>>>` cache serves reads (and survives
/// transient Postgres blips); mutations write through to Postgres then refresh
/// the cache entry.
#[derive(Clone)]
pub struct AppRegistry {
    pool: PgPool,
    apps: Arc<RwLock<Vec<Application>>>,
}

impl AppRegistry {
    /// Build the registry over the shared `atelier_meta` pool and warm the cache
    /// from the `applications` table.
    pub async fn new(pool: PgPool) -> anyhow::Result<Self> {
        let apps = load_all(&pool).await?;
        Ok(Self {
            pool,
            apps: Arc::new(RwLock::new(apps)),
        })
    }

    /// Snapshot all apps (from the in-memory cache).
    pub async fn list(&self) -> Vec<Application> {
        self.apps.read().await.clone()
    }

    /// Get one app by slug (from the in-memory cache).
    pub async fn get(&self, slug: &str) -> Option<Application> {
        self.apps
            .read()
            .await
            .iter()
            .find(|a| a.slug == slug)
            .cloned()
    }

    /// Insert or replace an app: write through to Postgres, then update the cache.
    pub async fn upsert(&self, app: Application) -> anyhow::Result<()> {
        let data = serde_json::to_value(&app)?;
        let port = port_to_col(app.port);
        let state = app.state.as_str();
        query(
            r#"
            INSERT INTO applications (slug, port, state, data, updated_at)
            VALUES ($1, $2, $3, $4, now())
            ON CONFLICT (slug) DO UPDATE SET
                port       = EXCLUDED.port,
                state      = EXCLUDED.state,
                data       = EXCLUDED.data,
                updated_at = now()
            "#,
        )
        .bind(&app.slug)
        .bind(port)
        .bind(state)
        .bind(&data)
        .execute(&self.pool)
        .await?;

        let mut apps = self.apps.write().await;
        if let Some(existing) = apps.iter_mut().find(|a| a.slug == app.slug) {
            *existing = app;
        } else {
            apps.push(app);
        }
        Ok(())
    }

    /// Remove an app by slug. Returns true if a row was deleted.
    pub async fn remove(&self, slug: &str) -> anyhow::Result<bool> {
        let res = query("DELETE FROM applications WHERE slug = $1")
            .bind(slug)
            .execute(&self.pool)
            .await?;
        let removed = res.rows_affected() > 0;
        if removed {
            self.apps.write().await.retain(|a| a.slug != slug);
        }
        Ok(removed)
    }

    /// Reload the cache from Postgres. Used after an external mutation of the
    /// `applications` table (e.g. a direct port assignment) to keep reads fresh.
    pub async fn refresh(&self) -> anyhow::Result<()> {
        let apps = load_all(&self.pool).await?;
        *self.apps.write().await = apps;
        Ok(())
    }
}

async fn load_all(pool: &PgPool) -> anyhow::Result<Vec<Application>> {
    let rows: Vec<PgRow> = query("SELECT port, data FROM applications ORDER BY slug")
        .fetch_all(pool)
        .await?;
    let mut apps = Vec::with_capacity(rows.len());
    for row in &rows {
        match row_to_app(row) {
            Ok(app) => apps.push(app),
            Err(e) => warn!(error = %e, "skipping malformed application row"),
        }
    }
    Ok(apps)
}

/// Decode an `applications` row into an `Application`. The `port` column is
/// authoritative and overrides whatever port is embedded in the JSON blob.
fn row_to_app(row: &PgRow) -> anyhow::Result<Application> {
    let data: serde_json::Value = row.get("data");
    let mut app: Application = serde_json::from_value(data)?;
    let port: Option<i32> = row.get("port");
    app.port = port.map(|p| p as u16).unwrap_or(0);
    Ok(app)
}

/// Map the in-memory sentinel port `0` (unassigned) to a NULL column value.
fn port_to_col(port: u16) -> Option<i32> {
    if port == 0 { None } else { Some(port as i32) }
}
