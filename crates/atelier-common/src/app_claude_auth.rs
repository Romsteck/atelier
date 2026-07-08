//! Store du token Claude **destiné aux apps** (table `app_claude_auth` singleton
//! dans `atelier_meta`).
//!
//! Distinct de [`crate::agent_auth`] (token du runner/scan de la PLATEFORME) : ce
//! token-ci est injecté aux **process applicatifs** opt-in (`Application.claude_access`)
//! comme variable plateforme calculée `CLAUDE_CODE_OAUTH_TOKEN`, à la façon de
//! `HR_DV_*`. WHY un token séparé plutôt que réutiliser `agent_auth` : une app est
//! un tiers moins fiable que l'agent plateforme — un token dédié, que Romain
//! provisionne et révoque indépendamment (`claude setup-token`, ~1 an, inference-
//! only), garantit qu'une compromission d'app ne fuite jamais le credential qui
//! pilote le Studio/la surveillance.
//!
//! Le token remplace le hack où une app pointait `CLAUDE_CONFIG_DIR` sur le dossier
//! de config du runner (`/var/lib/hr-studio/.claude`) et, tournant en root,
//! clobberait `.credentials.json` en `root:root` — cassant toute la pile agent
//! hr-studio (cf. iss-d10ef97b). Avec le token en env, l'app n'a plus aucun besoin
//! de config-dir ni de fichier partagé → collision structurellement impossible.
//!
//! En clair (base root-only, même exposition que `agent_auth` / `dataverse-secrets`
//! / le `.env` rendu). Dégrade en no-op / `None` quand le pool est absent.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use tracing::{error, info};

use crate::control_db::sqlx::{PgPool, Pool, Postgres, Row, query};

#[derive(Clone)]
pub struct AppClaudeAuthStore {
    pool: Option<Pool<Postgres>>,
}

impl AppClaudeAuthStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    /// Token frais pour l'injection env des apps (relu à chaque render `.env`).
    /// `None` = non configuré (→ pas de `CLAUDE_CODE_OAUTH_TOKEN` injecté) ou pool
    /// absent.
    pub async fn token(&self) -> Option<String> {
        let pool = self.pool.as_ref()?;
        match query("SELECT token FROM app_claude_auth WHERE id = 1")
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
                error!(error = %e, "app_claude_auth token read failed");
                None
            }
        }
    }

    /// Vue MASQUÉE pour l'API — ne renvoie JAMAIS la valeur du token, seulement
    /// `configured` + la télémétrie. `configured=false` quand pool absent.
    pub async fn status(&self) -> Value {
        let Some(pool) = self.pool.as_ref() else {
            return json!({ "configured": false, "available": false });
        };
        match query(
            "SELECT (token IS NOT NULL AND token <> '') AS configured, \
                    updated_at, last_ok_at, last_error_at, last_error_msg \
               FROM app_claude_auth WHERE id = 1",
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
                })
            }
            Ok(None) => json!({ "configured": false, "available": true }),
            Err(e) => {
                error!(error = %e, "app_claude_auth status read failed");
                json!({ "configured": false, "available": true })
            }
        }
    }

    /// Persiste un token VALIDÉ (le smoke-test a réussi côté appelant) + estampe un
    /// état sain.
    pub async fn set_token(&self, token: &str) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("control-plane Postgres (atelier_meta) indisponible");
        };
        query(
            "UPDATE app_claude_auth SET \
                token = $1, updated_at = now(), \
                last_ok_at = now(), last_error_at = NULL, last_error_msg = NULL \
             WHERE id = 1",
        )
        .bind(token)
        .execute(pool)
        .await?;
        info!(token_len = token.len(), "app_claude_auth token set"); // JAMAIS la valeur
        Ok(())
    }

    /// Retire le token (les apps opt-in n'auront plus `CLAUDE_CODE_OAUTH_TOKEN`).
    pub async fn clear_token(&self) -> anyhow::Result<()> {
        let Some(pool) = self.pool.as_ref() else {
            anyhow::bail!("control-plane Postgres (atelier_meta) indisponible");
        };
        query("UPDATE app_claude_auth SET token = NULL, updated_at = now() WHERE id = 1")
            .execute(pool)
            .await?;
        info!("app_claude_auth token cleared");
        Ok(())
    }

    /// Signal sain (smoke-test OK). Best-effort.
    pub async fn record_ok(&self) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) = query(
            "UPDATE app_claude_auth SET \
                last_ok_at = now(), last_error_at = NULL, last_error_msg = NULL \
             WHERE id = 1",
        )
        .execute(pool)
        .await
        {
            error!(error = %e, "app_claude_auth record_ok failed");
        }
    }

    /// Consigne une panne d'auth (diagnostic). Best-effort.
    pub async fn record_failure(&self, msg: &str) {
        let Some(pool) = self.pool.as_ref() else { return };
        if let Err(e) =
            query("UPDATE app_claude_auth SET last_error_at = now(), last_error_msg = $1 WHERE id = 1")
                .bind(msg)
                .execute(pool)
                .await
        {
            error!(error = %e, "app_claude_auth record_failure failed");
        }
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}
