//! Page « Statistiques d'utilisation » (`/api/stats/*`).
//!
//! Un endpoint overview rapide (SQL `atelier_meta` + logs 24 h) + des endpoints
//! par domaine, les plus lents étant mis en cache TTL (dataverse, disque, git).
//! Deux structures d'état vivent ici et sont partagées via `ApiState` :
//!   - [`ProxyStats`] : compteur mémoire de trafic HTTP/WS, alimenté par le
//!     path-proxy (zéro écriture SQL par requête) et flushé périodiquement.
//!   - [`StatsCache`] : cache TTL en mémoire pour les endpoints coûteux.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{instrument, warn};

use atelier_common::usage_stats::TrafficDelta;
use atelier_logging::LogQuery;

use crate::routes::internal_err;
use crate::state::ApiState;

// ─── Compteur de trafic proxy (module 1) ───────────────────────────────────

/// Compteur de trafic HTTP/WS par (app, jour) accumulé en mémoire par le
/// path-proxy. Flushé périodiquement en UPSERT incrémental (`main.rs`) —
/// l'écriture SQL est amortie, jamais par requête.
#[derive(Default)]
pub struct ProxyStats {
    inner: Mutex<HashMap<(String, NaiveDate), TrafficDelta>>,
}

impl ProxyStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enregistre une requête HTTP proxifiée : hit + éventuelle erreur 5xx +
    /// latence (time-to-headers upstream, ms).
    pub fn record_http(&self, slug: &str, status: u16, latency_ms: u64) {
        let day = Utc::now().date_naive();
        let mut g = self.inner.lock();
        let e = g.entry((slug.to_string(), day)).or_default();
        e.hits += 1;
        if status >= 500 {
            e.errors_5xx += 1;
        }
        e.latency_ms_sum += latency_ms as i64;
        e.latency_n += 1;
    }

    /// Enregistre un upgrade WebSocket proxifié (compté comme hit + ws_upgrade).
    pub fn record_ws(&self, slug: &str) {
        let day = Utc::now().date_naive();
        let mut g = self.inner.lock();
        let e = g.entry((slug.to_string(), day)).or_default();
        e.hits += 1;
        e.ws_upgrades += 1;
    }

    /// Vide le compteur → lignes à flusher. Le compteur repart de zéro.
    pub fn drain(&self) -> Vec<(String, NaiveDate, TrafficDelta)> {
        let mut g = self.inner.lock();
        std::mem::take(&mut *g)
            .into_iter()
            .map(|((s, d), c)| (s, d, c))
            .collect()
    }

    /// Ré-injecte des compteurs (flush SQL échoué) en les ADDITIONNANT à ceux
    /// éventuellement accumulés depuis le drain — aucune perte, aucun écrasement.
    pub fn merge_back(&self, rows: Vec<(String, NaiveDate, TrafficDelta)>) {
        let mut g = self.inner.lock();
        for (s, d, c) in rows {
            let e = g.entry((s, d)).or_default();
            e.hits += c.hits;
            e.errors_5xx += c.errors_5xx;
            e.ws_upgrades += c.ws_upgrades;
            e.latency_ms_sum += c.latency_ms_sum;
            e.latency_n += c.latency_n;
        }
    }
}

// ─── Cache TTL des endpoints coûteux ────────────────────────────────────────

/// Cache mémoire à TTL pour les endpoints dont le calcul est coûteux (fan-out
/// dataverse, `du` des workspaces, shell-out git). Clé statique par endpoint.
#[derive(Default)]
pub struct StatsCache {
    inner: Mutex<HashMap<&'static str, (Instant, Value)>>,
}

impl StatsCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, key: &'static str, ttl: Duration) -> Option<Value> {
        let g = self.inner.lock();
        g.get(key)
            .and_then(|(t, v)| (t.elapsed() < ttl).then(|| v.clone()))
    }

    fn put(&self, key: &'static str, v: Value) {
        self.inner.lock().insert(key, (Instant::now(), v));
    }
}

// ─── Router + handlers ──────────────────────────────────────────────────────

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/overview", get(overview))
        .route("/apps", get(apps_table))
        .route("/dataverse", get(dataverse))
        .route("/disk", get(disk))
        .route("/git/activity", get(git_activity))
        .route("/perf", get(perf))
}

fn ok(data: Value) -> Response {
    Json(json!({"success": true, "data": data})).into_response()
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Debug, Deserialize)]
struct RefreshQuery {
    #[serde(default)]
    refresh: Option<String>,
}

fn is_truthy(o: &Option<String>) -> bool {
    matches!(o.as_deref(), Some("1") | Some("true") | Some("yes"))
}

#[derive(Debug, Deserialize)]
struct DaysQuery {
    #[serde(default)]
    days: Option<u32>,
}

/// Overview global : agrégats `atelier_meta` (via le store) + logs 24 h (base
/// séparée `atelier_logs`). Un seul aller-retour côté front.
#[instrument(skip(state))]
async fn overview(State(state): State<ApiState>) -> Response {
    let mut meta = match state.usage_stats.overview_meta().await {
        Ok(v) => v,
        Err(e) => return internal_err("stats", e),
    };
    let q = LogQuery {
        since: Some(Utc::now() - chrono::Duration::hours(24)),
        ..Default::default()
    };
    let logs = match state.logs.stats(&q).await {
        Ok(s) => serde_json::to_value(s).unwrap_or(Value::Null),
        Err(e) => {
            warn!(?e, "stats overview: logs stats failed");
            Value::Null
        }
    };
    if let Value::Object(ref mut m) = meta {
        m.insert("logs".into(), logs);
    }
    ok(meta)
}

/// Tableau par app : base registre + métriques SQL (store) + encapsulation FS.
#[instrument(skip(state))]
async fn apps_table(State(state): State<ApiState>) -> Response {
    let apps = state.app_registry.list().await;
    let metrics = match state.usage_stats.apps_metrics().await {
        Ok(m) => m,
        Err(e) => return internal_err("stats", e),
    };

    // Encapsulation projet (stat FS) hors du thread runtime — quelques stats par app.
    let dirs: Vec<(String, std::path::PathBuf)> =
        apps.iter().map(|a| (a.slug.clone(), a.src_dir())).collect();
    let encaps: HashMap<String, Value> = tokio::task::spawn_blocking(move || {
        dirs.into_iter()
            .map(|(slug, dir)| (slug, encapsulation(&dir)))
            .collect()
    })
    .await
    .unwrap_or_default();

    let rows: Vec<Value> = apps
        .into_iter()
        .map(|a| {
            let m = metrics.get(&a.slug).cloned().unwrap_or_else(|| json!({}));
            let enc = encaps.get(&a.slug).cloned().unwrap_or_else(|| json!({}));
            let secrets = a.env.iter().filter(|e| e.secret).count();
            json!({
                "slug": a.slug,
                "name": a.name,
                "state": a.state.as_str(),
                "port": a.port,
                "stack": a.stack,
                "has_db": a.has_db,
                "claude_access": a.claude_access,
                "created_at": rfc3339(a.created_at),
                "env": {"user": a.env.len(), "secrets": secrets},
                "metrics": m,
                "encapsulation": enc,
            })
        })
        .collect();
    ok(json!({ "apps": rows }))
}

/// Encapsulation projet d'une app : fichiers de contexte agent présents dans son
/// workspace `src/` (bloquant, appelé sous `spawn_blocking`).
fn encapsulation(src: &Path) -> Value {
    let claude_md = src.join("CLAUDE.md");
    let claude_md_mtime = std::fs::metadata(&claude_md)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| rfc3339(DateTime::<Utc>::from(t)));
    json!({
        "claude_md": claude_md.exists(),
        "claude_md_mtime": claude_md_mtime,
        "has_mcp": src.join(".mcp.json").exists(),
        "rules": count_dir(&src.join(".claude/rules")),
        "skills": count_dir(&src.join(".claude/skills")),
    })
}

fn count_dir(p: &Path) -> usize {
    std::fs::read_dir(p)
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0)
}

/// Stats dataverse par app (taille base, lignes estimées, audit CRUD). Fan-out
/// sur les bases `app_{slug}` → cache TTL 5 min (`?refresh=1` force).
#[instrument(skip(state, q))]
async fn dataverse(State(state): State<ApiState>, Query(q): Query<RefreshQuery>) -> Response {
    const KEY: &str = "dataverse";
    if !is_truthy(&q.refresh) {
        if let Some(v) = state.stats_cache.get(KEY, Duration::from_secs(300)) {
            return ok(v);
        }
    }
    let Some(dv) = state.dv.clone() else {
        return ok(json!({"disabled": true, "apps": []}));
    };
    let apps = state.app_registry.list().await;
    let mut rows = Vec::new();
    for a in apps.into_iter().filter(|a| a.has_db) {
        match dv.engine_for(&a.slug).await {
            Ok(engine) => {
                // size/rows/tables uniquement : rapides. L'audit CRUD (`_dv_audit`)
                // peut compter des millions de lignes (trader) → trop coûteux à
                // agréger live, et non affiché ici.
                let size = engine.database_size_bytes().await.ok();
                let rows_est = engine.live_row_estimate().await.ok();
                let tables = engine.list_tables().await.map(|t| t.len() as i64).ok();
                rows.push(json!({
                    "slug": a.slug,
                    "size_bytes": size,
                    "rows_estimate": rows_est,
                    "tables": tables,
                }));
            }
            Err(e) => {
                warn!(slug = %a.slug, ?e, "stats dataverse: engine_for failed");
                rows.push(json!({"slug": a.slug, "error": true}));
            }
        }
    }
    let data = json!({ "apps": rows });
    state.stats_cache.put(KEY, data.clone());
    ok(data)
}

/// Tailles disque : bases Postgres + workspaces `src/` (du) + repos git bare.
/// Calcul potentiellement lent (du sur node_modules/target) → cache TTL 10 min.
#[instrument(skip(state, q))]
async fn disk(State(state): State<ApiState>, Query(q): Query<RefreshQuery>) -> Response {
    const KEY: &str = "disk";
    if !is_truthy(&q.refresh) {
        if let Some(v) = state.stats_cache.get(KEY, Duration::from_secs(600)) {
            return ok(v);
        }
    }

    // Bases (une requête sur le pool admin).
    let (databases, db_total): (Vec<Value>, i64) = match &state.dv {
        Some(dv) => match dv.database_sizes().await {
            Ok(sizes) => {
                let total: i64 = sizes.iter().map(|(_, b)| b).sum();
                (
                    sizes
                        .into_iter()
                        .map(|(name, bytes)| json!({"name": name, "bytes": bytes}))
                        .collect(),
                    total,
                )
            }
            Err(e) => {
                warn!(?e, "stats disk: database_sizes failed");
                (vec![], 0)
            }
        },
        None => (vec![], 0),
    };

    // Workspaces src/ — du concurrent, borné par un timeout par app.
    let apps = state.app_registry.list().await;
    let du_futs = apps.iter().map(|a| {
        let dir = a.src_dir();
        let slug = a.slug.clone();
        async move {
            let b = tokio::time::timeout(Duration::from_secs(8), du_bytes(&dir))
                .await
                .ok()
                .flatten();
            (slug, b)
        }
    });
    let du_res = futures_util::future::join_all(du_futs).await;
    let mut workspaces = Vec::new();
    let mut ws_total: u64 = 0;
    for (slug, b) in du_res {
        if let Some(bytes) = b {
            ws_total += bytes;
            workspaces.push(json!({"slug": slug, "bytes": bytes}));
        }
    }

    // Repos git bare (taille déjà calculée par le service git).
    let (git_repos, git_total): (Vec<Value>, u64) = match state.git.list_repos().await {
        Ok(repos) => {
            let total: u64 = repos.iter().map(|r| r.size_bytes).sum();
            (
                repos
                    .into_iter()
                    .map(|r| json!({"slug": r.slug, "bytes": r.size_bytes}))
                    .collect(),
                total,
            )
        }
        Err(e) => {
            warn!(?e, "stats disk: list_repos failed");
            (vec![], 0)
        }
    };

    let data = json!({
        "databases": databases,
        "databases_total": db_total,
        "workspaces": workspaces,
        "workspaces_total": ws_total,
        "git_repos": git_repos,
        "git_total": git_total,
    });
    state.stats_cache.put(KEY, data.clone());
    ok(data)
}

/// `du -sb` d'un répertoire (octets), `None` si le chemin n'existe pas / du échoue.
async fn du_bytes(path: &Path) -> Option<u64> {
    let out = tokio::process::Command::new("du")
        .arg("-sb")
        .arg("--")
        .arg(path)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
}

/// Heatmap d'activité git GLOBALE (tous les repos fusionnés par jour). Cache TTL
/// 10 min pour la fenêtre annuelle par défaut.
#[instrument(skip(state, q))]
async fn git_activity(State(state): State<ApiState>, Query(q): Query<DaysQuery>) -> Response {
    let days = q.days.unwrap_or(365).clamp(1, 1825);
    if days == 365 {
        if let Some(v) = state.stats_cache.get("git_activity", Duration::from_secs(600)) {
            return ok(v);
        }
    }
    let repos = match state.git.list_repos().await {
        Ok(r) => r,
        Err(e) => return internal_err("stats", e),
    };
    let mut merged: BTreeMap<String, u32> = BTreeMap::new();
    for r in &repos {
        if let Ok(buckets) = state.git.get_commit_activity(&r.slug, days).await {
            for b in buckets {
                *merged.entry(b.date).or_insert(0) += b.count;
            }
        }
    }
    let activity: Vec<Value> = merged
        .into_iter()
        .map(|(date, count)| json!({"date": date, "count": count}))
        .collect();
    let data = json!({ "activity": activity, "repos": repos.len() });
    if days == 365 {
        state.stats_cache.put("git_activity", data.clone());
    }
    ok(data)
}

/// Snapshot LIVE des perfs par app (CPU %/RAM/tâches/réseau). Deux échantillons
/// espacés de 400 ms pour dériver le %CPU depuis le compteur cumulatif
/// CPUUsageNSec. Aucune persistance : coût nul quand la page n'est pas ouverte.
#[instrument(skip(state))]
async fn perf(State(state): State<ApiState>) -> Response {
    let slugs: Vec<String> = state
        .app_registry
        .list()
        .await
        .into_iter()
        .map(|a| a.slug)
        .collect();

    let mut s1: HashMap<String, atelier_apps::metrics::UnitPerf> = HashMap::new();
    for s in &slugs {
        if let Some(p) = atelier_apps::metrics::sample(s).await {
            s1.insert(s.clone(), p);
        }
    }
    tokio::time::sleep(Duration::from_millis(400)).await;

    let ncpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1) as f64;
    let elapsed_ns = 400_000_000f64;

    let mut rows = Vec::new();
    for s in &slugs {
        let Some(p2) = atelier_apps::metrics::sample(s).await else {
            continue;
        };
        // Unité inactive : MemoryCurrent absent → app arrêtée, on l'omet.
        if p2.memory_bytes.is_none() {
            continue;
        }
        let cpu_pct = match (s1.get(s).and_then(|p| p.cpu_nsec), p2.cpu_nsec) {
            (Some(a), Some(b)) if b >= a => Some((b - a) as f64 / (elapsed_ns * ncpu) * 100.0),
            _ => None,
        };
        rows.push(json!({
            "slug": s,
            "cpu_pct": cpu_pct,
            "memory_bytes": p2.memory_bytes,
            "memory_peak_bytes": p2.memory_peak_bytes,
            "tasks": p2.tasks,
            "ip_ingress_bytes": p2.ip_ingress_bytes,
            "ip_egress_bytes": p2.ip_egress_bytes,
        }));
    }
    ok(json!({ "apps": rows, "ncpu": ncpu as u64 }))
}
