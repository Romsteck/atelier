//! Per-app todos store (simple JSON file at `{apps_root}/{slug}/todos.json`).
//!
//! Live-updated via the `app_todos` broadcast channel on `EventBus` — the Studio
//! right-side panel subscribes through the WS API.
//!
//! ## Sémantique (volontairement minimaliste)
//!
//! Deux statuts uniquement : `pending` (note à penser plus tard) et `in_progress`
//! (en cours maintenant — une seule à la fois). Une tâche terminée ou abandonnée
//! est **supprimée** (`delete`). Pas de status `done`/`blocked`/`completed`.
//!
//! Les anciens fichiers `todos.json` qui contiennent `done`/`blocked` sont migrés
//! au load : les `done` sont droppés, les `blocked` repassés à `pending` (avec
//! le `status_reason` éventuel fusionné dans la `description`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use hr_common::events::{AppTodosEvent, EventBus};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, instrument, warn};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
}

impl TodoStatus {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "done" | "blocked" | "completed" | "archived" => Err(anyhow!(
                "todo status '{s}' is no longer supported — terminated todos must be \
                 deleted via todos_delete (no 'done' status). Only 'pending' and \
                 'in_progress' are accepted."
            )),
            other => Err(anyhow!("invalid todo status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub status: TodoStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodosFile {
    #[serde(default)]
    pub todos: Vec<Todo>,
}

/// Per-app JSON todo store. One file per app at `{apps_root}/{slug}/todos.json`.
#[derive(Clone)]
pub struct TodosManager {
    apps_root: PathBuf,
    locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    events: Arc<EventBus>,
}

impl TodosManager {
    pub fn new(apps_root: impl Into<PathBuf>, events: Arc<EventBus>) -> Self {
        let apps_root = apps_root.into();
        info!(path = %apps_root.display(), "TodosManager initialized");
        Self {
            apps_root,
            locks: Arc::new(RwLock::new(HashMap::new())),
            events,
        }
    }

    fn path_for(&self, slug: &str) -> PathBuf {
        self.apps_root.join(slug).join("todos.json")
    }

    async fn lock_for(&self, slug: &str) -> Arc<Mutex<()>> {
        {
            let guard = self.locks.read().await;
            if let Some(m) = guard.get(slug) {
                return m.clone();
            }
        }
        let mut guard = self.locks.write().await;
        guard
            .entry(slug.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Read the JSON file and migrate legacy todos (done/blocked + status_reason)
    /// to the simplified two-status model. Returns the file plus a flag telling
    /// the caller whether it must persist the migration.
    async fn load_with_migration(&self, slug: &str) -> Result<(TodosFile, bool)> {
        let path = self.path_for(slug);
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((TodosFile::default(), false))
            }
            Err(e) => return Err(anyhow!("read {}: {e}", path.display())),
        };
        let raw: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                warn!(slug, error = %e, "todos.json unreadable — starting from empty");
                return Ok((TodosFile::default(), false));
            }
        };
        let raw_todos = raw
            .get("todos")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut migrated = false;
        let mut out: Vec<Todo> = Vec::with_capacity(raw_todos.len());
        for entry in raw_todos {
            let status_str = entry
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending")
                .to_string();
            let legacy_reason = entry
                .get("status_reason")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string());
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if name.is_empty() || id.is_empty() {
                warn!(slug, "dropping malformed todo entry during load");
                migrated = true;
                continue;
            }
            // Migration des statuts legacy
            let new_status = match status_str.as_str() {
                "pending" => TodoStatus::Pending,
                "in_progress" => TodoStatus::InProgress,
                "done" | "completed" | "archived" => {
                    info!(slug, id = %id, "migrating legacy 'done' todo → dropped");
                    migrated = true;
                    continue;
                }
                "blocked" => {
                    info!(slug, id = %id, "migrating legacy 'blocked' todo → pending");
                    migrated = true;
                    TodoStatus::Pending
                }
                other => {
                    warn!(slug, id = %id, status = %other, "unknown legacy status → pending");
                    migrated = true;
                    TodoStatus::Pending
                }
            };
            let mut description = entry
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string());
            // Si on avait un status_reason et qu'on perd le statut, on le préserve
            // dans la description pour ne pas perdre l'information.
            if let Some(reason) = legacy_reason {
                if status_str == "blocked" {
                    let prefix = format!("⚠ ancien blocage : {reason}");
                    description = Some(match description {
                        Some(d) => format!("{prefix}\n\n{d}"),
                        None => prefix,
                    });
                    migrated = true;
                }
            }
            let created_at = entry
                .get("created_at")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(Utc::now);
            let updated_at = entry
                .get("updated_at")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or(created_at);
            out.push(Todo {
                id,
                name,
                description,
                status: new_status,
                created_at,
                updated_at,
            });
        }
        // Si le fichier original avait un champ `status_reason` même sans status legacy,
        // on l'a déjà droppé via la projection — c'est aussi une migration silencieuse.
        // On ne flippe pas `migrated` pour ça si le statut était déjà valide :
        // l'absence dans le nouveau schéma sera juste reflétée au prochain save naturel.
        Ok((TodosFile { todos: out }, migrated))
    }

    async fn load(&self, slug: &str) -> Result<TodosFile> {
        let (file, migrated) = self.load_with_migration(slug).await?;
        if migrated {
            // Persiste la migration immédiatement et broadcaste pour rafraîchir le panneau.
            if let Err(e) = self.save(slug, &file).await {
                warn!(slug, error = %e, "failed to persist todos migration");
            } else {
                info!(slug, count = file.todos.len(), "todos migration persisted");
                self.broadcast(slug, &file.todos);
            }
        }
        Ok(file)
    }

    async fn save(&self, slug: &str, file: &TodosFile) -> Result<()> {
        let path = self.path_for(slug);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let bytes = serde_json::to_vec_pretty(file)?;
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| anyhow!("write tmp {}: {e}", tmp.display()))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| anyhow!("rename {}: {e}", path.display()))?;
        Ok(())
    }

    fn broadcast(&self, slug: &str, todos: &[Todo]) {
        let payload: Vec<serde_json::Value> = todos
            .iter()
            .map(|t| serde_json::to_value(t).unwrap_or(serde_json::Value::Null))
            .collect();
        let _ = self.events.app_todos.send(AppTodosEvent {
            slug: slug.to_string(),
            todos: payload,
        });
    }

    #[instrument(skip(self), fields(slug = %slug))]
    pub async fn list(&self, slug: &str, status: Option<TodoStatus>) -> Result<Vec<Todo>> {
        let lock = self.lock_for(slug).await;
        let _g = lock.lock().await;
        let file = self.load(slug).await?;
        let items: Vec<Todo> = match status {
            Some(s) => file.todos.into_iter().filter(|t| t.status == s).collect(),
            None => file.todos,
        };
        info!(count = items.len(), "todos listed");
        Ok(items)
    }

    #[instrument(skip(self), fields(slug = %slug))]
    pub async fn create(
        &self,
        slug: &str,
        name: String,
        description: Option<String>,
    ) -> Result<Todo> {
        if name.trim().is_empty() {
            return Err(anyhow!("todo name cannot be empty"));
        }
        let lock = self.lock_for(slug).await;
        let _g = lock.lock().await;
        let mut file = self.load(slug).await?;
        let now = Utc::now();
        let todo = Todo {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.trim().to_string(),
            description: description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
            status: TodoStatus::Pending,
            created_at: now,
            updated_at: now,
        };
        file.todos.push(todo.clone());
        self.save(slug, &file).await?;
        info!(id = %todo.id, name = %todo.name, "todo created");
        self.broadcast(slug, &file.todos);
        Ok(todo)
    }

    #[instrument(skip(self), fields(slug = %slug, id = %id))]
    pub async fn update(
        &self,
        slug: &str,
        id: &str,
        name: Option<String>,
        description: Option<String>,
        status: Option<TodoStatus>,
    ) -> Result<Todo> {
        let lock = self.lock_for(slug).await;
        let _g = lock.lock().await;
        let mut file = self.load(slug).await?;
        let idx = file
            .todos
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| anyhow!("todo {id} not found"))?;

        // Enforce single in_progress : si on demande d'en démarrer un, demoter les autres.
        if matches!(status, Some(TodoStatus::InProgress)) {
            let now = Utc::now();
            for (i, t) in file.todos.iter_mut().enumerate() {
                if i == idx {
                    continue;
                }
                if t.status == TodoStatus::InProgress {
                    warn!(
                        slug,
                        demoted_id = %t.id,
                        demoted_name = %t.name,
                        "demoting previous in_progress todo to pending (single in_progress rule)"
                    );
                    t.status = TodoStatus::Pending;
                    t.updated_at = now;
                }
            }
        }

        let t = &mut file.todos[idx];
        if let Some(n) = name {
            if !n.trim().is_empty() {
                t.name = n.trim().to_string();
            }
        }
        if let Some(d) = description {
            let trimmed = d.trim().to_string();
            t.description = if trimmed.is_empty() { None } else { Some(trimmed) };
        }
        if let Some(s) = status {
            t.status = s;
        }
        t.updated_at = Utc::now();
        let todo = t.clone();
        self.save(slug, &file).await?;
        info!(status = ?todo.status, "todo updated");
        self.broadcast(slug, &file.todos);
        Ok(todo)
    }

    #[instrument(skip(self), fields(slug = %slug, id = %id))]
    pub async fn delete(&self, slug: &str, id: &str) -> Result<()> {
        let lock = self.lock_for(slug).await;
        let _g = lock.lock().await;
        let mut file = self.load(slug).await?;
        let before = file.todos.len();
        file.todos.retain(|t| t.id != id);
        if file.todos.len() == before {
            warn!("todo not found for delete");
            return Err(anyhow!("todo {id} not found"));
        }
        self.save(slug, &file).await?;
        info!("todo deleted");
        self.broadcast(slug, &file.todos);
        Ok(())
    }
}
