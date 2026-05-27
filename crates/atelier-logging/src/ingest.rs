use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::migration::{self, drop_partitions_before, ensure_partition};
use crate::query::{self, LogQuery, LogStats};
use crate::ring_buffer::RingBuffer;
use crate::sqlx::{Pool, Postgres, query_as};
use crate::store;
use crate::types::{LogEntry, LogEntryBuilder, RawIngestEntry};

#[derive(Clone)]
pub struct LogIngestService {
    inner: Arc<Inner>,
}

struct Inner {
    ring: RingBuffer,
    writer_pool: Option<Pool<Postgres>>,
    next_id: AtomicI64,
    log_tx: broadcast::Sender<LogEntry>,
    flush_interval: Duration,
    batch_limit: usize,
    retention_days: u32,
    enabled: bool,
}

#[derive(Debug, Clone)]
pub struct LogIngestConfig {
    pub admin_dsn: Option<String>,         // postgres:// .../postgres (super-user) — used to create DB+role
    pub writer_dsn: Option<String>,        // postgres:// .../atelier_logs (least-priv writer)
    pub flush_interval: Duration,
    pub batch_limit: usize,
    pub ring_capacity: usize,
    pub retention_days: u32,
    pub create_database: bool,             // attempt CREATE DATABASE on bootstrap
    pub create_writer_role: bool,          // attempt CREATE ROLE on bootstrap
    pub writer_role: String,
    pub writer_password: Option<String>,
    pub db_name: String,                    // typically "atelier_logs"
}

impl Default for LogIngestConfig {
    fn default() -> Self {
        Self {
            admin_dsn: None,
            writer_dsn: None,
            flush_interval: Duration::from_secs(3),
            batch_limit: 500,
            ring_capacity: 10_000,
            retention_days: 365,
            create_database: true,
            create_writer_role: true,
            writer_role: "atelier_logs_writer".to_string(),
            writer_password: None,
            db_name: "atelier_logs".to_string(),
        }
    }
}

impl LogIngestService {
    /// Bootstraps the service. If Postgres is unreachable or DSN missing,
    /// returns a noop service that still accepts pushes (drop-on-full ring)
    /// but does not persist or stream.
    pub async fn start(cfg: LogIngestConfig) -> Self {
        let (tx, _rx) = broadcast::channel::<LogEntry>(512);

        let writer_pool = match bootstrap(&cfg).await {
            Ok(p) => Some(p),
            Err(err) => {
                warn!(?err, "atelier-logging: bootstrap failed — running in noop mode");
                None
            }
        };

        let enabled = writer_pool.is_some();
        let inner = Arc::new(Inner {
            ring: RingBuffer::new(cfg.ring_capacity),
            writer_pool,
            next_id: AtomicI64::new(1),
            log_tx: tx,
            flush_interval: cfg.flush_interval,
            batch_limit: cfg.batch_limit,
            retention_days: cfg.retention_days,
            enabled,
        });

        let svc = Self { inner: inner.clone() };
        if enabled {
            tokio::spawn(svc.clone().flush_loop());
            tokio::spawn(svc.clone().retention_loop());
            info!("atelier-logging: started (writer connected)");
        }
        svc
    }

    pub fn is_enabled(&self) -> bool { self.inner.enabled }

    /// Hot push from in-process tracing Layer. Sync, never blocks.
    pub fn push(&self, builder: LogEntryBuilder) {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = LogEntry {
            id,
            timestamp: builder.timestamp,
            service: builder.service,
            app_slug: builder.app_slug,
            level: builder.level,
            category: builder.category,
            message: builder.message,
            fields: builder.fields,
            request_id: builder.request_id,
            user_id: builder.user_id,
            source: builder.source,
            app_version: builder.app_version,
            deploy_id: builder.deploy_id,
        };
        // Broadcast first (cheap, non-blocking) so WebSocket subscribers see
        // the event even if the ring is full and we are about to drop oldest.
        let _ = self.inner.log_tx.send(entry.clone());
        self.inner.ring.push(entry);
    }

    /// Batch ingest from HTTP endpoint.
    pub async fn ingest_batch(&self, default_service: &str, entries: Vec<RawIngestEntry>) -> usize {
        let now = Utc::now();
        for raw in entries.iter().cloned() {
            self.push(raw.into_builder(default_service, now));
        }
        entries.len()
    }

    /// Subscribe to live broadcast (used by WebSocket handler).
    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.inner.log_tx.subscribe()
    }

    pub async fn query(&self, q: &LogQuery) -> anyhow::Result<Vec<LogEntry>> {
        match &self.inner.writer_pool {
            Some(pool) => query::query_logs(pool, q).await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn stats(&self, q: &LogQuery) -> anyhow::Result<LogStats> {
        match &self.inner.writer_pool {
            Some(pool) => query::stats(pool, q).await,
            None => Ok(LogStats { total: 0, by_level: vec![], by_service: vec![], by_app: vec![] }),
        }
    }

    pub async fn by_request(&self, rid: &str) -> anyhow::Result<Vec<LogEntry>> {
        match &self.inner.writer_pool {
            Some(pool) => query::by_request(pool, rid).await,
            None => Ok(Vec::new()),
        }
    }

    /// Force a single drain+insert pass. Returns how many were inserted.
    pub async fn flush_once(&self) -> anyhow::Result<usize> {
        let Some(pool) = self.inner.writer_pool.as_ref() else { return Ok(0); };
        let batch = self.inner.ring.drain(self.inner.batch_limit);
        if batch.is_empty() {
            return Ok(0);
        }
        let n = store::insert_batch(pool, &batch).await?;
        debug!(n, "atelier-logging: flushed batch");
        Ok(n)
    }

    async fn flush_loop(self) {
        let mut tick = tokio::time::interval(self.inner.flush_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(err) = self.flush_once().await {
                warn!(?err, "atelier-logging: flush failed");
            }
        }
    }

    async fn retention_loop(self) {
        let Some(pool) = self.inner.writer_pool.clone() else { return; };
        let mut tick = tokio::time::interval(Duration::from_secs(6 * 3600));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let today = Utc::now().date_naive();
            // Ensure today + 2 days ahead
            for offset in 0..=2 {
                let d = today + chrono::Duration::days(offset);
                if let Err(err) = ensure_partition(&pool, d).await {
                    warn!(?err, date = %d, "ensure_partition failed");
                }
            }
            // Drop partitions older than retention window
            let cutoff = today - chrono::Duration::days(self.inner.retention_days as i64);
            match drop_partitions_before(&pool, cutoff).await {
                Ok(0) => {}
                Ok(n) => info!(n, cutoff = %cutoff, "atelier-logging: dropped old partitions"),
                Err(err) => warn!(?err, "drop_partitions_before failed"),
            }
        }
    }
}

async fn bootstrap(cfg: &LogIngestConfig) -> anyhow::Result<Pool<Postgres>> {
    // Step 1: if admin_dsn is provided, create DB + role (idempotent).
    if let Some(admin_dsn) = cfg.admin_dsn.as_deref() {
        let admin_pool = migration::open_admin_pool(admin_dsn).await?;
        if cfg.create_database {
            migration::ensure_database(&admin_pool, &cfg.db_name).await?;
        }
        if cfg.create_writer_role {
            if let Some(pwd) = cfg.writer_password.as_deref() {
                migration::ensure_writer_role(&admin_pool, &cfg.writer_role, pwd).await?;
            }
        }
    }

    // Step 2: connect to target DB (as admin first, to run DDL).
    let target_admin_dsn = match cfg.admin_dsn.as_deref() {
        Some(dsn) => swap_db(dsn, &cfg.db_name),
        None => {
            // No admin DSN — assume writer_dsn is owner of the DB
            cfg.writer_dsn.clone().ok_or_else(|| anyhow::anyhow!("no writer_dsn"))?
        }
    };
    let ddl_pool = migration::open_admin_pool(&target_admin_dsn).await?;
    migration::run_migrations(&ddl_pool).await?;

    // Step 3: open writer pool
    let writer_dsn = cfg
        .writer_dsn
        .clone()
        .unwrap_or_else(|| target_admin_dsn.clone());
    let writer_pool = migration::open_writer_pool(&writer_dsn).await?;

    // Recover next_id from MAX(id) — best effort, ignore errors at boot.
    let max_id: Option<i64> = query_as::<_, (Option<i64>,)>("SELECT MAX(id) FROM events_log")
        .fetch_one(&writer_pool)
        .await
        .map(|(v,)| v)
        .unwrap_or(None);
    if let Some(n) = max_id {
        info!(max_id = n, "atelier-logging: recovered next_id");
    }

    Ok(writer_pool)
}

/// Swap the database segment of a Postgres DSN. Handles both
/// `postgres://user:pass@host:port/db` and ? trailing params.
fn swap_db(dsn: &str, dbname: &str) -> String {
    if let Some((head, tail)) = dsn.rsplit_once('/') {
        // tail may be "olddb" or "olddb?param=val"
        let (_, after) = tail.split_once('?').map(|(a, b)| (a, format!("?{}", b))).unwrap_or((tail, String::new()));
        format!("{}/{}{}", head, dbname, after)
    } else {
        dsn.to_string()
    }
}

