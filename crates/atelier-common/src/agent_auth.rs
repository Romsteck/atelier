//! Store d'authentification du Claude Agent SDK (table `agent_auth` singleton
//! dans `atelier_meta`).
//!
//! Le runner (chat Studio) et le scan de surveillance pilotent le SDK en OAuth
//! abonnement. Tant que le refresh token vit, le SDK renouvelle seul l'access
//! token — rien à faire. Mais quand le refresh token MEURT (expiré/révoqué), le
//! SDK renvoie `authentication_failed` et, le runner étant headless en
//! `hr-studio`, on ne peut pas y relancer `claude login` (flow navigateur). Ce
//! store porte un **token longue durée** (`claude setup-token`, ~1 an, inference-
//! only) que Romain recolle depuis Paramètres ; il est relu FRAIS à chaque run et
//! injecté au runner/scan par stdin (jamais argv/env — anti-leak journalctl,
//! comme MCP_TOKEN), donc une ré-auth s'applique sans redémarrer le service.
//!
//! `record_failure` porte la **dédup atomique** des notifications : un token mort
//! touche chaque scan du sweep (≥10 apps × 3 scans) + l'agent interactif ; le
//! claim de `last_notified_at` sur la ligne unique effondre tout en une seule
//! notification par intervalle. `record_ok` remet ce watermark à NULL → la panne
//! suivante notifie immédiatement (transition sain → cassé).
//!
//! Dégrade en no-op / `None` quand le pool est absent (Postgres down au boot) —
//! mirror de [`crate::conversation_meta::ConversationMetaStore`]. Aucun sender
//! EventBus : la notification est poussée par l'appelant (API/watcher).

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use tracing::{error, info};

use crate::control_db::sqlx::{PgPool, Pool, Postgres, Row, query};

#[derive(Clone)]
pub struct AgentAuthStore {
    pool: Option<Pool<Postgres>>,
}

impl AgentAuthStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Token frais pour injection stdin (runner + scan). `None` = non configuré
    /// (→ le runner retombe sur un `.credentials.json` présent) ou pool absent.
    pub async fn token(&self) -> Option<String> {
        let pool = self.pool.as_ref()?;
        match query("SELECT token FROM agent_auth WHERE id = 1")
            .fetch_optional(pool)
            .await
        {
            Ok(Some(row)) => row
                .try_get::<Option<String>, _>("token")
                .ok()
                .flatten()
                .filter(|t| !t.is_empty()),
            Ok(None) => None,
            Err(e) => {
                error!(error = %e, "agent_auth token read failed");
                None
            }
        }
    }

    /// Vue MASQUÉE pour l'API — ne renvoie JAMAIS la valeur du token, seulement
    /// `configured` + la télémétrie d'auth. `configured=false` quand pool absent.
    pub async fn status(&self) -> Value {
        let Some(pool) = self.pool.as_ref() else {
            return json!({ "configured": false, "available": false });
        };
        match query(
            "SELECT (token IS NOT NULL AND token <> '') AS configured, \
                    updated_at, last_ok_at, last_error_at, last_error_msg, last_notified_at \
               FROM agent_auth WHERE id = 1",
        )
        .fetch_optional(pool)
        .await
        {
            Ok(Some(row)) => {
                let ts = |c: &str| {
                    row.try_get::<Option<DateTime<Utc>>, _>(c)
                        .ok()
                        .flatten()
                        .map(rfc3339)
                };
                json!({
                    "configured": row.try_get::<bool, _>("configured").unwrap_or(false),
                    "available": true,
                    "updated_at": ts("updated_at"),
                    "last_ok_at": ts("last_ok_at"),
                    "last_error_at": ts("last_error_at"),
                    "last_error_msg": row.try_get::<Option<String>, _>("last_error_msg").ok().flatten(),
                    "last_notified_at": ts("last_notified_at"),
                })
            }
            // Row seedée par la migration : Ok(None) ne devrait pas arriver, mais
            // on dégrade proprement plutôt que de paniquer.
            Ok(None) => json!({ "configured": false, "available": true }),
            Err(e) => {
                error!(error = %e, "agent_auth status read failed");
                json!({ "configured": false, "available": true })
            }
        }
    }

    /// Persiste un token VALIDÉ (le smoke-test a réussi côté appelant) et estampe
    /// un état sain (`record_ok` inline : last_ok_at=now, erreur effacée, watermark
    /// de notif réarmé).
    pub async fn set_token(&self, token: &str) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("control-plane Postgres (atelier_meta) indisponible");
        };
        query(
            "UPDATE agent_auth SET \
                token = $1, updated_at = now(), \
                last_ok_at = now(), last_error_at = NULL, last_error_msg = NULL, \
                last_notified_at = NULL \
             WHERE id = 1",
        )
        .bind(token)
        .execute(pool)
        .await?;
        info!(token_len = token.len(), "agent_auth token set"); // JAMAIS la valeur
        Ok(())
    }

    /// Retire le token (retour au fallback `.credentials.json`).
    pub async fn clear_token(&self) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("control-plane Postgres (atelier_meta) indisponible");
        };
        query("UPDATE agent_auth SET token = NULL, updated_at = now() WHERE id = 1")
            .execute(pool)
            .await?;
        info!("agent_auth token cleared");
        Ok(())
    }

    /// Signal sain (smoke-test OK, ou run authentifié) : réarme le watermark de
    /// notif → la panne SUIVANTE notifiera immédiatement. Best-effort.
    pub async fn record_ok(&self) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            "UPDATE agent_auth SET \
                last_ok_at = now(), last_error_at = NULL, last_error_msg = NULL, \
                last_notified_at = NULL \
             WHERE id = 1",
        )
        .execute(pool)
        .await
        {
            error!(error = %e, "agent_auth record_ok failed");
        }
    }

    /// Enregistre une panne d'auth ; renvoie `true` SSI CE caller a gagné le
    /// créneau de notification. WHY le dédup vit dans la ligne DB (pas par-process) :
    /// un token mort touche chaque scan du sweep + l'agent — le claim atomique du
    /// `last_notified_at` (verrou de ligne Postgres) élit un seul gagnant par
    /// intervalle, quel que soit le nombre de runs concurrents. Pool absent → false.
    pub async fn record_failure(&self, msg: &str, min_interval_secs: i64) -> bool {
        let Some(pool) = self.pool.as_ref() else {
            return false;
        };
        // 1) toujours consigner la dernière erreur (diagnostic).
        let _ = query("UPDATE agent_auth SET last_error_at = now(), last_error_msg = $1 WHERE id = 1")
            .bind(msg)
            .execute(pool)
            .await;
        // 2) claim atomique du créneau notif (débounce).
        match query(
            "UPDATE agent_auth SET last_notified_at = now() \
               WHERE id = 1 \
                 AND (last_notified_at IS NULL \
                      OR last_notified_at < now() - make_interval(secs => $1)) \
             RETURNING id",
        )
        .bind(min_interval_secs as f64)
        .fetch_optional(pool)
        .await
        {
            Ok(claimed) => claimed.is_some(),
            Err(e) => {
                error!(error = %e, "agent_auth record_failure claim failed");
                false
            }
        }
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Intervalle minimal (secondes) entre deux notifications d'échec d'auth SDK —
/// passé à [`AgentAuthStore::record_failure`]. Un token mort touche chaque scan du
/// sweep + l'agent interactif → sans ce débounce on spammerait le tiroir. Défaut
/// 6 h ; surchargeable via `ATELIER_AGENT_AUTH_NOTIFY_INTERVAL_SECS`. Partagé par
/// la route agent (API interactive) et le sink du watcher (main.rs).
pub fn notify_interval_secs() -> i64 {
    std::env::var("ATELIER_AGENT_AUTH_NOTIFY_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(21_600)
}
