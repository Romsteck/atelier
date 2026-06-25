//! Scheduler interne du *sweep* automatique de surveillance. **Inactif par
//! défaut** (`enabled=false` dans la config singleton `sweep_schedule`).
//! Activable via `PUT /api/surveillance/sweep/schedule`. Tick toutes les 5 min ;
//! déclenche un sweep `cron` quand l'heure locale correspond et que le dernier
//! sweep est assez ancien. Calqué sur `atelier-backup/src/scheduler.rs`.
//!
//! Le sweep planifié réutilise le chemin Claude existant (`start_sweep` → run
//! rows + `scan.js` en hr-studio, OAuth abonnement) — aucun nouveau moteur IA.

use std::time::Duration;

use chrono::{DateTime, Local, Timelike, Utc};
use serde::Serialize;
use tracing::{info, warn};

use crate::service::SurveillanceService;
use crate::sqlx::{Pool, Postgres, Row, query};

/// Config singleton du sweep planifié (table `sweep_schedule`).
#[derive(Debug, Clone, Serialize)]
pub struct SweepSchedule {
    pub enabled: bool,
    pub hour: i32,
    pub cadence: String,
    pub last_run_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct SweepScheduleStore {
    pool: Pool<Postgres>,
}

impl SweepScheduleStore {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }

    pub async fn get(&self) -> anyhow::Result<SweepSchedule> {
        let row =
            query("SELECT enabled, hour, cadence, last_run_at FROM sweep_schedule WHERE id = TRUE")
                .fetch_one(&self.pool)
                .await?;
        Ok(SweepSchedule {
            enabled: row.try_get("enabled")?,
            hour: row.try_get("hour")?,
            cadence: row.try_get("cadence")?,
            last_run_at: row.try_get("last_run_at").ok(),
        })
    }

    pub async fn set(
        &self,
        enabled: bool,
        hour: i32,
        cadence: &str,
    ) -> anyhow::Result<SweepSchedule> {
        let hour = hour.clamp(0, 23);
        let cadence = if cadence == "weekly" { "weekly" } else { "daily" };
        let row = query(
            r#"
            UPDATE sweep_schedule
               SET enabled = $1, hour = $2, cadence = $3, updated_at = now()
             WHERE id = TRUE
            RETURNING enabled, hour, cadence, last_run_at
            "#,
        )
        .bind(enabled)
        .bind(hour)
        .bind(cadence)
        .fetch_one(&self.pool)
        .await?;
        Ok(SweepSchedule {
            enabled: row.try_get("enabled")?,
            hour: row.try_get("hour")?,
            cadence: row.try_get("cadence")?,
            last_run_at: row.try_get("last_run_at").ok(),
        })
    }

    /// Stamp the last sweep completion (bounds the min-age window).
    pub async fn mark_ran(&self) -> anyhow::Result<()> {
        query("UPDATE sweep_schedule SET last_run_at = now() WHERE id = TRUE")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

pub async fn run_loop(svc: SurveillanceService, store: SweepScheduleStore) {
    let mut tick = tokio::time::interval(Duration::from_secs(300));
    loop {
        tick.tick().await;
        if let Err(e) = maybe_run(&svc, &store).await {
            warn!(error = %e, "atelier-watcher sweep scheduler tick failed");
        }
    }
}

async fn maybe_run(svc: &SurveillanceService, store: &SweepScheduleStore) -> anyhow::Result<()> {
    let cfg = store.get().await?;
    if !cfg.enabled {
        return Ok(());
    }
    let now = Local::now();
    if now.hour() as i32 != cfg.hour {
        return Ok(());
    }
    // Fenêtre minimale depuis le dernier sweep (anti-double-déclenchement dans
    // l'heure + cadence). daily ≈ 20 h, weekly ≈ 6 j 12 h.
    let min_age = match cfg.cadence.as_str() {
        "weekly" => chrono::Duration::hours(24 * 6 + 12),
        _ => chrono::Duration::hours(20),
    };
    let due = match cfg.last_run_at {
        Some(ts) => Utc::now().signed_duration_since(ts) >= min_age,
        None => true,
    };
    if !due {
        return Ok(());
    }
    match svc.start_sweep("cron") {
        Ok(_) => {
            info!(hour = cfg.hour, cadence = %cfg.cadence, "surveillance: sweep planifié déclenché")
        }
        // start_sweep refuse si un sweep tourne déjà (single-flight) — bénin.
        Err(e) => warn!(error = %e, "surveillance: sweep planifié non démarré"),
    }
    Ok(())
}
