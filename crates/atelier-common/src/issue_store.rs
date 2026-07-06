//! Platform-issue store (table `platform_issues` dans `atelier_meta`).
//!
//! Canal de remontée des frictions **plateforme** signalées par les chats Claude
//! Code des apps (Studio) via la skill `0-report-issue` (`POST /api/apps/{slug}/
//! issues`). WHY centralisé ici : la feature concerne des bugs de la PLATEFORME,
//! pas des apps — le store appartient au control-plane Atelier, plus à l'arbre
//! source de chaque app. L'ancien `CLAUDE_ISSUES.json` per-app a été rapatrié une
//! fois puis supprimé ([`PlatformIssueStore::backfill_from_files`]).
//!
//! La forme JSON renvoyée étend l'historique (`id, ts, app, kind, area,
//! severity, title, context, tried, status, note?, updated_at?`) — `kind`
//! (error | limitation | suggestion) est l'axe de nature ajouté en 2026-07.
//!
//! Le store est l'unique autorité des enums (`coerce`, mirror de
//! [`crate::notification_store`]) et porte le sender du canal EventBus `issue` :
//! chaque mutation réussie publie un [`IssueEvent`] (jamais d'event fantôme),
//! relayé en WS `issue:event` pour la page /issues + la pastille sidebar.
//!
//! Dégrade en no-op / vide quand le pool est absent (Postgres down au boot) —
//! mirror de [`crate::task_store::TaskStore`].

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::control_db::sqlx::{PgPool, PgRow, Pool, Postgres, Row, query};
use crate::events::IssueEvent;

pub const ISSUE_KINDS: &[&str] = &["error", "limitation", "suggestion"];
pub const ISSUE_SEVERITIES: &[&str] = &["low", "medium", "high"];
pub const ISSUE_STATUSES: &[&str] = &["open", "resolved", "dismissed"];
pub const ISSUE_AREAS: &[&str] = &[
    "mcp", "docs", "build", "deploy", "dataverse", "agent", "studio-ui", "platform", "other",
];

#[derive(Clone)]
pub struct PlatformIssueStore {
    pool: Option<Pool<Postgres>>,
    tx: broadcast::Sender<IssueEvent>,
}

impl PlatformIssueStore {
    pub fn new(pool: Option<PgPool>, tx: broadcast::Sender<IssueEvent>) -> Self {
        Self { pool, tx }
    }

    fn pool(&self) -> Option<&Pool<Postgres>> {
        self.pool.as_ref()
    }

    fn no_pool() -> anyhow::Error {
        anyhow::anyhow!("control-plane Postgres (atelier_meta) indisponible")
    }

    fn publish(&self, ev: IssueEvent) {
        // Err = aucun abonné WS pour l'instant : normal, pas une erreur.
        let _ = self.tx.send(ev);
    }

    /// Ajoute une remontée. Le serveur estampe `id`/`created_at`/`status:open` ;
    /// `kind`/`area`/`severity` inconnus sont coercés vers leur défaut. Renvoie
    /// l'entrée stockée et publie `action:"created"`.
    pub async fn insert(
        &self,
        slug: &str,
        kind: &str,
        area: &str,
        severity: &str,
        title: &str,
        context: &str,
        tried: &str,
    ) -> anyhow::Result<Value> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        let kind = coerce(kind, ISSUE_KINDS, "error");
        let area = coerce(area, ISSUE_AREAS, "other");
        let severity = coerce(severity, ISSUE_SEVERITIES, "medium");
        let id = format!("iss-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let now = Utc::now();
        query(
            "INSERT INTO platform_issues \
                (id, slug, kind, area, severity, title, context, tried, status, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'open', $9)",
        )
        .bind(&id)
        .bind(slug)
        .bind(kind)
        .bind(area)
        .bind(severity)
        .bind(title)
        .bind(context)
        .bind(tried)
        .bind(now)
        .execute(pool)
        .await?;
        let entry = json!({
            "id": id,
            "ts": rfc3339(now),
            "app": slug,
            "kind": kind,
            "area": area,
            "severity": severity,
            "title": title,
            "context": context,
            "tried": tried,
            "status": "open",
        });
        self.publish(IssueEvent {
            action: "created".into(),
            id,
            item: Some(entry.clone()),
        });
        Ok(entry)
    }

    /// Liste agrégée, filtres optionnels `status` / `slug`. Tri serveur : rang de
    /// sévérité (high→low), puis slug, puis date (récent en dernier — stable).
    pub async fn list(&self, status: Option<&str>, slug: Option<&str>) -> Vec<Value> {
        let Some(pool) = self.pool() else {
            return Vec::new();
        };
        let rows = query(
            "SELECT id, slug, kind, area, severity, title, context, tried, status, note, created_at, updated_at \
               FROM platform_issues \
              WHERE ($1::text IS NULL OR status = $1) \
                AND ($2::text IS NULL OR slug = $2) \
              ORDER BY CASE severity \
                         WHEN 'high' THEN 0 WHEN 'medium' THEN 1 WHEN 'low' THEN 2 ELSE 3 END, \
                       slug, created_at",
        )
        .bind(status)
        .bind(slug)
        .fetch_all(pool)
        .await;
        match rows {
            Ok(rows) => rows.iter().map(row_to_json).collect(),
            Err(e) => {
                error!(error = %e, "platform_issues list failed");
                Vec::new()
            }
        }
    }

    /// Met à jour `status` et/ou `note` (COALESCE : champ absent = inchangé),
    /// estampe `updated_at`, publie `action:"updated"`. Un `status` hors enum
    /// est ignoré (= inchangé) plutôt que coercé — un typo ne doit pas rouvrir
    /// une issue. `Ok(None)` si l'`id` est introuvable.
    pub async fn update(
        &self,
        id: &str,
        status: Option<&str>,
        note: Option<&str>,
    ) -> anyhow::Result<Option<Value>> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        let status = status.filter(|s| ISSUE_STATUSES.contains(s));
        let row = query(
            "UPDATE platform_issues \
                SET status = COALESCE($2, status), \
                    note = COALESCE($3, note), \
                    updated_at = now() \
              WHERE id = $1 \
          RETURNING id, slug, kind, area, severity, title, context, tried, status, note, created_at, updated_at",
        )
        .bind(id)
        .bind(status)
        .bind(note)
        .fetch_optional(pool)
        .await?;
        let entry = row.map(|r| row_to_json(&r));
        if let Some(entry) = &entry {
            self.publish(IssueEvent {
                action: "updated".into(),
                id: id.to_string(),
                item: Some(entry.clone()),
            });
        }
        Ok(entry)
    }

    /// Supprime une remontée et publie `action:"deleted"`. `Ok(true)` si
    /// supprimée, `Ok(false)` si id absent.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        let res = query("DELETE FROM platform_issues WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        let deleted = res.rows_affected() > 0;
        if deleted {
            self.publish(IssueEvent {
                action: "deleted".into(),
                id: id.to_string(),
                item: None,
            });
        }
        Ok(deleted)
    }

    /// Purge toutes les issues d'une app (hook AppDelete). Best-effort : un échec
    /// est loggué, jamais propagé (le delete d'app ne doit pas échouer pour ça).
    /// Publie un `deleted` par ligne purgée (volumes minuscules) pour que les
    /// clients WS retirent les entrées sans refetch.
    pub async fn delete_by_slug(&self, slug: &str) {
        let Some(pool) = self.pool() else { return };
        match query("DELETE FROM platform_issues WHERE slug = $1 RETURNING id")
            .bind(slug)
            .fetch_all(pool)
            .await
        {
            Ok(rows) => {
                for r in rows {
                    self.publish(IssueEvent {
                        action: "deleted".into(),
                        id: r.get::<String, _>("id"),
                        item: None,
                    });
                }
            }
            Err(e) => error!(slug, error = %e, "platform_issues delete_by_slug failed"),
        }
    }

    /// One-shot : rapatrie les anciens `CLAUDE_ISSUES.json` per-app vers la base
    /// PUIS supprime les fichiers (réalise l'intention : plus aucun store au
    /// niveau projet). Idempotent : `INSERT ... ON CONFLICT (id) DO NOTHING` ;
    /// une fois les fichiers partis, le scan suivant ne trouve rien. No-op sans
    /// pool ou si l'arbre apps est absent (env de dev). Un fichier qui ne parse
    /// pas est laissé en place (jamais de perte silencieuse).
    pub async fn backfill_from_files(&self, apps_src_root: &Path) {
        let Some(pool) = self.pool() else { return };
        let dir = match std::fs::read_dir(apps_src_root) {
            Ok(d) => d,
            Err(_) => return,
        };
        let (mut scanned, mut imported, mut files_removed) = (0u32, 0u32, 0u32);
        for ent in dir.flatten() {
            let path = ent.path().join("src").join("CLAUDE_ISSUES.json");
            if !path.is_file() {
                continue;
            }
            scanned += 1;
            let slug = ent.file_name().to_string_lossy().to_string();
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "backfill: read failed, file kept");
                    continue;
                }
            };
            let items: Vec<Value> = if raw.trim().is_empty() {
                Vec::new()
            } else {
                match serde_json::from_str(&raw) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "backfill: parse failed, file kept");
                        continue;
                    }
                }
            };
            let mut all_ok = true;
            for it in &items {
                let id = it
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| {
                        format!("iss-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
                    });
                let app = it.get("app").and_then(|v| v.as_str()).unwrap_or(slug.as_str());
                let created = it
                    .get("ts")
                    .and_then(|v| v.as_str())
                    .and_then(parse_ts)
                    .unwrap_or_else(Utc::now);
                let updated = it.get("updated_at").and_then(|v| v.as_str()).and_then(parse_ts);
                let res = query(
                    "INSERT INTO platform_issues \
                        (id, slug, area, severity, title, context, tried, status, note, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(&id)
                .bind(app)
                .bind(it.get("area").and_then(|v| v.as_str()).unwrap_or("other"))
                .bind(it.get("severity").and_then(|v| v.as_str()).unwrap_or("medium"))
                .bind(it.get("title").and_then(|v| v.as_str()).unwrap_or(""))
                .bind(it.get("context").and_then(|v| v.as_str()).unwrap_or(""))
                .bind(it.get("tried").and_then(|v| v.as_str()).unwrap_or(""))
                .bind(it.get("status").and_then(|v| v.as_str()).unwrap_or("open"))
                .bind(it.get("note").and_then(|v| v.as_str()))
                .bind(created)
                .bind(updated)
                .execute(pool)
                .await;
                match res {
                    Ok(r) => imported += r.rows_affected() as u32,
                    Err(e) => {
                        error!(id = %id, error = %e, "backfill: insert failed, file kept");
                        all_ok = false;
                    }
                }
            }
            // On ne supprime le fichier que si tout a été (idempotemment) importé.
            // Un tableau vide passe par ici aussi → le fichier vide est retiré.
            if all_ok {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!(path = %path.display(), error = %e, "backfill: remove file failed");
                } else {
                    files_removed += 1;
                }
            }
        }
        if scanned > 0 {
            info!(
                scanned,
                imported,
                files_removed,
                "platform_issues backfill: rapatriement CLAUDE_ISSUES.json → atelier_meta"
            );
        }
    }
}

fn coerce<'a>(v: &'a str, allowed: &[&'a str], default: &'a str) -> &'a str {
    if allowed.contains(&v) { v } else { default }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn row_to_json(row: &PgRow) -> Value {
    let created: DateTime<Utc> = row.get("created_at");
    let updated: Option<DateTime<Utc>> = row.try_get("updated_at").ok().flatten();
    let note: Option<String> = row.try_get("note").ok().flatten();
    let mut v = json!({
        "id": row.get::<String, _>("id"),
        "ts": rfc3339(created),
        "app": row.get::<String, _>("slug"),
        "kind": row.get::<String, _>("kind"),
        "area": row.get::<String, _>("area"),
        "severity": row.get::<String, _>("severity"),
        "title": row.get::<String, _>("title"),
        "context": row.try_get::<Option<String>, _>("context").ok().flatten().unwrap_or_default(),
        "tried": row.try_get::<Option<String>, _>("tried").ok().flatten().unwrap_or_default(),
        "status": row.get::<String, _>("status"),
    });
    if let Some(n) = note {
        v["note"] = json!(n);
    }
    if let Some(u) = updated {
        v["updated_at"] = json!(rfc3339(u));
    }
    v
}
