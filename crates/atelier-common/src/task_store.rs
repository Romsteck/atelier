use tracing::error;

use crate::control_db::sqlx::{PgPool, Pool, Postgres, Row, query};
use crate::tasks::*;

/// Persistent task storage backed by the shared `atelier_meta` Postgres pool.
///
/// When `pool` is `None` (Postgres not configured / unreachable at boot) every
/// operation degrades to a best-effort no-op — mirroring the previous
/// error-swallowing behaviour so callers never need to handle storage errors.
pub struct TaskStore {
    pool: Option<Pool<Postgres>>,
}

impl TaskStore {
    /// Build the store over the shared control-plane pool and run crash
    /// recovery (orphaned `pending`/`running` tasks → `failed`).
    pub async fn new(pool: Option<PgPool>) -> Self {
        let store = Self { pool };
        if let Some(p) = store.pool.as_ref() {
            if let Err(e) = query(
                "UPDATE tasks SET status = 'failed', error = 'Interrupted by restart', \
                 finished_at = now() WHERE status IN ('pending', 'running')",
            )
            .execute(p)
            .await
            {
                error!("Failed to recover orphaned tasks: {}", e);
            }
        }
        store
    }

    fn pool(&self) -> Option<&Pool<Postgres>> {
        self.pool.as_ref()
    }

    // ── CRUD Tasks ──

    pub async fn create_task(
        &self,
        task_type: TaskType,
        title: &str,
        trigger: TaskTrigger,
        target: Option<&str>,
    ) -> Task {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let task_type_str = task_type.to_string();
        let (trigger_type, trigger_info) = match &trigger {
            TaskTrigger::User(u) => ("user".to_string(), Some(u.clone())),
            TaskTrigger::System => ("system".to_string(), None),
            TaskTrigger::Api => ("api".to_string(), None),
        };

        let task = Task {
            id: id.clone(),
            task_type,
            title: title.to_string(),
            status: TaskStatus::Pending,
            trigger,
            target: target.map(String::from),
            created_at: now,
            started_at: None,
            finished_at: None,
            error: None,
        };

        if let Some(p) = self.pool() {
            if let Err(e) = query(
                "INSERT INTO tasks (id, task_type, title, status, trigger_type, trigger_info, target, created_at) \
                 VALUES ($1, $2, $3, 'pending', $4, $5, $6, $7)",
            )
            .bind(&id)
            .bind(&task_type_str)
            .bind(title)
            .bind(&trigger_type)
            .bind(&trigger_info)
            .bind(target)
            .bind(now)
            .execute(p)
            .await
            {
                error!("Failed to create task: {}", e);
            }
        }

        task
    }

    pub async fn update_task_status(&self, id: &str, status: TaskStatus, error_msg: Option<&str>) {
        let Some(p) = self.pool() else { return };
        let now = chrono::Utc::now();
        let status_str = status.to_string();

        let result = match status {
            TaskStatus::Running => {
                query("UPDATE tasks SET status = $2, started_at = $3 WHERE id = $1")
                    .bind(id)
                    .bind(&status_str)
                    .bind(now)
                    .execute(p)
                    .await
            }
            TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => {
                query("UPDATE tasks SET status = $2, finished_at = $3, error = $4 WHERE id = $1")
                    .bind(id)
                    .bind(&status_str)
                    .bind(now)
                    .bind(error_msg)
                    .execute(p)
                    .await
            }
            _ => {
                query("UPDATE tasks SET status = $2 WHERE id = $1")
                    .bind(id)
                    .bind(&status_str)
                    .execute(p)
                    .await
            }
        };

        if let Err(e) = result {
            error!("Failed to update task status: {}", e);
        }
    }

    pub async fn get_task(&self, id: &str) -> Option<Task> {
        let p = self.pool()?;
        match query(
            "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error \
             FROM tasks WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(p)
        .await
        {
            Ok(row) => row.map(|r| row_to_task(&r)),
            Err(e) => {
                error!("Failed to get task: {}", e);
                None
            }
        }
    }

    pub async fn list_tasks(
        &self,
        limit: u32,
        offset: u32,
        status: Option<&str>,
    ) -> (Vec<Task>, u32) {
        let Some(p) = self.pool() else {
            return (vec![], 0);
        };

        let total: i64 = if let Some(s) = status {
            match query("SELECT COUNT(*) FROM tasks WHERE status = $1")
                .bind(s)
                .fetch_one(p)
                .await
            {
                Ok(row) => row.get::<i64, _>(0),
                Err(_) => 0,
            }
        } else {
            match query("SELECT COUNT(*) FROM tasks").fetch_one(p).await {
                Ok(row) => row.get::<i64, _>(0),
                Err(_) => 0,
            }
        };

        let rows = if let Some(s) = status {
            query(
                "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error \
                 FROM tasks WHERE status = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            )
            .bind(s)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(p)
            .await
        } else {
            query(
                "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error \
                 FROM tasks ORDER BY created_at DESC LIMIT $1 OFFSET $2",
            )
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(p)
            .await
        };

        match rows {
            Ok(rows) => (rows.iter().map(row_to_task).collect(), total as u32),
            Err(e) => {
                error!("Failed to list tasks: {}", e);
                (vec![], 0)
            }
        }
    }

    pub async fn get_active_tasks(&self) -> Vec<Task> {
        let Some(p) = self.pool() else {
            return vec![];
        };
        match query(
            "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error \
             FROM tasks WHERE status IN ('pending', 'running') ORDER BY created_at DESC",
        )
        .fetch_all(p)
        .await
        {
            Ok(rows) => rows.iter().map(row_to_task).collect(),
            Err(e) => {
                error!("Failed to get active tasks: {}", e);
                vec![]
            }
        }
    }

    // ── CRUD Steps ──

    pub async fn create_step(&self, task_id: &str, name: &str, message: &str) -> TaskStep {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();

        let step = TaskStep {
            id: id.clone(),
            task_id: task_id.to_string(),
            step_name: name.to_string(),
            status: TaskStatus::Running,
            started_at: now,
            finished_at: None,
            message: Some(message.to_string()),
            details: None,
        };

        if let Some(p) = self.pool() {
            if let Err(e) = query(
                "INSERT INTO task_steps (id, task_id, step_name, status, started_at, message) \
                 VALUES ($1, $2, $3, 'running', $4, $5)",
            )
            .bind(&id)
            .bind(task_id)
            .bind(name)
            .bind(now)
            .bind(message)
            .execute(p)
            .await
            {
                error!("Failed to create step: {}", e);
            }
        }

        step
    }

    pub async fn complete_step(&self, id: &str) {
        let Some(p) = self.pool() else { return };
        if let Err(e) = query("UPDATE task_steps SET status = 'done', finished_at = $2 WHERE id = $1")
            .bind(id)
            .bind(chrono::Utc::now())
            .execute(p)
            .await
        {
            error!("Failed to complete step: {}", e);
        }
    }

    pub async fn fail_step(&self, id: &str, error_msg: &str) {
        let Some(p) = self.pool() else { return };
        if let Err(e) =
            query("UPDATE task_steps SET status = 'failed', finished_at = $2, message = $3 WHERE id = $1")
                .bind(id)
                .bind(chrono::Utc::now())
                .bind(error_msg)
                .execute(p)
                .await
        {
            error!("Failed to fail step: {}", e);
        }
    }

    pub async fn update_step(&self, id: &str, message: &str, details: Option<serde_json::Value>) {
        let Some(p) = self.pool() else { return };
        if let Err(e) = query("UPDATE task_steps SET message = $2, details = $3 WHERE id = $1")
            .bind(id)
            .bind(message)
            .bind(details)
            .execute(p)
            .await
        {
            error!("Failed to update step: {}", e);
        }
    }

    pub async fn get_steps(&self, task_id: &str) -> Vec<TaskStep> {
        let Some(p) = self.pool() else {
            return vec![];
        };
        match query(
            "SELECT id, task_id, step_name, status, started_at, finished_at, message, details \
             FROM task_steps WHERE task_id = $1 ORDER BY started_at ASC",
        )
        .bind(task_id)
        .fetch_all(p)
        .await
        {
            Ok(rows) => rows.iter().map(row_to_step).collect(),
            Err(e) => {
                error!("Failed to get steps: {}", e);
                vec![]
            }
        }
    }

    // ── Cleanup ──

    pub async fn cleanup_old(&self, max_age_days: u32) {
        let Some(p) = self.pool() else { return };
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);
        // task_steps cascade-delete via the FK, so a single DELETE suffices.
        if let Err(e) = query("DELETE FROM tasks WHERE created_at < $1")
            .bind(cutoff)
            .execute(p)
            .await
        {
            error!("Failed to cleanup old tasks: {}", e);
        }
    }
}

fn row_to_task(row: &crate::control_db::sqlx::PgRow) -> Task {
    let status_str: String = row.get("status");
    let trigger_type: String = row.get("trigger_type");
    let trigger_info: Option<String> = row.get("trigger_info");
    let task_type_str: String = row.get("task_type");

    Task {
        id: row.get("id"),
        task_type: serde_json::from_value(serde_json::Value::String(task_type_str))
            .unwrap_or(TaskType::ContainerCreate),
        title: row.get("title"),
        status: parse_status(&status_str),
        trigger: match trigger_type.as_str() {
            "user" => TaskTrigger::User(trigger_info.unwrap_or_default()),
            "api" => TaskTrigger::Api,
            _ => TaskTrigger::System,
        },
        target: row.get("target"),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        error: row.get("error"),
    }
}

fn row_to_step(row: &crate::control_db::sqlx::PgRow) -> TaskStep {
    let status_str: String = row.get("status");

    TaskStep {
        id: row.get("id"),
        task_id: row.get("task_id"),
        step_name: row.get("step_name"),
        status: parse_status(&status_str),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        message: row.get("message"),
        details: row.get("details"),
    }
}

fn parse_status(s: &str) -> TaskStatus {
    match s {
        "pending" => TaskStatus::Pending,
        "running" => TaskStatus::Running,
        "done" => TaskStatus::Done,
        "failed" => TaskStatus::Failed,
        "cancelled" => TaskStatus::Cancelled,
        _ => TaskStatus::Pending,
    }
}
