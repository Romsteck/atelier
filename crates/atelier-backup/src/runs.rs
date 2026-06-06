use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::models::SnapshotResult;
use crate::sqlx::{PgRow, Pool, Postgres, Row, query, query_scalar};

#[derive(Debug, Clone, Serialize)]
pub struct RunSnapshot {
    pub tag: String,
    pub snapshot_id: Option<String>,
    pub status: String,
    pub files: i64,
    pub bytes_processed: i64,
    pub bytes_added: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupRun {
    pub id: Uuid,
    pub trigger: String,
    pub status: String,
    pub phase: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub git_added: Option<i64>,
    pub postgres_added: Option<i64>,
    pub config_added: Option<i64>,
    pub total_added: Option<i64>,
    pub total_processed: Option<i64>,
    pub error: Option<String>,
    pub snapshots: Vec<RunSnapshot>,
}

#[derive(Clone)]
pub struct RunsStore {
    pool: Pool<Postgres>,
}

impl RunsStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    /// Crée la ligne de run avec un id pré-généré (l'appelant l'a déjà réservé
    /// dans le verrou single-flight + le canal d'annulation).
    pub async fn start(&self, id: Uuid, trigger: &str) -> anyhow::Result<()> {
        query("INSERT INTO backup_runs (id, trigger, phase) VALUES ($1, $2, 'repo')")
            .bind(id)
            .bind(trigger)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_phase(&self, id: Uuid, phase: &str) -> anyhow::Result<()> {
        query("UPDATE backup_runs SET phase = $2 WHERE id = $1")
            .bind(id)
            .bind(phase)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Insère/maj le résultat d'un snapshot (tag git/postgres/config).
    pub async fn upsert_snapshot(
        &self,
        run_id: Uuid,
        tag: &str,
        status: &str,
        res: &SnapshotResult,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        query(
            r#"
            INSERT INTO backup_run_snapshots
                (run_id, tag, snapshot_id, status, files, bytes_processed, bytes_added, error)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (run_id, tag) DO UPDATE
               SET snapshot_id = EXCLUDED.snapshot_id, status = EXCLUDED.status,
                   files = EXCLUDED.files, bytes_processed = EXCLUDED.bytes_processed,
                   bytes_added = EXCLUDED.bytes_added, error = EXCLUDED.error
            "#,
        )
        .bind(run_id)
        .bind(tag)
        .bind(res.snapshot_id.as_deref())
        .bind(status)
        .bind(res.files)
        .bind(res.bytes_processed)
        .bind(res.bytes_added)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_success(
        &self,
        id: Uuid,
        git_added: i64,
        postgres_added: i64,
        config_added: i64,
        total_processed: i64,
    ) -> anyhow::Result<()> {
        let total_added = git_added + postgres_added + config_added;
        query(
            r#"
            UPDATE backup_runs
               SET status = 'success', phase = 'done', finished_at = now(),
                   git_added = $2, postgres_added = $3, config_added = $4,
                   total_added = $5, total_processed = $6
             WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(git_added)
        .bind(postgres_added)
        .bind(config_added)
        .bind(total_added)
        .bind(total_processed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_failed(&self, id: Uuid, error: &str) -> anyhow::Result<()> {
        let truncated: String = error.chars().take(2000).collect();
        query(
            "UPDATE backup_runs SET status = 'failed', phase = 'failed', finished_at = now(), error = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(truncated)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_cancelled(&self, id: Uuid) -> anyhow::Result<()> {
        query(
            "UPDATE backup_runs SET status = 'cancelled', phase = 'cancelled', finished_at = now(), error = 'annulé par l''utilisateur' WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Au boot : marque comme échoués les runs restés `running` (process crashé).
    pub async fn sweep_running(&self) -> anyhow::Result<u64> {
        let res = query(
            "UPDATE backup_runs SET status = 'failed', phase = 'failed', finished_at = now(), error = 'interrompu par un redémarrage' WHERE status = 'running'",
        )
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn last_success_at(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        let v: Option<DateTime<Utc>> = query_scalar(
            "SELECT finished_at FROM backup_runs WHERE status = 'success' ORDER BY finished_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        Ok(v)
    }

    pub async fn count(&self) -> anyhow::Result<i64> {
        let n: i64 = query_scalar("SELECT count(*) FROM backup_runs")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    pub async fn list(&self, limit: i64, offset: i64) -> anyhow::Result<Vec<BackupRun>> {
        let limit = limit.clamp(1, 500);
        let offset = offset.max(0);
        let rows: Vec<PgRow> = query(
            r#"
            SELECT id, trigger, status, phase, started_at, finished_at,
                   git_added, postgres_added, config_added, total_added, total_processed, error
              FROM backup_runs
             ORDER BY started_at DESC
             LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let mut runs: Vec<BackupRun> = rows.iter().map(row_to_run).collect::<anyhow::Result<_>>()?;
        let ids: Vec<Uuid> = runs.iter().map(|r| r.id).collect();
        let snaps = self.snapshots_for(&ids).await?;
        for r in &mut runs {
            if let Some(s) = snaps.get(&r.id) {
                r.snapshots = s.clone();
            }
        }
        Ok(runs)
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<BackupRun>> {
        let row: Option<PgRow> = query(
            r#"
            SELECT id, trigger, status, phase, started_at, finished_at,
                   git_added, postgres_added, config_added, total_added, total_processed, error
              FROM backup_runs WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let mut run = row_to_run(&row)?;
        let snaps = self.snapshots_for(&[id]).await?;
        if let Some(s) = snaps.get(&id) {
            run.snapshots = s.clone();
        }
        Ok(Some(run))
    }

    async fn snapshots_for(&self, ids: &[Uuid]) -> anyhow::Result<HashMap<Uuid, Vec<RunSnapshot>>> {
        let mut out: HashMap<Uuid, Vec<RunSnapshot>> = HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let rows: Vec<PgRow> = query(
            r#"
            SELECT run_id, tag, snapshot_id, status, files, bytes_processed, bytes_added, error
              FROM backup_run_snapshots
             WHERE run_id = ANY($1)
             ORDER BY tag
            "#,
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        for row in &rows {
            let run_id: Uuid = row.try_get("run_id")?;
            out.entry(run_id).or_default().push(RunSnapshot {
                tag: row.try_get("tag")?,
                snapshot_id: row.try_get("snapshot_id").ok(),
                status: row.try_get("status")?,
                files: row.try_get("files")?,
                bytes_processed: row.try_get("bytes_processed")?,
                bytes_added: row.try_get("bytes_added")?,
                error: row.try_get("error").ok(),
            });
        }
        Ok(out)
    }
}

fn row_to_run(row: &PgRow) -> anyhow::Result<BackupRun> {
    Ok(BackupRun {
        id: row.try_get("id")?,
        trigger: row.try_get("trigger")?,
        status: row.try_get("status")?,
        phase: row.try_get("phase").ok(),
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at").ok(),
        git_added: row.try_get("git_added").ok(),
        postgres_added: row.try_get("postgres_added").ok(),
        config_added: row.try_get("config_added").ok(),
        total_added: row.try_get("total_added").ok(),
        total_processed: row.try_get("total_processed").ok(),
        error: row.try_get("error").ok(),
        snapshots: Vec::new(),
    })
}
