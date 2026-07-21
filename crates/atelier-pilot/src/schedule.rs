use chrono::{DateTime, Local, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::sqlx::{PgPool, Row, query};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PilotSchedule {
    pub enabled: bool,
    pub start_hour: i32,
    pub end_hour: i32,
    pub max_concurrent: i32,
    pub include_atelier: bool,
    pub resolve_findings: bool,
    pub engine_policy: String,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SchedulePatch {
    pub enabled: Option<bool>,
    pub start_hour: Option<i32>,
    pub end_hour: Option<i32>,
    pub max_concurrent: Option<i32>,
    pub include_atelier: Option<bool>,
    pub resolve_findings: Option<bool>,
    pub engine_policy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NightSnapshot {
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stats: Value,
    pub atelier_unit: Option<String>,
}

impl NightSnapshot {
    pub fn idle() -> Self {
        Self {
            status: "idle".into(),
            started_at: None,
            finished_at: None,
            stats: json!({}),
            atelier_unit: None,
        }
    }
}

#[derive(Clone)]
pub struct ScheduleStore {
    pool: PgPool,
}

impl ScheduleStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn get(&self) -> anyhow::Result<PilotSchedule> {
        let row = query("SELECT enabled,start_hour,end_hour,max_concurrent,include_atelier,resolve_findings,engine_policy,last_run_at FROM pilot_schedule WHERE id=TRUE")
            .fetch_one(&self.pool).await?;
        let mut v = PilotSchedule {
            enabled: row.try_get("enabled")?,
            start_hour: row.try_get("start_hour")?,
            end_hour: row.try_get("end_hour")?,
            max_concurrent: row.try_get("max_concurrent")?,
            include_atelier: row.try_get("include_atelier")?,
            resolve_findings: row.try_get("resolve_findings")?,
            engine_policy: row.try_get("engine_policy")?,
            last_run_at: row.try_get("last_run_at").ok(),
            next_run_at: None,
        };
        v.next_run_at = next_run_at(&v);
        Ok(v)
    }

    pub async fn update(&self, p: SchedulePatch) -> anyhow::Result<PilotSchedule> {
        if let Some(v) = p.start_hour {
            anyhow::ensure!((0..=23).contains(&v), "start_hour invalide");
        }
        if let Some(v) = p.end_hour {
            anyhow::ensure!((0..=23).contains(&v), "end_hour invalide");
        }
        if let Some(v) = p.max_concurrent {
            anyhow::ensure!((1..=4).contains(&v), "max_concurrent invalide");
        }
        if let Some(v) = p.engine_policy.as_deref() {
            anyhow::ensure!(matches!(v, "claude" | "auto"), "engine_policy invalide");
        }
        query(
            r#"UPDATE pilot_schedule SET enabled=COALESCE($1,enabled),start_hour=COALESCE($2,start_hour),
               end_hour=COALESCE($3,end_hour),max_concurrent=COALESCE($4,max_concurrent),
               include_atelier=COALESCE($5,include_atelier),resolve_findings=COALESCE($6,resolve_findings),
               engine_policy=COALESCE($7,engine_policy),updated_at=now() WHERE id=TRUE"#,
        ).bind(p.enabled).bind(p.start_hour).bind(p.end_hour).bind(p.max_concurrent)
         .bind(p.include_atelier).bind(p.resolve_findings).bind(p.engine_policy.as_deref())
         .execute(&self.pool).await?;
        self.get().await
    }

    pub async fn mark_ran(&self) -> anyhow::Result<()> {
        query("UPDATE pilot_schedule SET last_run_at=now(),updated_at=now() WHERE id=TRUE")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn night(&self) -> anyhow::Result<NightSnapshot> {
        let row = query("SELECT status,started_at,finished_at,stats,atelier_unit FROM pilot_night WHERE id=TRUE")
            .fetch_one(&self.pool).await?;
        Ok(NightSnapshot {
            status: row.try_get("status")?,
            started_at: row.try_get("started_at").ok(),
            finished_at: row.try_get("finished_at").ok(),
            stats: row.try_get("stats").unwrap_or_else(|_| json!({})),
            atelier_unit: row.try_get("atelier_unit").ok(),
        })
    }

    pub async fn set_night(&self, status: &str, stats: &Value) -> anyhow::Result<NightSnapshot> {
        // `status` en partie droite = valeur AVANT update : une nuit déjà en
        // vol (running/waiting_atelier) conserve son started_at (les snapshots
        // de progression repassent par ici) et MERGE ses stats (les clés
        // initiales, ex. trigger, survivent) ; un départ frais réinitialise.
        query(
            r#"UPDATE pilot_night SET
               stats=CASE WHEN status IN ('running','waiting_atelier') THEN stats || $2::jsonb ELSE $2::jsonb END,
               started_at=CASE
                   WHEN $1='running' AND status NOT IN ('running','waiting_atelier') THEN now()
                   WHEN $1 IN ('running','waiting_atelier') THEN COALESCE(started_at,now())
                   ELSE started_at END,
               finished_at=CASE WHEN $1 IN ('done','failed','cancelled') THEN now() ELSE NULL END,
               status=$1
               WHERE id=TRUE"#,
        ).bind(status).bind(stats).execute(&self.pool).await?;
        self.night().await
    }

    pub async fn set_secret(&self, secret: Option<&str>, unit: Option<&str>) -> anyhow::Result<()> {
        query("UPDATE pilot_night SET secret=$1,atelier_unit=$2 WHERE id=TRUE")
            .bind(secret)
            .bind(unit)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_waiting_atelier(&self, patch: &Value) -> anyhow::Result<NightSnapshot> {
        query(
            r#"UPDATE pilot_night SET status='waiting_atelier',stats=stats || $1::jsonb,
               started_at=COALESCE(started_at,now()),finished_at=NULL WHERE id=TRUE"#,
        )
        .bind(patch)
        .execute(&self.pool)
        .await?;
        self.night().await
    }

    pub async fn secret_matches(&self, secret: &str) -> anyhow::Result<bool> {
        let row = query("SELECT secret=$1 AS ok FROM pilot_night WHERE id=TRUE")
            .bind(secret)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get("ok").unwrap_or(false))
    }
}

pub fn in_window(start: i32, end: i32, hour: i32) -> bool {
    if start == end {
        true
    } else if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

pub fn due(s: &PilotSchedule) -> bool {
    if !s.enabled || !in_window(s.start_hour, s.end_hour, Local::now().hour() as i32) {
        return false;
    }
    s.last_run_at
        .map(|t| Utc::now().signed_duration_since(t) >= chrono::Duration::hours(20))
        .unwrap_or(true)
}

fn next_run_at(s: &PilotSchedule) -> Option<DateTime<Utc>> {
    if !s.enabled {
        return None;
    }
    let now = Local::now();
    // m10 : `due` exige aussi 20 h depuis la dernière nuit — le prochain
    // départ affiché est donc le premier start_hour qui satisfait le min-age,
    // pas mécaniquement celui de demain.
    let earliest = s
        .last_run_at
        .map(|t| (t + chrono::Duration::hours(20)).with_timezone(&Local));
    let mut date = now.date_naive();
    for _ in 0..8 {
        if let Some(candidate) = date
            .and_hms_opt(s.start_hour as u32, 0, 0)
            .and_then(|n| n.and_local_timezone(Local).single())
            && candidate > now
            && earliest.map(|e| candidate >= e).unwrap_or(true)
        {
            return Some(candidate.with_timezone(&Utc));
        }
        date += chrono::Duration::days(1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{PilotSchedule, in_window, next_run_at};
    use chrono::{Local, Timelike, Utc};

    #[test]
    fn next_run_integrates_min_age() {
        let base = PilotSchedule {
            enabled: true,
            start_hour: 1,
            end_hour: 7,
            max_concurrent: 2,
            include_atelier: true,
            resolve_findings: true,
            engine_policy: "claude".into(),
            last_run_at: None,
            next_run_at: None,
        };
        let now = Utc::now();
        let fresh = next_run_at(&base).expect("next run");
        assert!(fresh > now);
        assert_eq!(fresh.with_timezone(&Local).hour(), 1);

        let recent = PilotSchedule {
            last_run_at: Some(now),
            ..base.clone()
        };
        let next = next_run_at(&recent).expect("next run");
        assert!(next >= now + chrono::Duration::hours(20));
        assert_eq!(next.with_timezone(&Local).hour(), 1);

        let disabled = PilotSchedule {
            enabled: false,
            ..base
        };
        assert!(next_run_at(&disabled).is_none());
    }

    #[test]
    fn window_handles_day_and_midnight_ranges() {
        assert!(in_window(1, 5, 1));
        assert!(in_window(1, 5, 4));
        assert!(!in_window(1, 5, 5));
        assert!(in_window(22, 5, 23));
        assert!(in_window(22, 5, 2));
        assert!(!in_window(22, 5, 12));
        assert!(in_window(0, 0, 12));
    }
}
