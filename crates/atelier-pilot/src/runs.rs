use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sqlx::{PgPool, PgRow, Row, query};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogRun {
    pub id: Uuid,
    pub item_id: Option<i64>,
    pub scope: String,
    pub run_kind: String,
    pub trigger: String,
    pub attempt: i32,
    pub engine: String,
    pub model: Option<String>,
    pub phase: String,
    pub status: String,
    pub failure_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub checkpoint_sha: Option<String>,
    pub git_sha_before: Option<String>,
    pub commit_sha: Option<String>,
    pub report: Option<String>,
    pub transcript_tail: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct RunsStore {
    pool: PgPool,
}

impl RunsStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn start(
        &self,
        id: Uuid,
        item_id: Option<i64>,
        scope: &str,
        run_kind: &str,
        trigger: &str,
        attempt: i32,
        engine: &str,
        model: Option<&str>,
    ) -> anyhow::Result<BacklogRun> {
        let row = query(
            r#"INSERT INTO backlog_runs (id,item_id,scope,run_kind,trigger,attempt,engine,model)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING *"#,
        )
        .bind(id)
        .bind(item_id)
        .bind(scope)
        .bind(run_kind)
        .bind(trigger)
        .bind(attempt)
        .bind(engine)
        .bind(model)
        .fetch_one(&self.pool)
        .await?;
        row_to_run(&row)
    }

    pub async fn set_phase(&self, id: Uuid, phase: &str) -> anyhow::Result<()> {
        query("UPDATE backlog_runs SET phase=$2 WHERE id=$1 AND status='running'")
            .bind(id)
            .bind(phase)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_git_state(
        &self,
        id: Uuid,
        checkpoint: Option<&str>,
        before: Option<&str>,
    ) -> anyhow::Result<()> {
        query("UPDATE backlog_runs SET checkpoint_sha=$2,git_sha_before=$3 WHERE id=$1")
            .bind(id)
            .bind(checkpoint)
            .bind(before)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn finish_success(
        &self,
        id: Uuid,
        commit_sha: Option<&str>,
        report: Option<&str>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
    ) -> anyhow::Result<()> {
        query(
            r#"UPDATE backlog_runs SET status='success',phase='report',finished_at=now(),
               commit_sha=$2,report=$3,tokens_in=$4,tokens_out=$5 WHERE id=$1"#,
        )
        .bind(id)
        .bind(commit_sha)
        .bind(report)
        .bind(tokens_in)
        .bind(tokens_out)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_failure(
        &self,
        id: Uuid,
        status: &str,
        reason: &str,
        error: &str,
        report: Option<&str>,
        tail: Option<&str>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
    ) -> anyhow::Result<()> {
        let error: String = error.chars().take(4000).collect();
        let tail: Option<String> = tail.map(|s| {
            s.chars()
                .rev()
                .take(16000)
                .collect::<String>()
                .chars()
                .rev()
                .collect()
        });
        query(
            r#"UPDATE backlog_runs SET status=$2,failure_reason=$3,error=$4,report=$5,
               transcript_tail=$6,tokens_in=$7,tokens_out=$8,finished_at=now() WHERE id=$1"#,
        )
        .bind(id)
        .bind(status)
        .bind(reason)
        .bind(error)
        .bind(report)
        .bind(tail.as_deref())
        .bind(tokens_in)
        .bind(tokens_out)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_for_item(&self, item_id: i64) -> anyhow::Result<Vec<BacklogRun>> {
        let rows = query("SELECT * FROM backlog_runs WHERE item_id=$1 ORDER BY started_at DESC")
            .bind(item_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_run).collect()
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<BacklogRun>> {
        let row = query("SELECT * FROM backlog_runs WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_run).transpose()
    }

    /// Runs encore `running` au boot, hors worker atelier détaché en phase
    /// `report` (qui a sa réconciliation dédiée) : leurs arbres doivent être
    /// restaurés AVANT le fail_stale de `reconcile_interrupted`.
    pub async fn running_orphans(&self) -> anyhow::Result<Vec<BacklogRun>> {
        let rows = query(
            "SELECT * FROM backlog_runs WHERE status='running' AND NOT (scope='atelier' AND phase='report') ORDER BY started_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_run).collect()
    }

    pub async fn reconcile_interrupted(&self) -> anyhow::Result<u64> {
        Ok(query(
            "UPDATE backlog_runs SET status='failed',failure_reason='agent_error',error='interrupted by Atelier restart',finished_at=now() \
             WHERE status='running' AND NOT (scope='atelier' AND phase='report')"
        ).execute(&self.pool).await?.rows_affected())
    }

    pub async fn waiting_atelier(&self) -> anyhow::Result<Option<BacklogRun>> {
        let row = query(
            "SELECT * FROM backlog_runs WHERE status='running' AND scope='atelier' AND phase='report' ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_run).transpose()
    }

    pub async fn prune(&self) -> anyhow::Result<u64> {
        Ok(
            query("DELETE FROM backlog_runs WHERE finished_at < now() - interval '60 days'")
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }
}

fn row_to_run(row: &PgRow) -> anyhow::Result<BacklogRun> {
    Ok(BacklogRun {
        id: row.try_get("id")?,
        item_id: row.try_get("item_id").ok(),
        scope: row.try_get("scope")?,
        run_kind: row.try_get("run_kind")?,
        trigger: row.try_get("trigger")?,
        attempt: row.try_get("attempt")?,
        engine: row.try_get("engine")?,
        model: row.try_get("model").ok(),
        phase: row.try_get("phase")?,
        status: row.try_get("status")?,
        failure_reason: row.try_get("failure_reason").ok(),
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at").ok(),
        tokens_in: row.try_get("tokens_in").ok(),
        tokens_out: row.try_get("tokens_out").ok(),
        checkpoint_sha: row.try_get("checkpoint_sha").ok(),
        git_sha_before: row.try_get("git_sha_before").ok(),
        commit_sha: row.try_get("commit_sha").ok(),
        report: row.try_get("report").ok(),
        transcript_tail: row.try_get("transcript_tail").ok(),
        error: row.try_get("error").ok(),
    })
}
