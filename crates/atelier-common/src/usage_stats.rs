//! Store des statistiques d'utilisation (tables `app_traffic_daily`,
//! `agent_turn_usage`, `app_build_runs` dans `atelier_meta`) + agrégations
//! lues par la page `/stats` du panneau de contrôle.
//!
//! Deux rôles distincts :
//!   1. **Écritures d'instrumentation** — `flush_traffic` (compteur proxy flushé
//!      périodiquement), `insert_turn` (tokens/coût d'un tour agent Studio),
//!      `build_started`/`build_finished` (historique builds/ships, piloté par un
//!      subscriber du canal `app_build`). Toutes légères et débrayables.
//!   2. **Lectures d'agrégation** — `overview_meta` / `apps_metrics` : SELECT
//!      GROUP BY sur les tables `atelier_meta` (le pool control-plane requête
//!      TOUTES ses tables, y compris surveillance/findings/backup/docs qui
//!      vivent dans la même base). Les sources hors `atelier_meta` (logs,
//!      dataverse, git, perfs systemd) sont assemblées côté handler.
//!
//! Dégrade en no-op / vide quand le pool est absent (Postgres down au boot) —
//! mirror de [`crate::notification_store::NotificationStore`]. Aucun canal WS :
//! la page `/stats` est consultative (fetch on-demand), pas temps réel.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tracing::{info, warn};

use crate::control_db::sqlx::{PgPool, Pool, Postgres, query, query_as};

/// Delta de trafic d'une (app, jour) accumulé en mémoire par le path-proxy,
/// puis flushé en UPSERT incrémental. Produit par `ProxyStats` (atelier-api).
#[derive(Debug, Clone, Default)]
pub struct TrafficDelta {
    pub hits: i64,
    pub errors_5xx: i64,
    pub ws_upgrades: i64,
    pub latency_ms_sum: i64,
    pub latency_n: i64,
}

/// Usage d'un tour d'agent Studio (event `result` du runner), persisté hors du
/// chemin critique de relay NDJSON.
#[derive(Debug, Clone, Default)]
pub struct TurnUsage {
    pub slug: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_creation: Option<i64>,
    pub cost_usd: Option<f64>,
    pub num_turns: Option<i64>,
    pub duration_ms: Option<i64>,
    pub is_error: bool,
}

#[derive(Clone)]
pub struct UsageStatsStore {
    pool: Option<Pool<Postgres>>,
}

impl UsageStatsStore {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self { pool }
    }

    fn pool(&self) -> Option<&Pool<Postgres>> {
        self.pool.as_ref()
    }

    // ── Écritures d'instrumentation ────────────────────────────────────────

    /// Flush incrémental du compteur de trafic mémoire : une ligne par (slug,
    /// jour). UPSERT additif — `hits = hits + EXCLUDED.hits`, etc. **Atomique**
    /// (une transaction pour tout le lot) : l'UPSERT étant non-idempotent, un
    /// échec en milieu de lot ne doit PAS committer une partie des lignes —
    /// sinon la ré-injection par `merge_back` (lot complet) double-compterait
    /// les lignes déjà persistées. Tout-ou-rien → merge_back reste exact.
    /// Renvoie une erreur pour que l'appelant ré-injecte. Best-effort si pool
    /// absent.
    pub async fn flush_traffic(&self, rows: &[(String, chrono::NaiveDate, TrafficDelta)]) -> anyhow::Result<()> {
        let Some(pool) = self.pool() else { return Ok(()) };
        let mut tx = pool.begin().await?;
        for (slug, day, d) in rows {
            query(
                "INSERT INTO app_traffic_daily \
                    (slug, day, hits, errors_5xx, ws_upgrades, latency_ms_sum, latency_n) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (slug, day) DO UPDATE SET \
                    hits           = app_traffic_daily.hits + EXCLUDED.hits, \
                    errors_5xx     = app_traffic_daily.errors_5xx + EXCLUDED.errors_5xx, \
                    ws_upgrades    = app_traffic_daily.ws_upgrades + EXCLUDED.ws_upgrades, \
                    latency_ms_sum = app_traffic_daily.latency_ms_sum + EXCLUDED.latency_ms_sum, \
                    latency_n      = app_traffic_daily.latency_n + EXCLUDED.latency_n",
            )
            .bind(slug)
            .bind(day)
            .bind(d.hits)
            .bind(d.errors_5xx)
            .bind(d.ws_upgrades)
            .bind(d.latency_ms_sum)
            .bind(d.latency_n)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Persiste l'usage d'un tour d'agent. Best-effort : loggé, jamais propagé
    /// (appelé depuis un `tokio::spawn` hors du chemin de relay).
    pub async fn insert_turn(&self, u: TurnUsage) {
        let Some(pool) = self.pool() else { return };
        let res = query(
            "INSERT INTO agent_turn_usage \
                (slug, session_id, model, tokens_in, tokens_out, cache_read, \
                 cache_creation, cost_usd, num_turns, duration_ms, is_error) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&u.slug)
        .bind(&u.session_id)
        .bind(&u.model)
        .bind(u.tokens_in)
        .bind(u.tokens_out)
        .bind(u.cache_read)
        .bind(u.cache_creation)
        .bind(u.cost_usd)
        .bind(u.num_turns)
        .bind(u.duration_ms)
        .bind(u.is_error)
        .execute(pool)
        .await;
        if let Err(e) = res {
            warn!(slug = %u.slug, error = %e, "agent_turn_usage insert failed");
        }
    }

    /// Ouvre une ligne de build/ship (`status=running`). Renvoie l'id (uuid en
    /// texte) à conserver côté subscriber pour la clôture. `None` si pool absent.
    pub async fn build_started(&self, slug: &str, kind: &str) -> Option<String> {
        let pool = self.pool()?;
        let kind = if kind == "ship" { "ship" } else { "build" };
        let id = uuid::Uuid::new_v4();
        let res = query("INSERT INTO app_build_runs (id, slug, kind) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(slug)
            .bind(kind)
            .execute(pool)
            .await;
        match res {
            Ok(_) => Some(id.to_string()),
            Err(e) => {
                warn!(slug, error = %e, "app_build_runs insert failed");
                None
            }
        }
    }

    /// Clôture une ligne de build/ship. `duration_ms` calculé côté SQL depuis
    /// `started_at` (robuste aux horloges). `status` = success | error |
    /// interrupted (ce dernier = run orphelin remplacé sans event terminal).
    pub async fn build_finished(&self, id: &str, status: &str, error: Option<&str>) {
        let Some(pool) = self.pool() else { return };
        let Ok(uid) = uuid::Uuid::parse_str(id) else {
            warn!(id, "app_build_runs finish: id invalide");
            return;
        };
        let status = match status {
            "error" => "error",
            "interrupted" => "interrupted",
            _ => "success",
        };
        let res = query(
            "UPDATE app_build_runs SET \
                status = $2, \
                error = $3, \
                finished_at = now(), \
                duration_ms = (EXTRACT(EPOCH FROM (now() - started_at)) * 1000)::bigint \
             WHERE id = $1",
        )
        .bind(uid)
        .bind(status)
        .bind(error)
        .execute(pool)
        .await;
        if let Err(e) = res {
            warn!(id, error = %e, "app_build_runs finish failed");
        }
    }

    /// Réconciliation boot : les builds restés `running` (Atelier redémarré au
    /// milieu d'un build) sont marqués `interrupted`. Idempotent.
    pub async fn reconcile_interrupted_builds(&self) {
        let Some(pool) = self.pool() else { return };
        match query(
            "UPDATE app_build_runs SET status = 'interrupted', finished_at = now() \
             WHERE status = 'running'",
        )
        .execute(pool)
        .await
        {
            Ok(res) if res.rows_affected() > 0 => {
                info!(count = res.rows_affected(), "app_build_runs: builds interrompus réconciliés");
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "app_build_runs reconcile failed"),
        }
    }

    /// Purge boot des 3 tables (rétention distincte par table). Best-effort.
    pub async fn prune_old(&self) {
        let Some(pool) = self.pool() else { return };
        let jobs: [(&str, &str); 3] = [
            ("app_traffic_daily", "DELETE FROM app_traffic_daily WHERE day < current_date - 400"),
            ("agent_turn_usage", "DELETE FROM agent_turn_usage WHERE ts < now() - interval '365 days'"),
            ("app_build_runs", "DELETE FROM app_build_runs WHERE started_at < now() - interval '90 days'"),
        ];
        for (name, sql) in jobs {
            match query(sql).execute(pool).await {
                Ok(res) if res.rows_affected() > 0 => {
                    info!(table = name, pruned = res.rows_affected(), "usage_stats: purge rétention");
                }
                Ok(_) => {}
                Err(e) => warn!(table = name, error = %e, "usage_stats prune failed"),
            }
        }
    }

    // ── Agrégations (page /stats) ──────────────────────────────────────────

    /// Snapshot global pour l'overview : uniquement les sources `atelier_meta`
    /// (apps, trafic, tokens agent, surveillance, findings, builds, backup,
    /// journal, docs, conversations, issues). Les logs/dataverse/git/perfs sont
    /// assemblés côté handler. Renvoie un objet vide (zéros) si pool absent.
    pub async fn overview_meta(&self) -> anyhow::Result<Value> {
        let Some(pool) = self.pool() else { return Ok(empty_overview()) };

        // Apps — total, répartition par état et par stack.
        let by_state: Vec<(String, i64)> =
            query_as("SELECT state, count(*)::bigint FROM applications GROUP BY state ORDER BY 2 DESC")
                .fetch_all(pool)
                .await?;
        let by_stack: Vec<(Option<String>, i64)> = query_as(
            "SELECT NULLIF(data->>'stack','') AS stack, count(*)::bigint \
               FROM applications GROUP BY 1 ORDER BY 2 DESC",
        )
        .fetch_all(pool)
        .await?;
        let apps_total: i64 = by_state.iter().map(|(_, n)| n).sum();

        // Trafic — aujourd'hui + série 30 j.
        let (t_hits, t_err): (i64, i64) = query_as(
            "SELECT COALESCE(sum(hits),0)::bigint, COALESCE(sum(errors_5xx),0)::bigint \
               FROM app_traffic_daily WHERE day = current_date",
        )
        .fetch_one(pool)
        .await?;
        let traffic_series: Vec<(String, i64, i64)> = query_as(
            "SELECT day::text, COALESCE(sum(hits),0)::bigint, COALESCE(sum(errors_5xx),0)::bigint \
               FROM app_traffic_daily WHERE day >= current_date - 29 \
              GROUP BY day ORDER BY day",
        )
        .fetch_all(pool)
        .await?;

        // Agent Studio — totaux 30 j + série + répartition par modèle.
        let (a_in, a_out, a_cost, a_turns): (i64, i64, f64, i64) = query_as(
            "SELECT COALESCE(sum(tokens_in),0)::bigint, COALESCE(sum(tokens_out),0)::bigint, \
                    COALESCE(sum(cost_usd),0)::float8, count(*)::bigint \
               FROM agent_turn_usage WHERE ts >= now() - interval '30 days'",
        )
        .fetch_one(pool)
        .await?;
        let agent_series: Vec<(String, f64, i64)> = query_as(
            "SELECT date(ts)::text, COALESCE(sum(cost_usd),0)::float8, \
                    COALESCE(sum(COALESCE(tokens_in,0)+COALESCE(tokens_out,0)),0)::bigint \
               FROM agent_turn_usage WHERE ts >= now() - interval '30 days' \
              GROUP BY 1 ORDER BY 1",
        )
        .fetch_all(pool)
        .await?;
        let agent_by_model: Vec<(Option<String>, i64, i64)> = query_as(
            "SELECT model, count(*)::bigint, \
                    COALESCE(sum(COALESCE(tokens_in,0)+COALESCE(tokens_out,0)),0)::bigint \
               FROM agent_turn_usage WHERE ts >= now() - interval '30 days' \
              GROUP BY model ORDER BY 2 DESC",
        )
        .fetch_all(pool)
        .await?;

        // Surveillance — runs 30 j par statut, tokens, findings ouverts.
        let sv_runs: Vec<(String, i64)> = query_as(
            "SELECT status, count(*)::bigint FROM surveillance_runs \
              WHERE started_at >= now() - interval '30 days' GROUP BY status",
        )
        .fetch_all(pool)
        .await?;
        let (sv_tokens,): (i64,) = query_as(
            "SELECT COALESCE(sum(COALESCE(tokens_in,0)+COALESCE(tokens_out,0)),0)::bigint \
               FROM surveillance_runs WHERE started_at >= now() - interval '30 days'",
        )
        .fetch_one(pool)
        .await?;
        let findings_open: Vec<(String, i64)> = query_as(
            "SELECT severity, count(*)::bigint FROM findings WHERE status = 'open' GROUP BY severity",
        )
        .fetch_all(pool)
        .await?;
        let findings_open_total: i64 = findings_open.iter().map(|(_, n)| n).sum();

        // Builds/ships 7 j.
        let builds: Vec<(String, String, i64)> = query_as(
            "SELECT kind, status, count(*)::bigint FROM app_build_runs \
              WHERE started_at >= now() - interval '7 days' GROUP BY kind, status",
        )
        .fetch_all(pool)
        .await?;

        // Backup — dernier run + octets ajoutés 30 j.
        let last_backup: Option<(String, Option<DateTime<Utc>>, Option<i64>)> = query_as(
            "SELECT status, finished_at, total_added FROM backup_runs ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(pool)
        .await?;
        let (backup_added,): (i64,) = query_as(
            "SELECT COALESCE(sum(total_added),0)::bigint FROM backup_runs \
              WHERE status = 'success' AND started_at >= now() - interval '30 days'",
        )
        .fetch_one(pool)
        .await?;

        // Journal d'actions agents 7 j.
        let (actions_7d,): (i64,) = query_as(
            "SELECT count(*)::bigint FROM platform_notifications \
              WHERE kind = 'action' AND created_at >= now() - interval '7 days'",
        )
        .fetch_one(pool)
        .await?;

        // Docs.
        let (docs_total, docs_diagram): (i64, i64) = query_as(
            "SELECT count(*)::bigint, count(*) FILTER (WHERE has_diagram) FROM doc_entries",
        )
        .fetch_one(pool)
        .await?;

        // Conversations agent (méta).
        let (conv_total, conv_apps): (i64, i64) = query_as(
            "SELECT count(*)::bigint, count(DISTINCT slug)::bigint FROM agent_conversation_meta",
        )
        .fetch_one(pool)
        .await?;
        let conv_by_model: Vec<(Option<String>, i64)> = query_as(
            "SELECT model, count(*)::bigint FROM agent_conversation_meta GROUP BY model ORDER BY 2 DESC",
        )
        .fetch_all(pool)
        .await?;

        Ok(json!({
            "apps": {
                "total": apps_total,
                "by_state": kv_pairs(by_state),
                "by_stack": by_stack.into_iter()
                    .map(|(s, n)| json!({"stack": s.unwrap_or_else(|| "—".into()), "count": n}))
                    .collect::<Vec<_>>(),
            },
            "traffic": {
                "today": {"hits": t_hits, "errors_5xx": t_err},
                "series_30d": traffic_series.into_iter()
                    .map(|(day, hits, err)| json!({"day": day, "hits": hits, "errors_5xx": err}))
                    .collect::<Vec<_>>(),
            },
            "agent": {
                "tokens_in_30d": a_in,
                "tokens_out_30d": a_out,
                "cost_30d": a_cost,
                "turns_30d": a_turns,
                "series_30d": agent_series.into_iter()
                    .map(|(day, cost, tokens)| json!({"day": day, "cost": cost, "tokens": tokens}))
                    .collect::<Vec<_>>(),
                "by_model": agent_by_model.into_iter()
                    .map(|(m, turns, tokens)| json!({"model": m.unwrap_or_else(|| "défaut".into()), "turns": turns, "tokens": tokens}))
                    .collect::<Vec<_>>(),
            },
            "surveillance": {
                "runs_30d": kv_pairs(sv_runs),
                "tokens_30d": sv_tokens,
                "findings_open": kv_pairs(findings_open),
                "findings_open_total": findings_open_total,
            },
            "builds_7d": builds.into_iter()
                .map(|(kind, status, n)| json!({"kind": kind, "status": status, "count": n}))
                .collect::<Vec<_>>(),
            "backup": {
                "last": last_backup.map(|(status, finished, added)| json!({
                    "status": status,
                    "finished_at": finished.map(rfc3339),
                    "total_added": added,
                })),
                "added_30d": backup_added,
            },
            "actions_7d": actions_7d,
            "docs": {"entries": docs_total, "with_diagram": docs_diagram},
            "conversations": {
                "total": conv_total,
                "apps": conv_apps,
                "by_model": conv_by_model.into_iter()
                    .map(|(m, n)| json!({"model": m.unwrap_or_else(|| "défaut".into()), "count": n}))
                    .collect::<Vec<_>>(),
            },
        }))
    }

    /// Métriques agrégées PAR SLUG (pour le tableau par app). Le handler combine
    /// avec `AppRegistry::list()` (état/port/stack) + l'encapsulation FS. Vide si
    /// pool absent.
    pub async fn apps_metrics(&self) -> anyhow::Result<BTreeMap<String, Value>> {
        let mut out: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
        let Some(pool) = self.pool() else { return Ok(BTreeMap::new()) };

        macro_rules! entry {
            ($slug:expr) => {
                out.entry($slug).or_default()
            };
        }

        // Trafic 24 h / 7 j + latence moyenne.
        let traffic: Vec<(String, i64, i64, i64, i64, i64)> = query_as(
            "SELECT slug, \
                COALESCE(sum(hits) FILTER (WHERE day = current_date),0)::bigint, \
                COALESCE(sum(hits),0)::bigint, \
                COALESCE(sum(errors_5xx),0)::bigint, \
                COALESCE(sum(latency_ms_sum),0)::bigint, \
                COALESCE(sum(latency_n),0)::bigint \
              FROM app_traffic_daily WHERE day >= current_date - 6 GROUP BY slug",
        )
        .fetch_all(pool)
        .await?;
        for (slug, h24, h7, e7, lsum, ln) in traffic {
            let avg = if ln > 0 { Some(lsum as f64 / ln as f64) } else { None };
            let e = entry!(slug);
            e.insert("hits_24h".into(), json!(h24));
            e.insert("hits_7d".into(), json!(h7));
            e.insert("errors_7d".into(), json!(e7));
            e.insert("latency_ms_avg".into(), json!(avg));
        }

        // Tokens/coût agent 30 j.
        let tokens: Vec<(String, i64, f64, i64)> = query_as(
            "SELECT slug, COALESCE(sum(COALESCE(tokens_in,0)+COALESCE(tokens_out,0)),0)::bigint, \
                    COALESCE(sum(cost_usd),0)::float8, count(*)::bigint \
               FROM agent_turn_usage WHERE ts >= now() - interval '30 days' GROUP BY slug",
        )
        .fetch_all(pool)
        .await?;
        for (slug, tok, cost, turns) in tokens {
            let e = entry!(slug);
            e.insert("tokens_30d".into(), json!(tok));
            e.insert("cost_30d".into(), json!(cost));
            e.insert("turns_30d".into(), json!(turns));
        }

        // Dernier build/ship.
        let last_build: Vec<(String, String, String, Option<DateTime<Utc>>)> = query_as(
            "SELECT DISTINCT ON (slug) slug, kind, status, finished_at \
               FROM app_build_runs ORDER BY slug, started_at DESC",
        )
        .fetch_all(pool)
        .await?;
        for (slug, kind, status, finished) in last_build {
            let e = entry!(slug);
            e.insert("last_build".into(), json!({
                "kind": kind, "status": status, "finished_at": finished.map(rfc3339),
            }));
        }

        // Findings ouverts.
        let findings: Vec<(String, i64)> = query_as(
            "SELECT slug, count(*)::bigint FROM findings WHERE status = 'open' GROUP BY slug",
        )
        .fetch_all(pool)
        .await?;
        for (slug, n) in findings {
            entry!(slug).insert("findings_open".into(), json!(n));
        }

        // Dernier scan.
        let last_scan: Vec<(String, String, DateTime<Utc>)> = query_as(
            "SELECT DISTINCT ON (slug) slug, status, started_at \
               FROM surveillance_runs ORDER BY slug, started_at DESC",
        )
        .fetch_all(pool)
        .await?;
        for (slug, status, started) in last_scan {
            entry!(slug).insert("last_scan".into(), json!({"status": status, "started_at": rfc3339(started)}));
        }

        // Docs.
        let docs: Vec<(String, i64, i64)> = query_as(
            "SELECT app_id, count(*)::bigint, count(*) FILTER (WHERE has_diagram) \
               FROM doc_entries GROUP BY app_id",
        )
        .fetch_all(pool)
        .await?;
        for (slug, total, diagram) in docs {
            entry!(slug).insert("docs".into(), json!({"entries": total, "with_diagram": diagram}));
        }

        // Conversations (nombre).
        let convs: Vec<(String, i64)> = query_as(
            "SELECT slug, count(*)::bigint FROM agent_conversation_meta GROUP BY slug",
        )
        .fetch_all(pool)
        .await?;
        for (slug, n) in convs {
            entry!(slug).insert("conversations".into(), json!(n));
        }

        Ok(out.into_iter().map(|(k, v)| (k, Value::Object(v))).collect())
    }
}

/// Transforme `Vec<(clé, count)>` en objet `{clé: count}`.
fn kv_pairs(rows: Vec<(String, i64)>) -> Value {
    let map: serde_json::Map<String, Value> = rows.into_iter().map(|(k, n)| (k, json!(n))).collect();
    Value::Object(map)
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Overview minimal quand Postgres est absent — le front reste fonctionnel.
fn empty_overview() -> Value {
    json!({
        "apps": {"total": 0, "by_state": {}, "by_stack": []},
        "traffic": {"today": {"hits": 0, "errors_5xx": 0}, "series_30d": []},
        "agent": {"tokens_in_30d": 0, "tokens_out_30d": 0, "cost_30d": 0.0, "turns_30d": 0, "series_30d": [], "by_model": []},
        "surveillance": {"runs_30d": {}, "tokens_30d": 0, "findings_open": {}, "findings_open_total": 0},
        "builds_7d": [],
        "backup": {"last": null, "added_30d": 0},
        "actions_7d": 0,
        "docs": {"entries": 0, "with_diagram": 0},
        "conversations": {"total": 0, "apps": 0, "by_model": []},
        "issues": [],
    })
}
