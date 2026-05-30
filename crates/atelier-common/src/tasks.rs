use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::events::EventBus;
use crate::task_store::TaskStore;

/// Type d'action trackée.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    ContainerCreate,
    ContainerMigrate,
    ContainerRename,
    ContainerDelete,
    ContainerStart,
    ContainerStop,
    AppDeploy,
    AgentUpdate,
    GitSync,
    AcmeRenew,
    UpdatesCheck,
    UpdatesUpgrade,
    DnsReload,
    ProxyReload,
    HostPower,
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{:?}", self));
        write!(f, "{}", s)
    }
}

/// Statut d'une task ou d'un step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Done => write!(f, "done"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Qui a déclenché la task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "info")]
pub enum TaskTrigger {
    User(String),
    System,
    Api,
}

/// Une task = une action complète avec ses métadonnées.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub task_type: TaskType,
    pub title: String,
    pub status: TaskStatus,
    pub trigger: TaskTrigger,
    pub target: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

/// Un step = une étape dans l'exécution d'une task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStep {
    pub id: String,
    pub task_id: String,
    pub step_name: String,
    pub status: TaskStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub message: Option<String>,
    pub details: Option<serde_json::Value>,
}

/// Event broadcast quand une task est mise à jour.
#[derive(Debug, Clone, Serialize)]
pub struct TaskUpdateEvent {
    pub task: Task,
    pub steps: Vec<TaskStep>,
}

/// Context passé aux handlers pour reporter le progress.
#[derive(Clone)]
pub struct TaskContext {
    task_id: String,
    store: Arc<TaskStore>,
    events: Arc<EventBus>,
}

impl TaskContext {
    pub fn new(task_id: String, store: Arc<TaskStore>, events: Arc<EventBus>) -> Self {
        Self {
            task_id,
            store,
            events,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// Marque la task comme running.
    pub async fn start(&self) {
        self.store
            .update_task_status(&self.task_id, TaskStatus::Running, None)
            .await;
        self.emit_update().await;
    }

    /// Crée un nouveau step et le marque running.
    pub async fn step(&self, name: &str, message: &str) -> TaskStepHandle {
        let step = self.store.create_step(&self.task_id, name, message).await;
        self.emit_update().await;
        TaskStepHandle {
            step_id: step.id,
            ctx: self.clone(),
            completed: false,
        }
    }

    /// Met à jour le message du step courant (pour progress).
    pub async fn update_step(
        &self,
        step_id: &str,
        message: &str,
        details: Option<serde_json::Value>,
    ) {
        self.store.update_step(step_id, message, details).await;
        self.emit_update().await;
    }

    /// Marque la task comme failed.
    pub async fn fail(&self, error: &str) {
        self.store
            .update_task_status(&self.task_id, TaskStatus::Failed, Some(error))
            .await;
        self.emit_update().await;
    }

    /// Marque la task comme done.
    pub async fn done(&self) {
        self.store
            .update_task_status(&self.task_id, TaskStatus::Done, None)
            .await;
        self.emit_update().await;
    }

    /// Marque la task comme cancelled.
    pub async fn cancel(&self) {
        self.store
            .update_task_status(&self.task_id, TaskStatus::Cancelled, None)
            .await;
        self.emit_update().await;
    }

    async fn emit_update(&self) {
        if let Some(task) = self.store.get_task(&self.task_id).await {
            let steps = self.store.get_steps(&self.task_id).await;
            let _ = self
                .events
                .task_update
                .send(TaskUpdateEvent { task, steps });
        }
    }
}

/// Handle sur un step en cours — auto-fail si droppé sans complete/fail.
pub struct TaskStepHandle {
    step_id: String,
    ctx: TaskContext,
    completed: bool,
}

impl TaskStepHandle {
    pub fn id(&self) -> &str {
        &self.step_id
    }

    pub async fn complete(mut self) {
        self.completed = true;
        self.ctx.store.complete_step(&self.step_id).await;
        self.ctx.emit_update().await;
    }

    pub async fn fail(mut self, error: &str) {
        self.completed = true;
        self.ctx.store.fail_step(&self.step_id, error).await;
        self.ctx.emit_update().await;
    }
}

impl Drop for TaskStepHandle {
    fn drop(&mut self) {
        if !self.completed {
            let store = self.ctx.store.clone();
            let step_id = self.step_id.clone();
            tokio::spawn(async move {
                store
                    .fail_step(&step_id, "Step dropped without completion")
                    .await;
            });
        }
    }
}
