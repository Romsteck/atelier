//! Planificateur interne (boucle Tokio). **Inactif par défaut** : ne déclenche
//! rien tant que `schedule_enabled = false` sur la cible. Activable via l'UI
//! (PUT /api/backup/target). Tick toutes les 5 min ; déclenche un run `cron`
//! quand l'heure locale correspond et que le dernier succès est assez ancien.

use std::time::Duration;

use chrono::{Local, Timelike, Utc};
use tracing::{info, warn};

use crate::service::BackupService;

pub async fn run_loop(svc: BackupService) {
    let mut tick = tokio::time::interval(Duration::from_secs(300));
    loop {
        tick.tick().await;
        if let Err(e) = maybe_run(&svc).await {
            warn!(error = %e, "atelier-backup scheduler tick failed");
        }
    }
}

async fn maybe_run(svc: &BackupService) -> Result<(), String> {
    let Some(t) = svc.target().await? else { return Ok(()) };
    if !t.schedule_enabled || svc.is_running() {
        return Ok(());
    }
    let now = Local::now();
    if now.hour() as i16 != t.schedule_hour {
        return Ok(());
    }
    // Fenêtre minimale depuis le dernier succès (anti-double-déclenchement dans
    // l'heure + cadence). daily ≈ 20 h, weekly ≈ 6 j 12 h.
    let min_age = match t.schedule_cadence.as_str() {
        "weekly" => chrono::Duration::hours(24 * 6 + 12),
        _ => chrono::Duration::hours(20),
    };
    let due = match svc.last_success_at().await? {
        Some(ts) => Utc::now().signed_duration_since(ts) >= min_age,
        None => true,
    };
    if !due {
        return Ok(());
    }
    info!(cadence = %t.schedule_cadence, hour = t.schedule_hour, "atelier-backup: déclenchement planifié");
    if let Err(e) = svc.run_now("cron").await {
        warn!(error = %e, "atelier-backup: run planifié échoué");
    }
    Ok(())
}
