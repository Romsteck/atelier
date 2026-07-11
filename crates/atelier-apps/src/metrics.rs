//! Lecture des métriques de ressources par app (CPU/RAM/tâches/réseau) via
//! `systemctl show` sur l'unité `atelier-app-{slug}.service`. Zéro
//! instrumentation intrusive : systemd (cgroup v2) fait autorité. Consommé par
//! l'endpoint `/api/stats/perf` (snapshot live, calcul de %CPU par delta côté
//! handler). Le réseau (IPIngress/IPEgress) exige `IPAccounting=yes` posé sur
//! l'unité au spawn (cf. supervisor::systemd_run_app) — sans lui, ces compteurs
//! restent absents jusqu'au prochain (re)démarrage de l'app.

use std::time::Duration;

use tokio::process::Command;

/// Snapshot brut des compteurs d'une unité. Tous cumulatifs depuis le start de
/// l'unité (CPUUsageNSec, IP*) ou instantanés (MemoryCurrent, TasksCurrent).
/// `None` = propriété non comptabilisée (accounting off) ou unité inactive.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct UnitPerf {
    pub cpu_nsec: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub tasks: Option<u64>,
    pub ip_ingress_bytes: Option<u64>,
    pub ip_egress_bytes: Option<u64>,
}

/// Lecture one-shot des compteurs d'une unité d'app. Un seul exec `systemctl
/// show` (format `Prop=value`, une propriété par ligne). `None` si l'unité
/// n'existe pas / systemctl échoue.
pub async fn sample(slug: &str) -> Option<UnitPerf> {
    let unit = crate::supervisor::unit_name(slug);
    // Timeout borné (comme le `du` du module disque) : `systemctl show` parle à
    // sd-bus ; si le bus est saturé / un daemon-reload est en cours, l'exec peut
    // traîner. L'endpoint /stats/perf ne doit jamais pendre sur un slug lent.
    let fut = Command::new("systemctl")
        .args([
            "show",
            &unit,
            "--no-pager",
            "-p",
            "CPUUsageNSec,MemoryCurrent,MemoryPeak,TasksCurrent,IPIngressBytes,IPEgressBytes",
        ])
        .output();
    let out = match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(Ok(o)) => o,
        _ => return None,
    };
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut p = UnitPerf::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let val = parse_counter(v);
        match k {
            "CPUUsageNSec" => p.cpu_nsec = val,
            "MemoryCurrent" => p.memory_bytes = val,
            "MemoryPeak" => p.memory_peak_bytes = val,
            "TasksCurrent" => p.tasks = val,
            "IPIngressBytes" => p.ip_ingress_bytes = val,
            "IPEgressBytes" => p.ip_egress_bytes = val,
            _ => {}
        }
    }
    Some(p)
}

/// systemd renvoie un entier décimal, ou une sentinelle (`[not set]`,
/// `infinity`, `[no data]`, vide) quand la propriété n'est pas comptabilisée /
/// l'unité est inactive → `None`.
fn parse_counter(v: &str) -> Option<u64> {
    v.trim().parse::<u64>().ok()
}
