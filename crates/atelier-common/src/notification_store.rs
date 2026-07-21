//! Notification store (table `platform_notifications` dans `atelier_meta`).
//!
//! Canal **agent → utilisateur** : notifications volontaires (tool MCP
//! `notify_user`, kind=`notice`) et journal automatique des actions plateforme
//! des agents (kind=`action`, inséré par le dispatch MCP). WHY le store porte
//! le sender du canal `notify` : insert + publish sont indissociables — aucun
//! call-site ne peut oublier le publish, et on ne publie jamais un event
//! fantôme non persisté (publish uniquement après insert OK).
//!
//! Sémantique journal : les entrées `kind=action` naissent **lues**
//! (`read_at = created_at`) — elles ne gonflent jamais le compteur non-lus ni
//! le badge, elles se consultent dans le tiroir Notifications.
//!
//! Dégrade en no-op / vide quand le pool est absent (Postgres down au boot) —
//! mirror de [`crate::issue_store::PlatformIssueStore`].

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::control_db::sqlx::{PgPool, PgRow, Pool, Postgres, Row, query};
use crate::events::NotifyEvent;

#[derive(Clone)]
pub struct NotificationStore {
    pool: Option<Pool<Postgres>>,
    tx: broadcast::Sender<NotifyEvent>,
}

impl NotificationStore {
    pub fn new(pool: Option<PgPool>, tx: broadcast::Sender<NotifyEvent>) -> Self {
        Self { pool, tx }
    }

    fn pool(&self) -> Option<&Pool<Postgres>> {
        self.pool.as_ref()
    }

    fn no_pool() -> anyhow::Error {
        anyhow::anyhow!("control-plane Postgres (atelier_meta) indisponible")
    }

    fn publish(&self, ev: NotifyEvent) {
        // Err = aucun abonné WS pour l'instant : normal, pas une erreur.
        let _ = self.tx.send(ev);
    }

    /// Insère une notification puis publie `action:"created"` sur le canal
    /// `notify`. `source`/`kind`/`level` inconnus sont coercés vers leur défaut.
    /// Renvoie l'entrée stockée.
    pub async fn push(
        &self,
        slug: Option<&str>,
        source: &str,
        kind: &str,
        level: &str,
        title: &str,
        body: Option<&str>,
    ) -> anyhow::Result<Value> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        // 'pilot' : notifications émises par les hooks Pilote (rapport du matin,
        // item bloqué, questions d'un run, auth) — le front route ce source vers
        // la page /backlog au clic.
        let source = coerce(source, &["agent", "scan", "system", "user", "pilot"], "system");
        let kind = coerce(kind, &["notice", "action"], "notice");
        let level = coerce(level, &["info", "warn", "error"], "info");
        let id = format!("ntf-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let now = Utc::now();
        // kind=action né lu : sémantique journal (jamais dans le compteur unread).
        let read_at = (kind == "action").then_some(now);
        query(
            "INSERT INTO platform_notifications \
                (id, slug, source, kind, level, title, body, created_at, read_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(slug)
        .bind(source)
        .bind(kind)
        .bind(level)
        .bind(title)
        .bind(body)
        .bind(now)
        .bind(read_at)
        .execute(pool)
        .await?;
        info!(id = %id, slug = ?slug, source, kind, level, "PlatformNotify");
        let entry = json!({
            "id": id,
            "ts": rfc3339(now),
            "slug": slug,
            "source": source,
            "kind": kind,
            "level": level,
            "title": title,
            "body": body,
            "read_at": read_at.map(rfc3339),
        });
        self.publish(NotifyEvent {
            action: "created".into(),
            id: Some(id.clone()),
            ts: rfc3339(now),
            slug: slug.map(String::from),
            source: source.into(),
            kind: kind.into(),
            level: level.into(),
            title: Some(title.into()),
            body: body.map(String::from),
            read_at: read_at.map(rfc3339),
        });
        Ok(entry)
    }

    /// Liste (récent d'abord), filtres `unread_only`/`slug`, limit clampé
    /// 1..=500 (défaut 100). Renvoie aussi le compte unread GLOBAL (pour le
    /// badge, indépendant des filtres). Vide si pool absent.
    pub async fn list(&self, unread_only: bool, slug: Option<&str>, limit: i64) -> (Vec<Value>, i64) {
        let Some(pool) = self.pool() else {
            return (Vec::new(), 0);
        };
        let limit = limit.clamp(1, 500);
        let rows = query(
            "SELECT id, slug, source, kind, level, title, body, created_at, read_at \
               FROM platform_notifications \
              WHERE ($1::bool = false OR read_at IS NULL) \
                AND ($2::text IS NULL OR slug = $2) \
              ORDER BY created_at DESC \
              LIMIT $3",
        )
        .bind(unread_only)
        .bind(slug)
        .bind(limit)
        .fetch_all(pool)
        .await;
        let items = match rows {
            Ok(rows) => rows.iter().map(row_to_json).collect(),
            Err(e) => {
                error!(error = %e, "platform_notifications list failed");
                Vec::new()
            }
        };
        let unread = query("SELECT count(*) AS n FROM platform_notifications WHERE read_at IS NULL")
            .fetch_one(pool)
            .await
            .map(|r| r.get::<i64, _>("n"))
            .unwrap_or(0);
        (items, unread)
    }

    /// Marque lue (idempotent : déjà lue → renvoie l'entrée telle quelle).
    /// `Ok(None)` si l'id est introuvable. Publie `action:"read"`.
    pub async fn mark_read(&self, id: &str) -> anyhow::Result<Option<Value>> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        let row = query(
            "UPDATE platform_notifications \
                SET read_at = COALESCE(read_at, now()) \
              WHERE id = $1 \
          RETURNING id, slug, source, kind, level, title, body, created_at, read_at",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        let entry = row.map(|r| row_to_json(&r));
        if entry.is_some() {
            self.publish(NotifyEvent::mutation("read", Some(id)));
        }
        Ok(entry)
    }

    /// Marque tout lu. Renvoie le nombre de lignes touchées. Publie `read_all`.
    pub async fn mark_all_read(&self) -> anyhow::Result<u64> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        let res = query("UPDATE platform_notifications SET read_at = now() WHERE read_at IS NULL")
            .execute(pool)
            .await?;
        self.publish(NotifyEvent::mutation("read_all", None));
        Ok(res.rows_affected())
    }

    /// `Ok(true)` si supprimée, `Ok(false)` si id absent. Publie `deleted`.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let pool = self.pool().ok_or_else(Self::no_pool)?;
        let res = query("DELETE FROM platform_notifications WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        let removed = res.rows_affected() > 0;
        if removed {
            self.publish(NotifyEvent::mutation("deleted", Some(id)));
        }
        Ok(removed)
    }

    /// Purge des notifications d'une app (hook AppDelete). Best-effort : un
    /// échec est loggué, jamais propagé (mirror issues.delete_by_slug).
    pub async fn delete_by_slug(&self, slug: &str) {
        let Some(pool) = self.pool() else { return };
        if let Err(e) = query("DELETE FROM platform_notifications WHERE slug = $1")
            .bind(slug)
            .execute(pool)
            .await
        {
            error!(slug, error = %e, "platform_notifications delete_by_slug failed");
        }
    }

    /// Purge du journal d'actions > 30 j (appelée au boot — anti-accumulation ;
    /// les `notice` sont gérées à la main par l'utilisateur, jamais purgées ici).
    pub async fn prune_old_actions(&self) {
        let Some(pool) = self.pool() else { return };
        match query(
            "DELETE FROM platform_notifications \
              WHERE kind = 'action' AND created_at < now() - interval '30 days'",
        )
        .execute(pool)
        .await
        {
            Ok(res) if res.rows_affected() > 0 => {
                info!(pruned = res.rows_affected(), "platform_notifications: journal d'actions purgé (>30j)");
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "platform_notifications prune failed"),
        }
    }
}

/// Coerce une valeur vers son enum : inconnue → défaut (le store reste la seule
/// autorité des valeurs possibles, quel que soit l'appelant MCP/HTTP).
fn coerce<'a>(v: &'a str, allowed: &[&'a str], default: &'a str) -> &'a str {
    if allowed.contains(&v) { v } else { default }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn row_to_json(row: &PgRow) -> Value {
    let created: DateTime<Utc> = row.get("created_at");
    let read_at: Option<DateTime<Utc>> = row.try_get("read_at").ok().flatten();
    json!({
        "id": row.get::<String, _>("id"),
        "ts": rfc3339(created),
        "slug": row.try_get::<Option<String>, _>("slug").ok().flatten(),
        "source": row.get::<String, _>("source"),
        "kind": row.get::<String, _>("kind"),
        "level": row.get::<String, _>("level"),
        "title": row.get::<String, _>("title"),
        "body": row.try_get::<Option<String>, _>("body").ok().flatten(),
        "read_at": read_at.map(rfc3339),
    })
}
