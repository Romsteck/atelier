use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::error;

use crate::tasks::*;

/// Persistent task storage backed by SQLite (WAL mode).
pub struct TaskStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl TaskStore {
    pub fn new(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS tasks (
                id          TEXT PRIMARY KEY,
                task_type   TEXT NOT NULL,
                title       TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'pending',
                trigger_type TEXT NOT NULL,
                trigger_info TEXT,
                target      TEXT,
                created_at  TEXT NOT NULL,
                started_at  TEXT,
                finished_at TEXT,
                error       TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
            CREATE INDEX IF NOT EXISTS idx_tasks_created ON tasks(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_tasks_type ON tasks(task_type);

            CREATE TABLE IF NOT EXISTS task_steps (
                id          TEXT PRIMARY KEY,
                task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                step_name   TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'running',
                started_at  TEXT NOT NULL,
                finished_at TEXT,
                message     TEXT,
                details     TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_steps_task ON task_steps(task_id);
            ",
        )?;

        // Mark orphaned running tasks as failed (from previous crash)
        conn.execute(
            "UPDATE tasks SET status = 'failed', error = 'Interrupted by restart', finished_at = ?1 WHERE status IN ('pending', 'running')",
            rusqlite::params![chrono::Utc::now().to_rfc3339()],
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
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

        let conn = self.conn.lock().await;
        if let Err(e) = conn.execute(
            "INSERT INTO tasks (id, task_type, title, status, trigger_type, trigger_info, target, created_at)
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                task_type_str,
                title,
                trigger_type,
                trigger_info,
                target,
                now.to_rfc3339(),
            ],
        ) {
            error!("Failed to create task: {}", e);
        }

        task
    }

    pub async fn update_task_status(&self, id: &str, status: TaskStatus, error_msg: Option<&str>) {
        let now = chrono::Utc::now().to_rfc3339();
        let status_str = status.to_string();
        let conn = self.conn.lock().await;

        let result = match status {
            TaskStatus::Running => conn.execute(
                "UPDATE tasks SET status = ?2, started_at = ?3 WHERE id = ?1",
                rusqlite::params![id, status_str, now],
            ),
            TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => conn.execute(
                "UPDATE tasks SET status = ?2, finished_at = ?3, error = ?4 WHERE id = ?1",
                rusqlite::params![id, status_str, now, error_msg],
            ),
            _ => conn.execute(
                "UPDATE tasks SET status = ?2 WHERE id = ?1",
                rusqlite::params![id, status_str],
            ),
        };

        if let Err(e) = result {
            error!("Failed to update task status: {}", e);
        }
    }

    pub async fn get_task(&self, id: &str) -> Option<Task> {
        let conn = self.conn.lock().await;
        conn.query_row(
            "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error FROM tasks WHERE id = ?1",
            rusqlite::params![id],
            |row| Ok(row_to_task(row)),
        )
        .ok()
    }

    pub async fn list_tasks(
        &self,
        limit: u32,
        offset: u32,
        status: Option<&str>,
    ) -> (Vec<Task>, u32) {
        let conn = self.conn.lock().await;

        let total: u32 = if let Some(s) = status {
            conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE status = ?1",
                rusqlite::params![s],
                |row| row.get(0),
            )
            .unwrap_or(0)
        } else {
            conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
                .unwrap_or(0)
        };

        let tasks: Vec<Task> = if let Some(s) = status {
            let mut stmt = match conn.prepare(
                "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error FROM tasks WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            ) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to prepare list_tasks: {}", e);
                    return (vec![], 0);
                }
            };
            stmt.query_map(rusqlite::params![s, limit, offset], |row| {
                Ok(row_to_task(row))
            })
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        } else {
            let mut stmt = match conn.prepare(
                "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error FROM tasks ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
            ) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to prepare list_tasks: {}", e);
                    return (vec![], 0);
                }
            };
            stmt.query_map(rusqlite::params![limit, offset], |row| Ok(row_to_task(row)))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };

        (tasks, total)
    }

    pub async fn get_active_tasks(&self) -> Vec<Task> {
        let conn = self.conn.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT id, task_type, title, status, trigger_type, trigger_info, target, created_at, started_at, finished_at, error FROM tasks WHERE status IN ('pending', 'running') ORDER BY created_at DESC",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to prepare get_active_tasks: {}", e);
                return vec![];
            }
        };

        match stmt.query_map([], |row| Ok(row_to_task(row))) {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
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

        let conn = self.conn.lock().await;
        if let Err(e) = conn.execute(
            "INSERT INTO task_steps (id, task_id, step_name, status, started_at, message) VALUES (?1, ?2, ?3, 'running', ?4, ?5)",
            rusqlite::params![id, task_id, name, now.to_rfc3339(), message],
        ) {
            error!("Failed to create step: {}", e);
        }

        step
    }

    pub async fn complete_step(&self, id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        if let Err(e) = conn.execute(
            "UPDATE task_steps SET status = 'done', finished_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now],
        ) {
            error!("Failed to complete step: {}", e);
        }
    }

    pub async fn fail_step(&self, id: &str, error_msg: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        if let Err(e) = conn.execute(
            "UPDATE task_steps SET status = 'failed', finished_at = ?2, message = ?3 WHERE id = ?1",
            rusqlite::params![id, now, error_msg],
        ) {
            error!("Failed to fail step: {}", e);
        }
    }

    pub async fn update_step(&self, id: &str, message: &str, details: Option<serde_json::Value>) {
        let details_str = details.map(|d| d.to_string());
        let conn = self.conn.lock().await;
        if let Err(e) = conn.execute(
            "UPDATE task_steps SET message = ?2, details = ?3 WHERE id = ?1",
            rusqlite::params![id, message, details_str],
        ) {
            error!("Failed to update step: {}", e);
        }
    }

    pub async fn get_steps(&self, task_id: &str) -> Vec<TaskStep> {
        let conn = self.conn.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT id, task_id, step_name, status, started_at, finished_at, message, details FROM task_steps WHERE task_id = ?1 ORDER BY started_at ASC",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to prepare get_steps: {}", e);
                return vec![];
            }
        };

        match stmt.query_map(rusqlite::params![task_id], |row| Ok(row_to_step(row))) {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                error!("Failed to get steps: {}", e);
                vec![]
            }
        }
    }

    // ── Cleanup ──

    pub async fn cleanup_old(&self, max_age_days: u32) {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);
        let conn = self.conn.lock().await;
        if let Err(e) = conn.execute(
            "DELETE FROM task_steps WHERE task_id IN (SELECT id FROM tasks WHERE created_at < ?1)",
            rusqlite::params![cutoff.to_rfc3339()],
        ) {
            error!("Failed to cleanup old steps: {}", e);
        }
        if let Err(e) = conn.execute(
            "DELETE FROM tasks WHERE created_at < ?1",
            rusqlite::params![cutoff.to_rfc3339()],
        ) {
            error!("Failed to cleanup old tasks: {}", e);
        }
    }
}

fn row_to_task(row: &rusqlite::Row) -> Task {
    let status_str: String = row.get(3).unwrap_or_default();
    let trigger_type: String = row.get(4).unwrap_or_default();
    let trigger_info: Option<String> = row.get(5).unwrap_or(None);
    let task_type_str: String = row.get(1).unwrap_or_default();

    Task {
        id: row.get(0).unwrap_or_default(),
        task_type: serde_json::from_value(serde_json::Value::String(task_type_str))
            .unwrap_or(TaskType::ContainerCreate),
        title: row.get(2).unwrap_or_default(),
        status: parse_status(&status_str),
        trigger: match trigger_type.as_str() {
            "user" => TaskTrigger::User(trigger_info.unwrap_or_default()),
            "api" => TaskTrigger::Api,
            _ => TaskTrigger::System,
        },
        target: row.get(6).unwrap_or(None),
        created_at: parse_datetime(row.get::<_, String>(7).ok()),
        started_at: parse_datetime_opt(row.get::<_, Option<String>>(8).ok().flatten()),
        finished_at: parse_datetime_opt(row.get::<_, Option<String>>(9).ok().flatten()),
        error: row.get(10).unwrap_or(None),
    }
}

fn row_to_step(row: &rusqlite::Row) -> TaskStep {
    let status_str: String = row.get(3).unwrap_or_default();
    let details_str: Option<String> = row.get(7).unwrap_or(None);

    TaskStep {
        id: row.get(0).unwrap_or_default(),
        task_id: row.get(1).unwrap_or_default(),
        step_name: row.get(2).unwrap_or_default(),
        status: parse_status(&status_str),
        started_at: parse_datetime(row.get::<_, String>(4).ok()),
        finished_at: parse_datetime_opt(row.get::<_, Option<String>>(5).ok().flatten()),
        message: row.get(6).unwrap_or(None),
        details: details_str.and_then(|s| serde_json::from_str(&s).ok()),
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

fn parse_datetime(s: Option<String>) -> chrono::DateTime<chrono::Utc> {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now)
}

fn parse_datetime_opt(s: Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
}
