//! Store d'authentification du moteur **Codex** (table `codex_auth` singleton
//! dans `atelier_meta`).
//!
//! Clone fonctionnel de [`crate::agent_auth`] (mêmes méthodes, même dédup de
//! notification), mais avec une différence de nature à ne pas perdre de vue :
//!
//! - Pour Claude, le token PORTÉ ICI est le credential effectivement injecté au
//!   runner (stdin) : la base fait autorité.
//! - Pour Codex, la vérité runtime est le fichier `$CODEX_HOME/auth.json`, lu ET
//!   réécrit par le CLI lui-même (il rotate son refresh token). La base ne porte
//!   donc qu'un **seed** : le contenu d'`auth.json` collé depuis Paramètres,
//!   conservé pour restaurer le fichier après une perte de `/var/lib`, servir le
//!   statut à l'UI et dédupliquer les notifications d'expiration.
//!
//! Conséquence directe : le flow `device-login` écrit `auth.json` via le CLI sans
//! jamais passer par PG — un `configured=false` avec un `auth.json` valide est un
//! état NORMAL. C'est l'appelant (route `/api/agent/codex/auth`) qui compose le
//! statut de la base avec la présence du fichier (`auth_file`).
//!
//! Auth = OAuth abonnement ChatGPT uniquement : aucune clé API n'est acceptée ici
//! ni ailleurs dans la chaîne Codex.
//!
//! `record_failure` porte la **dédup atomique** des notifications, sur le même
//! principe qu'`agent_auth` : le claim de `last_notified_at` sur la ligne unique
//! élit un seul notifiant par intervalle, quel que soit le nombre de tours
//! concurrents qui butent sur l'auth morte. `record_ok` réarme le watermark → la
//! panne suivante notifie immédiatement (transition sain → cassé). L'intervalle
//! est celui d'[`crate::agent_auth::notify_interval_secs`] (partagé, pas dupliqué :
//! c'est la même doctrine de débounce côté utilisateur).
//!
//! Dégrade en no-op / `None` quand le pool est absent (Postgres down au boot).
//! Aucun sender EventBus : la notification est poussée par l'appelant.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use tracing::{error, info};

use crate::control_db::sqlx::{PgPool, Pool, Postgres, Row, query};

#[derive(Clone)]
pub struct CodexAuthStore {
    pool: Option<Pool<Postgres>>,
}

impl CodexAuthStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Seed `auth.json` persisté, pour (ré)hydrater `$CODEX_HOME/auth.json` quand
    /// le fichier manque. `None` = jamais collé (→ l'auth ne peut venir que d'un
    /// device-login) ou pool absent.
    pub async fn token(&self) -> Option<String> {
        let pool = self.pool.as_ref()?;
        match query("SELECT token FROM codex_auth WHERE id = 1")
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
                error!(error = %e, "codex_auth token read failed");
                None
            }
        }
    }

    /// Vue MASQUÉE pour l'API — ne renvoie JAMAIS le contenu du seed, seulement
    /// `configured` + la télémétrie d'auth. `configured=false` quand pool absent.
    /// L'appelant y ajoute `auth_file` (présence du fichier, seule vérité runtime).
    pub async fn status(&self) -> Value {
        let Some(pool) = self.pool.as_ref() else {
            return json!({ "configured": false, "available": false });
        };
        match query(
            "SELECT (token IS NOT NULL AND token <> '') AS configured, \
                    updated_at, last_ok_at, last_error_at, last_error_msg, last_notified_at \
               FROM codex_auth WHERE id = 1",
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
                error!(error = %e, "codex_auth status read failed");
                json!({ "configured": false, "available": true })
            }
        }
    }

    /// Persiste un seed `auth.json` VALIDÉ (le tour réel isolé a réussi côté
    /// appelant, qui a aussi écrit le fichier) et estampe un état sain.
    pub async fn set_token(&self, token: &str) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("control-plane Postgres (atelier_meta) indisponible");
        };
        query(
            "UPDATE codex_auth SET \
                token = $1, updated_at = now(), \
                last_ok_at = now(), last_error_at = NULL, last_error_msg = NULL, \
                last_notified_at = NULL \
             WHERE id = 1",
        )
        .bind(token)
        .execute(pool)
        .await?;
        info!(token_len = token.len(), "codex_auth token set"); // JAMAIS la valeur
        Ok(())
    }

    /// Retire le seed. L'appelant efface AUSSI `$CODEX_HOME/auth.json` — sinon
    /// Codex resterait authentifié par le fichier seul.
    pub async fn clear_token(&self) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("control-plane Postgres (atelier_meta) indisponible");
        };
        query("UPDATE codex_auth SET token = NULL, updated_at = now() WHERE id = 1")
            .execute(pool)
            .await?;
        info!("codex_auth token cleared");
        Ok(())
    }

    /// Signal sain (tour authentifié, smoke-test OK, device-login abouti) : réarme
    /// le watermark de notif → la panne SUIVANTE notifiera immédiatement.
    /// Best-effort.
    pub async fn record_ok(&self) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            "UPDATE codex_auth SET \
                last_ok_at = now(), last_error_at = NULL, last_error_msg = NULL, \
                last_notified_at = NULL \
             WHERE id = 1",
        )
        .execute(pool)
        .await
        {
            error!(error = %e, "codex_auth record_ok failed");
        }
    }

    /// Enregistre une panne d'auth ; renvoie `true` SSI CE caller a gagné le
    /// créneau de notification. WHY le dédup vit dans la ligne DB (pas par-process) :
    /// une auth Codex morte touche chaque tour de chaque conversation — le claim
    /// atomique du `last_notified_at` (verrou de ligne Postgres) élit un seul
    /// gagnant par intervalle, quel que soit le nombre de runs concurrents. Pool
    /// absent → false.
    pub async fn record_failure(&self, msg: &str, min_interval_secs: i64) -> bool {
        let Some(pool) = self.pool.as_ref() else {
            return false;
        };
        // 1) toujours consigner la dernière erreur (diagnostic).
        let _ = query("UPDATE codex_auth SET last_error_at = now(), last_error_msg = $1 WHERE id = 1")
            .bind(msg)
            .execute(pool)
            .await;
        // 2) claim atomique du créneau notif (débounce).
        match query(
            "UPDATE codex_auth SET last_notified_at = now() \
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
                error!(error = %e, "codex_auth record_failure claim failed");
                false
            }
        }
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}
