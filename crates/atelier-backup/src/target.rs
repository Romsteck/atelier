use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sqlx::{PgRow, Pool, Postgres, Row, query};

/// Vue API de la cible (secrets RÉDIGÉS : `has_password` / `has_restic_password`,
/// jamais les valeurs).
#[derive(Debug, Clone, Serialize)]
pub struct BackupTarget {
    pub kind: String,
    pub label: String,
    pub host: String,
    pub share: String,
    pub username: String,
    pub domain: String,
    pub has_password: bool,
    pub has_restic_password: bool,
    pub repo_subpath: String,
    pub schedule_enabled: bool,
    pub schedule_cadence: String,
    pub schedule_hour: i16,
    pub retention_keep: i32,
    pub updated_at: DateTime<Utc>,
}

/// Vue interne (secrets inclus) — sert à construire l'env restic/rclone.
#[derive(Debug, Clone)]
pub struct FullTarget {
    pub kind: String,
    pub host: String,
    pub share: String,
    pub username: String,
    pub domain: String,
    pub password: Option<String>,
    pub restic_password: Option<String>,
    pub repo_subpath: String,
    pub schedule_enabled: bool,
    pub schedule_cadence: String,
    pub schedule_hour: i16,
    pub retention_keep: i32,
}

impl FullTarget {
    /// Cible minimalement utilisable pour lancer un backup.
    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty()
            && !self.share.trim().is_empty()
            && !self.username.trim().is_empty()
    }
}

/// Corps d'un PUT /api/backup/target. `password` absent ⇒ conservé.
#[derive(Debug, Clone, Deserialize)]
pub struct NewTarget {
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    pub host: String,
    pub share: String,
    pub username: String,
    #[serde(default)]
    pub domain: String,
    /// None ⇒ conserver le mot de passe existant.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "default_repo_subpath")]
    pub repo_subpath: String,
    #[serde(default)]
    pub schedule_enabled: bool,
    #[serde(default = "default_cadence")]
    pub schedule_cadence: String,
    #[serde(default = "default_hour")]
    pub schedule_hour: i16,
    #[serde(default = "default_keep")]
    pub retention_keep: i32,
}

fn default_kind() -> String {
    "smb".to_string()
}
fn default_repo_subpath() -> String {
    "atelier-backup".to_string()
}
fn default_cadence() -> String {
    "daily".to_string()
}
fn default_hour() -> i16 {
    3
}
fn default_keep() -> i32 {
    7
}

impl NewTarget {
    /// Validation des champs (renvoie un message d'erreur lisible).
    pub fn validate(&self) -> Result<(), String> {
        if self.kind != "smb" {
            return Err(format!("kind must be 'smb' (got '{}')", self.kind));
        }
        if self.host.trim().is_empty() {
            return Err("host is required".into());
        }
        if self.share.trim().is_empty() {
            return Err("share is required".into());
        }
        if self.username.trim().is_empty() {
            return Err("username is required".into());
        }
        if !(0..=23).contains(&self.schedule_hour) {
            return Err("schedule_hour must be 0..23".into());
        }
        if self.retention_keep < 1 {
            return Err("retention_keep must be >= 1".into());
        }
        if !matches!(self.schedule_cadence.as_str(), "daily" | "weekly") {
            return Err("schedule_cadence must be 'daily' or 'weekly'".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> NewTarget {
        NewTarget {
            kind: "smb".into(),
            label: String::new(),
            host: "nas".into(),
            share: "backups".into(),
            username: "u".into(),
            domain: String::new(),
            password: None,
            repo_subpath: "atelier-backup".into(),
            schedule_enabled: false,
            schedule_cadence: "daily".into(),
            schedule_hour: 3,
            retention_keep: 7,
        }
    }

    #[test]
    fn valid_target_passes() {
        assert!(base().validate().is_ok());
    }

    #[test]
    fn rejects_missing_fields_and_bad_values() {
        let mut t = base();
        t.host = "  ".into();
        assert!(t.validate().is_err());
        let mut t = base();
        t.schedule_hour = 24;
        assert!(t.validate().is_err());
        let mut t = base();
        t.retention_keep = 0;
        assert!(t.validate().is_err());
        let mut t = base();
        t.kind = "nfs".into();
        assert!(t.validate().is_err());
    }
}

#[derive(Clone)]
pub struct TargetStore {
    pool: Pool<Postgres>,
}

impl TargetStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    /// Vue API (secrets rédigés).
    pub async fn get_redacted(&self) -> anyhow::Result<Option<BackupTarget>> {
        let row: Option<PgRow> = query(
            r#"
            SELECT kind, label, host, share, username, domain,
                   (password IS NOT NULL AND password <> '')               AS has_password,
                   (restic_password IS NOT NULL AND restic_password <> '') AS has_restic_password,
                   repo_subpath, schedule_enabled, schedule_cadence, schedule_hour,
                   retention_keep, updated_at
              FROM backup_target WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(BackupTarget {
            kind: row.try_get("kind")?,
            label: row.try_get("label")?,
            host: row.try_get("host")?,
            share: row.try_get("share")?,
            username: row.try_get("username")?,
            domain: row.try_get("domain")?,
            has_password: row.try_get("has_password")?,
            has_restic_password: row.try_get("has_restic_password")?,
            repo_subpath: row.try_get("repo_subpath")?,
            schedule_enabled: row.try_get("schedule_enabled")?,
            schedule_cadence: row.try_get("schedule_cadence")?,
            schedule_hour: row.try_get("schedule_hour")?,
            retention_keep: row.try_get("retention_keep")?,
            updated_at: row.try_get("updated_at")?,
        }))
    }

    /// Vue interne (secrets inclus) — pour le service.
    pub async fn get_full(&self) -> anyhow::Result<Option<FullTarget>> {
        let row: Option<PgRow> = query(
            r#"
            SELECT kind, host, share, username, domain, password,
                   restic_password, repo_subpath, schedule_enabled, schedule_cadence,
                   schedule_hour, retention_keep
              FROM backup_target WHERE id = 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(FullTarget {
            kind: row.try_get("kind")?,
            host: row.try_get("host")?,
            share: row.try_get("share")?,
            username: row.try_get("username")?,
            domain: row.try_get("domain")?,
            password: row.try_get("password").ok(),
            restic_password: row.try_get("restic_password").ok(),
            repo_subpath: row.try_get("repo_subpath")?,
            schedule_enabled: row.try_get("schedule_enabled")?,
            schedule_cadence: row.try_get("schedule_cadence")?,
            schedule_hour: row.try_get("schedule_hour")?,
            retention_keep: row.try_get("retention_keep")?,
        }))
    }

    /// Met à jour la ligne singleton. `password` NULL ⇒ conservé (COALESCE).
    pub async fn upsert(&self, t: &NewTarget) -> anyhow::Result<()> {
        query(
            r#"
            UPDATE backup_target
               SET kind = $1, label = $2, host = $3, share = $4,
                   username = $5, domain = $6,
                   password = COALESCE($7, password),
                   repo_subpath = $8, schedule_enabled = $9, schedule_cadence = $10,
                   schedule_hour = $11, retention_keep = $12, updated_at = now()
             WHERE id = 1
            "#,
        )
        .bind(&t.kind)
        .bind(&t.label)
        .bind(t.host.trim())
        .bind(t.share.trim())
        .bind(t.username.trim())
        .bind(&t.domain)
        .bind(t.password.as_deref().filter(|s| !s.is_empty()))
        .bind(&t.repo_subpath)
        .bind(t.schedule_enabled)
        .bind(&t.schedule_cadence)
        .bind(t.schedule_hour)
        .bind(t.retention_keep)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persiste le mot de passe du dépôt restic (généré une fois à l'init).
    pub async fn set_restic_password(&self, pwd: &str) -> anyhow::Result<()> {
        query("UPDATE backup_target SET restic_password = $1, updated_at = now() WHERE id = 1")
            .bind(pwd)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Révèle le mot de passe du dépôt restic (conservation hors-ligne).
    pub async fn reveal_restic_password(&self) -> anyhow::Result<Option<String>> {
        let row: Option<PgRow> =
            query("SELECT restic_password FROM backup_target WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| r.try_get::<Option<String>, _>("restic_password").ok().flatten()))
    }
}
