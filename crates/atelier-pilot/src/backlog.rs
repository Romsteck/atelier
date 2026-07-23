use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tracing::warn;
use uuid::Uuid;

use crate::service::PilotEvent;
use crate::sqlx::{PgPool, PgRow, Row, query};

pub const LANES: &[&str] = &["ready", "in_progress", "attention", "done", "archived"];
pub const ACTIVE_EXEC: &[&str] = &["queued", "running"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Question {
    pub q: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogItem {
    pub id: i64,
    pub scope: String,
    pub title: String,
    pub request: String,
    pub description: String,
    pub plan: Option<String>,
    pub kind: String,
    pub priority: String,
    pub severity: String,
    pub effort: String,
    pub lane: String,
    pub position: f64,
    pub exec_status: String,
    pub attempts: i32,
    pub engine: String,
    pub needs_user: bool,
    pub needs_user_reason: Option<String>,
    pub questions: Vec<Question>,
    pub session_id: Option<String>,
    pub finding_id: Option<i64>,
    pub last_run_id: Option<Uuid>,
    pub last_engine: Option<String>,
    pub commit_shas: Value,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub done_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewBacklogItem {
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub title: String,
    pub request: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default = "default_medium")]
    pub priority: String,
    #[serde(default = "default_medium")]
    pub severity: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default = "default_lane")]
    pub lane: String,
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default)]
    pub needs_user: bool,
    #[serde(default)]
    pub needs_user_reason: Option<String>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub finding_id: Option<i64>,
    #[serde(default = "default_created_by")]
    pub created_by: String,
}

fn default_scope() -> String {
    "atelier".into()
}
fn default_kind() -> String {
    "improvement".into()
}
fn default_medium() -> String {
    "medium".into()
}
fn default_effort() -> String {
    "m".into()
}
fn default_lane() -> String {
    "ready".into()
}
fn default_engine() -> String {
    "auto".into()
}
fn default_created_by() -> String {
    "user".into()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BacklogPatch {
    pub scope: Option<String>,
    pub title: Option<String>,
    pub request: Option<String>,
    pub description: Option<String>,
    pub plan: Option<String>,
    pub kind: Option<String>,
    pub priority: Option<String>,
    pub severity: Option<String>,
    pub effort: Option<String>,
    pub lane: Option<String>,
    pub position: Option<f64>,
    pub exec_status: Option<String>,
    pub engine: Option<String>,
    pub needs_user: Option<bool>,
    pub needs_user_reason: Option<String>,
    pub questions: Option<Vec<Question>>,
    pub session_id: Option<String>,
    pub reset_attempts: Option<bool>,
}

#[derive(Clone)]
pub struct BacklogStore {
    pool: PgPool,
    // Sender du canal pilot:backlog (posé par PilotService::start) : permet à
    // `republish` d'émettre sans repasser par le service — consommé par le
    // callback git_watcher de main.rs.
    events: Option<broadcast::Sender<PilotEvent>>,
}

impl BacklogStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool, events: None }
    }

    pub fn with_events(mut self, tx: broadcast::Sender<PilotEvent>) -> Self {
        self.events = Some(tx);
        self
    }

    /// Ré-émet l'item tel quel (event `updated`) après une mutation externe au
    /// service (ex. settle Done par le git_watcher). Best-effort.
    pub async fn republish(&self, id: i64) {
        match self.get(id).await {
            Ok(Some(item)) => {
                if let Some(tx) = &self.events {
                    let _ = tx.send(PilotEvent {
                        action: "updated".into(),
                        item: Some(item),
                        id: Some(id),
                    });
                }
            }
            Ok(None) => {}
            Err(e) => warn!(id, error = %e, "pilot republish fetch failed"),
        }
    }

    pub async fn insert(&self, mut item: NewBacklogItem) -> anyhow::Result<BacklogItem> {
        normalize_new(&mut item)?;
        if item.title.trim().is_empty() {
            item.title = derive_title(&item.request);
        }
        let questions = serde_json::to_value(&item.questions)?;
        let row = query(
            r#"
            INSERT INTO backlog_items
                (scope,title,request,description,plan,kind,priority,severity,effort,lane,
                 position,engine,needs_user,needs_user_reason,questions,finding_id,created_by)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,
                    COALESCE((SELECT max(position)+1024 FROM backlog_items WHERE scope=$1 AND lane=$10),1024),
                    $11,$12,$13,$14,$15,$16)
            RETURNING *
            "#,
        )
        .bind(&item.scope).bind(item.title.trim()).bind(item.request.trim())
        .bind(item.description.trim()).bind(item.plan.as_deref())
        .bind(&item.kind).bind(&item.priority).bind(&item.severity).bind(&item.effort)
        .bind(&item.lane).bind(&item.engine).bind(item.needs_user)
        .bind(item.needs_user_reason.as_deref()).bind(questions).bind(item.finding_id)
        .bind(&item.created_by).fetch_one(&self.pool).await?;
        row_to_item(&row)
    }

    pub async fn list(
        &self,
        scope: Option<&str>,
        lane: Option<&str>,
    ) -> anyhow::Result<Vec<BacklogItem>> {
        let rows = query(
            r#"SELECT * FROM backlog_items
               WHERE ($1::text IS NULL OR scope=$1) AND ($2::text IS NULL OR lane=$2)
               ORDER BY CASE lane WHEN 'ready' THEN 0 WHEN 'in_progress' THEN 1 WHEN 'attention' THEN 2 WHEN 'done' THEN 3 ELSE 4 END,
                        position, id"#,
        ).bind(scope).bind(lane).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_item).collect()
    }

    pub async fn get(&self, id: i64) -> anyhow::Result<Option<BacklogItem>> {
        let row = query("SELECT * FROM backlog_items WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_item).transpose()
    }

    pub async fn update(
        &self,
        id: i64,
        patch: BacklogPatch,
    ) -> anyhow::Result<Option<BacklogItem>> {
        validate_patch(&patch)?;
        let q = patch
            .questions
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let row = query(
            r#"UPDATE backlog_items SET
                scope=COALESCE($2,scope), title=COALESCE($3,title), request=COALESCE($4,request),
                description=COALESCE($5,description), plan=COALESCE($6,plan), kind=COALESCE($7,kind),
                priority=COALESCE($8,priority), severity=COALESCE($9,severity), effort=COALESCE($10,effort),
                lane=COALESCE($11,lane), position=COALESCE($12,position), exec_status=COALESCE($13,exec_status),
                engine=COALESCE($14,engine), needs_user=COALESCE($15,needs_user),
                needs_user_reason=COALESCE($16,needs_user_reason), questions=COALESCE($17,questions),
                session_id=COALESCE($18,session_id),
                attempts=CASE WHEN COALESCE($19,false) THEN 0 ELSE attempts END,
                done_at=CASE WHEN COALESCE($11,lane)='done' THEN COALESCE(done_at,now()) ELSE NULL END,
                updated_at=now()
              WHERE id=$1
                AND (exec_status NOT IN ('queued','running') OR COALESCE($20,false))
              RETURNING *"#,
        )
        .bind(id).bind(patch.scope.as_deref()).bind(patch.title.as_deref())
        .bind(patch.request.as_deref()).bind(patch.description.as_deref()).bind(patch.plan.as_deref())
        .bind(patch.kind.as_deref()).bind(patch.priority.as_deref()).bind(patch.severity.as_deref())
        .bind(patch.effort.as_deref()).bind(patch.lane.as_deref()).bind(patch.position)
        .bind(patch.exec_status.as_deref()).bind(patch.engine.as_deref()).bind(patch.needs_user)
        .bind(patch.needs_user_reason.as_deref()).bind(q).bind(patch.session_id.as_deref())
        // A worker may suspend itself for clarification while the item is
        // running. Every other edit remains locked until the run settles.
        .bind(patch.reset_attempts).bind(patch.needs_user == Some(true))
        .fetch_optional(&self.pool).await?;
        row.as_ref().map(row_to_item).transpose()
    }

    pub async fn delete(&self, id: i64) -> anyhow::Result<bool> {
        let res = query(
            "DELETE FROM backlog_items WHERE id=$1 AND exec_status NOT IN ('queued','running')",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn delete_by_scope(&self, scope: &str) -> anyhow::Result<u64> {
        Ok(query(
            "DELETE FROM backlog_items WHERE scope=$1 AND exec_status NOT IN ('queued','running')",
        )
        .bind(scope)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// File d'attente manuelle : l'item passe `queued` SANS run_id (il n'a pas
    /// encore de créneau). Le dispatcher lui en attribuera un via `mark_queued`
    /// au moment du lancement réel. `last_run_id=NULL` distingue un item « en
    /// file, jamais démarré » d'un run réellement interrompu (cf. reconcile_boot).
    pub async fn mark_pending(&self, id: i64) -> anyhow::Result<Option<BacklogItem>> {
        let row = query(
            "UPDATE backlog_items SET exec_status='queued',last_run_id=NULL,updated_at=now() \
             WHERE id=$1 AND exec_status NOT IN ('queued','running') AND needs_user=false \
             AND lane IN ('ready','attention') RETURNING *",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_item).transpose()
    }

    pub async fn mark_queued(&self, id: i64, run_id: Uuid) -> anyhow::Result<Option<BacklogItem>> {
        // Claim par le lanceur (nuit ou dispatcher manuel). Autorisé si l'item
        // est frais (idle/failed/…) OU « en file jamais démarré » (queued sans
        // run_id, posé par mark_pending) — mais JAMAIS s'il tourne déjà
        // (queued/running AVEC run_id).
        let row = query(
            "UPDATE backlog_items SET exec_status='queued',last_run_id=$2,updated_at=now() \
             WHERE id=$1 AND (exec_status NOT IN ('queued','running') OR last_run_id IS NULL) \
             AND needs_user=false AND lane IN ('ready','attention') RETURNING *",
        )
        .bind(id)
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_item).transpose()
    }

    pub async fn mark_running(&self, id: i64, attempt: i32) -> anyhow::Result<BacklogItem> {
        let row = query(
            "UPDATE backlog_items SET exec_status='running',lane='in_progress',attempts=$2,updated_at=now() WHERE id=$1 RETURNING *"
        ).bind(id).bind(attempt).fetch_one(&self.pool).await?;
        row_to_item(&row)
    }

    pub async fn settle_done(
        &self,
        id: i64,
        commit_sha: Option<&str>,
        engine: Option<&str>,
    ) -> anyhow::Result<BacklogItem> {
        let row = query(
            r#"UPDATE backlog_items SET exec_status='done',lane='done',needs_user=false,
               needs_user_reason=NULL, done_at=now(), updated_at=now(),
               last_engine=COALESCE($3,last_engine),
               commit_shas=CASE WHEN $2::text IS NULL THEN commit_shas ELSE commit_shas || jsonb_build_array($2::text) END
               WHERE id=$1 RETURNING *"#,
        ).bind(id).bind(commit_sha).bind(engine).fetch_one(&self.pool).await?;
        row_to_item(&row)
    }

    pub async fn settle_attention(
        &self,
        id: i64,
        blocked: bool,
        reason: &str,
        engine: Option<&str>,
    ) -> anyhow::Result<BacklogItem> {
        let exec = if blocked { "blocked" } else { "failed" };
        let row = query(
            "UPDATE backlog_items SET exec_status=$2,lane='attention',needs_user_reason=$3,last_engine=COALESCE($4,last_engine),updated_at=now() WHERE id=$1 RETURNING *"
        ).bind(id).bind(exec).bind(reason).bind(engine).fetch_one(&self.pool).await?;
        row_to_item(&row)
    }

    pub async fn settle_needs_user(
        &self,
        id: i64,
        reason: &str,
        questions: &[Question],
        engine: Option<&str>,
    ) -> anyhow::Result<BacklogItem> {
        // Un run incertain ne consomme pas sa tentative (m8) : mark_running
        // l'avait posée en absolu, on la rend — l'item repartira propre après
        // les réponses de Romain.
        let row = query(
            "UPDATE backlog_items SET exec_status='blocked',lane='attention',needs_user=true,needs_user_reason=$2,questions=$3,\
             attempts=GREATEST(attempts-1,0),last_engine=COALESCE($4,last_engine),updated_at=now() WHERE id=$1 RETURNING *"
        ).bind(id).bind(reason).bind(serde_json::to_value(questions)?).bind(engine).fetch_one(&self.pool).await?;
        row_to_item(&row)
    }

    pub async fn defer_ready(&self, id: i64) -> anyhow::Result<BacklogItem> {
        let row = query(
            "UPDATE backlog_items SET exec_status='idle',lane='ready',attempts=GREATEST(attempts-1,0),updated_at=now() WHERE id=$1 RETURNING *"
        ).bind(id).fetch_one(&self.pool).await?;
        row_to_item(&row)
    }

    pub async fn ready_items(&self, include_atelier: bool) -> anyhow::Result<Vec<BacklogItem>> {
        let rows = query(
            r#"SELECT * FROM backlog_items WHERE lane='ready' AND exec_status='idle' AND needs_user=false
               AND ($1::bool OR scope <> 'atelier')
               ORDER BY (scope='atelier'), CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END,
                        position,id"#,
        ).bind(include_atelier).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_item).collect()
    }

    pub async fn reconcile_boot(&self) -> anyhow::Result<u64> {
        // Les items « en file jamais démarrés » (queued sans run_id, cf.
        // mark_pending) ne sont PAS réinitialisés : la file manuelle mémoire est
        // reconstruite au boot depuis cet état (`resume_pending_queue`) — c'est
        // ce qui fait survivre la file au restart déclenché par un deploy
        // Atelier du Pilote lui-même. Seuls les runs réellement interrompus
        // (queued/running AVEC run_id) partent en attention, sauf le worker
        // atelier détaché encore en phase report.
        Ok(query(
            "UPDATE backlog_items b SET exec_status='failed',lane='attention',needs_user_reason='Run interrompu par le redémarrage d’Atelier',updated_at=now() \
             WHERE exec_status IN ('queued','running') AND NOT (exec_status='queued' AND last_run_id IS NULL) AND NOT EXISTS (\
               SELECT 1 FROM backlog_runs r WHERE r.item_id=b.id AND r.status='running' AND r.scope='atelier' AND r.phase='report'\
             )"
        ).execute(&self.pool).await?.rows_affected())
    }

    /// Items « en file jamais démarrés », dans l'ordre d'enfilement (updated_at
    /// posé par mark_pending) — base de la reconstruction de la file au boot.
    pub async fn pending_queue(&self) -> anyhow::Result<Vec<BacklogItem>> {
        let rows = query(
            "SELECT * FROM backlog_items WHERE exec_status='queued' AND last_run_id IS NULL ORDER BY updated_at, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_item).collect()
    }

    /// Retrait volontaire de la file d'attente : uniquement un item « en file
    /// jamais démarré » — un run déjà lancé s'annule via cancel_run.
    pub async fn unqueue(&self, id: i64) -> anyhow::Result<Option<BacklogItem>> {
        let row = query(
            "UPDATE backlog_items SET exec_status='idle',updated_at=now() \
             WHERE id=$1 AND exec_status='queued' AND last_run_id IS NULL RETURNING *",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_item).transpose()
    }
}

fn derive_title(request: &str) -> String {
    let first = request
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("Nouveau besoin")
        .trim();
    let title: String = first.chars().take(96).collect();
    if first.chars().count() > 96 {
        format!("{title}…")
    } else {
        title
    }
}

fn valid_scope(v: &str) -> bool {
    v == "atelier"
        || (!v.is_empty()
            && v.len() <= 80
            && v.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
}

fn one_of(v: &str, allowed: &[&str], field: &str) -> anyhow::Result<()> {
    anyhow::ensure!(allowed.contains(&v), "{field} invalide: {v}");
    Ok(())
}

fn normalize_new(i: &mut NewBacklogItem) -> anyhow::Result<()> {
    anyhow::ensure!(valid_scope(&i.scope), "scope invalide");
    anyhow::ensure!(!i.request.trim().is_empty(), "request vide");
    one_of(
        &i.kind,
        &["feature", "bug", "improvement", "finding_fix"],
        "kind",
    )?;
    one_of(
        &i.priority,
        &["critical", "high", "medium", "low"],
        "priority",
    )?;
    one_of(
        &i.severity,
        &["critical", "high", "medium", "low"],
        "severity",
    )?;
    one_of(&i.effort, &["xs", "s", "m", "l", "xl"], "effort")?;
    one_of(&i.lane, LANES, "lane")?;
    one_of(&i.engine, &["auto", "claude", "codex"], "engine")?;
    one_of(
        &i.created_by,
        &["user", "assistant", "scan", "system"],
        "created_by",
    )?;
    if i.needs_user {
        i.lane = "attention".into();
    }
    Ok(())
}

fn validate_patch(p: &BacklogPatch) -> anyhow::Result<()> {
    if let Some(v) = &p.scope {
        anyhow::ensure!(valid_scope(v), "scope invalide");
    }
    if let Some(v) = &p.kind {
        one_of(v, &["feature", "bug", "improvement", "finding_fix"], "kind")?;
    }
    if let Some(v) = &p.priority {
        one_of(v, &["critical", "high", "medium", "low"], "priority")?;
    }
    if let Some(v) = &p.severity {
        one_of(v, &["critical", "high", "medium", "low"], "severity")?;
    }
    if let Some(v) = &p.effort {
        one_of(v, &["xs", "s", "m", "l", "xl"], "effort")?;
    }
    if let Some(v) = &p.lane {
        one_of(v, LANES, "lane")?;
    }
    if let Some(v) = &p.exec_status {
        one_of(v, &["idle", "done", "failed", "blocked"], "exec_status")?;
    }
    if let Some(v) = &p.engine {
        one_of(v, &["auto", "claude", "codex"], "engine")?;
    }
    Ok(())
}

fn row_to_item(row: &PgRow) -> anyhow::Result<BacklogItem> {
    let qv: Value = row.try_get("questions").unwrap_or_else(|_| json!([]));
    Ok(BacklogItem {
        id: row.try_get("id")?,
        scope: row.try_get("scope")?,
        title: row.try_get("title")?,
        request: row.try_get("request")?,
        description: row.try_get("description")?,
        plan: row.try_get("plan").ok(),
        kind: row.try_get("kind")?,
        priority: row.try_get("priority")?,
        severity: row.try_get("severity")?,
        effort: row.try_get("effort")?,
        lane: row.try_get("lane")?,
        position: row.try_get("position")?,
        exec_status: row.try_get("exec_status")?,
        attempts: row.try_get("attempts")?,
        engine: row.try_get("engine")?,
        needs_user: row.try_get("needs_user")?,
        needs_user_reason: row.try_get("needs_user_reason").ok(),
        questions: serde_json::from_value(qv).unwrap_or_default(),
        session_id: row.try_get("session_id").ok(),
        finding_id: row.try_get("finding_id").ok(),
        last_run_id: row.try_get("last_run_id").ok(),
        last_engine: row.try_get("last_engine").ok(),
        commit_shas: row.try_get("commit_shas").unwrap_or_else(|_| json!([])),
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        done_at: row.try_get("done_at").ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::{BacklogPatch, NewBacklogItem, derive_title, normalize_new, validate_patch};

    fn item(request: &str) -> NewBacklogItem {
        serde_json::from_value(serde_json::json!({ "request": request })).unwrap()
    }

    #[test]
    fn defaults_and_titles_are_bounded() {
        let mut candidate = item("  Une première ligne utile\nune seconde");
        normalize_new(&mut candidate).unwrap();
        assert_eq!(candidate.scope, "atelier");
        assert_eq!(candidate.engine, "auto");
        assert_eq!(derive_title(&candidate.request), "Une première ligne utile");

        let long = "é".repeat(100);
        let title = derive_title(&long);
        assert_eq!(title.chars().count(), 97);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn validation_rejects_unsafe_values() {
        let mut candidate = item("faire quelque chose");
        candidate.scope = "../autre".into();
        assert!(normalize_new(&mut candidate).is_err());
        assert!(
            validate_patch(&BacklogPatch {
                lane: Some("running".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_patch(&BacklogPatch {
                engine: Some("shell".into()),
                ..Default::default()
            })
            .is_err()
        );
    }
}
