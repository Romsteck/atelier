use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use atelier_api::state::ApiState;
use atelier_logging::{LogIngestConfig, LogIngestService, LoggingLayer};
use atelier_backup::{BackupService, BackupServiceConfig, SourcePaths};
use atelier_watcher::{SurveillanceConfig, SurveillanceService};
use tokio::net::{TcpListener, UnixListener};
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod source_watcher;

const DEFAULT_HTTP_ADDR: &str = "0.0.0.0:4100";
const DEFAULT_IPC_SOCK: &str = "/run/atelier.sock";
const DEFAULT_DOCS_DIR: &str = "/var/lib/atelier/docs";
const DEFAULT_GIT_REPOS_DIR: &str = "/var/lib/atelier/git/repos";
const DEFAULT_APPS_STATE_DIR: &str = "/var/lib/atelier/state";
const DEFAULT_APPS_SRC_ROOT: &str = "/var/lib/atelier/apps";
const DEFAULT_APPS_RUNTIME_ROOT: &str = "/var/lib/atelier/apps";
const DEFAULT_WEB_DIST: &str = "/opt/atelier/web/dist";
/// Données canoniques d'Atelier post-cutover (Atelier owns these files).
const DEFAULT_APPS_DATA_DIR: &str = "/opt/atelier/data";
const DEFAULT_BASE_DOMAIN: &str = "mynetwk.biz";
/// Default MCP endpoint pour les agents inside-app. Direct IP:port pour
/// rester équivalent au legacy `http://10.0.0.254:4001/mcp` — hr-edge
/// exige une auth cookie sur `atelier.mynetwk.biz` qui collisionne avec le
/// Bearer-token MCP. Le Bearer (header `Authorization`) reste l'unique
/// gardien de la surface MCP.
const DEFAULT_MCP_ENDPOINT: &str = "http://10.0.0.254:4100/mcp";
/// Hôte des Postgres apps (Medion). Le secret synchronisé contient `127.0.0.1`
/// (point de vue de Medion) — Atelier le swap vers cet hôte au registre des DSN.
const DEFAULT_DV_HOST: &str = "10.0.0.254";

#[tokio::main]
async fn main() -> Result<()> {
    // Bootstrap the centralized logging service first, then install tracing
    // layers that pipe events into it. If Postgres is unreachable at boot,
    // LogIngestService runs in noop mode and Atelier still starts.
    let logs = init_logs_ingest().await;
    init_tracing(logs.clone());

    let http_addr = std::env::var("ATELIER_HTTP_ADDR").unwrap_or_else(|_| DEFAULT_HTTP_ADDR.to_string());
    let ipc_sock = PathBuf::from(std::env::var("ATELIER_IPC_SOCK").unwrap_or_else(|_| DEFAULT_IPC_SOCK.to_string()));
    let docs_dir = PathBuf::from(std::env::var("ATELIER_DOCS_DIR").unwrap_or_else(|_| DEFAULT_DOCS_DIR.to_string()));
    let git_repos_dir = PathBuf::from(std::env::var("ATELIER_GIT_REPOS_DIR").unwrap_or_else(|_| DEFAULT_GIT_REPOS_DIR.to_string()));
    let apps_state_dir = PathBuf::from(std::env::var("ATELIER_APPS_STATE_DIR").unwrap_or_else(|_| DEFAULT_APPS_STATE_DIR.to_string()));
    let apps_src_root = PathBuf::from(std::env::var("ATELIER_APPS_SRC_ROOT").unwrap_or_else(|_| DEFAULT_APPS_SRC_ROOT.to_string()));
    let apps_runtime_root = PathBuf::from(std::env::var("ATELIER_APPS_RUNTIME_ROOT").unwrap_or_else(|_| DEFAULT_APPS_RUNTIME_ROOT.to_string()));
    let apps_data_dir = PathBuf::from(std::env::var("ATELIER_APPS_DATA_DIR").unwrap_or_else(|_| DEFAULT_APPS_DATA_DIR.to_string()));
    let base_domain = std::env::var("ATELIER_BASE_DOMAIN").unwrap_or_else(|_| DEFAULT_BASE_DOMAIN.to_string());
    let mcp_endpoint = std::env::var("ATELIER_MCP_ENDPOINT").unwrap_or_else(|_| DEFAULT_MCP_ENDPOINT.to_string());
    let web_dist = PathBuf::from(std::env::var("ATELIER_WEB_DIST").unwrap_or_else(|_| DEFAULT_WEB_DIST.to_string()));

    info!(
        http_addr = %http_addr,
        ipc_sock = %ipc_sock.display(),
        docs_dir = %docs_dir.display(),
        git_repos_dir = %git_repos_dir.display(),
        apps_state_dir = %apps_state_dir.display(),
        apps_src_root = %apps_src_root.display(),
        apps_runtime_root = %apps_runtime_root.display(),
        web_dist = %web_dist.display(),
        "atelier starting"
    );

    // Chemins capturés par la sauvegarde (résolus avant que git_repos_dir ne soit
    // déplacé dans le GitService). git_dir = parent de .../git/repos.
    let backup_sources = SourcePaths {
        git_dir: git_repos_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| git_repos_dir.clone()),
        env_file: PathBuf::from(
            std::env::var("ATELIER_BACKUP_ENV_FILE").unwrap_or_else(|_| "/opt/atelier/.env".to_string()),
        ),
        data_dir: apps_data_dir.clone(),
        dv_secrets: apps_state_dir.join("dataverse-secrets.json"),
        apps_runtime_root: apps_runtime_root.clone(),
        docs_dir: docs_dir.clone(),
    };

    let git = Arc::new(atelier_git::GitService::with_repos_dir(git_repos_dir));
    let dv = init_dv(&apps_state_dir).await;
    // Shared control-plane Postgres pool (atelier_meta). Backs the task store +
    // docs index now; the registry/port stores migrate onto it in a later stage.
    let meta_pool = init_control_db().await;
    let task_store =
        Arc::new(atelier_common::task_store::TaskStore::new(meta_pool.clone()).await);
    // Studio open-tabs store (cross-PC tab sync). Degrades to no-op without the pool.
    let open_tabs = atelier_common::agent_ui_state::OpenTabsStore::new(meta_pool.clone());
    // Réglages par conversation agent (modèle/effort/mode) — suivent l'utilisateur
    // entre PCs, comme les onglets. Degrades to no-op without the pool.
    let conversation_meta =
        atelier_common::conversation_meta::ConversationMetaStore::new(meta_pool.clone());
    // EventBus créé AVANT les stores : le PlatformIssueStore et le
    // NotificationStore embarquent le sender de leur canal (`issue` / `notify`,
    // insert + publish indissociables).
    let events = Arc::new(atelier_common::events::EventBus::new());
    // Remontées plateforme (CLAUDE_ISSUES) — store central dans atelier_meta.
    // One-shot : rapatrie les anciens fichiers per-app `{slug}/src/CLAUDE_ISSUES.json`
    // vers la base PUIS les supprime (la feature concerne des bugs plateforme,
    // pas des apps → rien ne doit subsister au niveau projet). Idempotent.
    let issues = atelier_common::issue_store::PlatformIssueStore::new(
        meta_pool.clone(),
        events.issue.clone(),
    );
    issues.backfill_from_files(&apps_src_root).await;
    // Notifications plateforme (notify_user + journal d'actions des agents).
    let notifications = atelier_common::notification_store::NotificationStore::new(
        meta_pool.clone(),
        events.notify.clone(),
    );
    notifications.prune_old_actions().await;
    // Authentification du Claude Agent SDK : token OAuth abonnement longue durée du
    // runner/scan (setup-token). Construit ICI (après notifications, avant
    // init_surveillance ET ApiState::new) car le sink de panne d'auth du watcher en
    // dépend. No-op quand Postgres est down.
    let agent_auth = atelier_common::agent_auth::AgentAuthStore::new(meta_pool.clone());
    // Token Claude destiné aux apps opt-in (injecté en CLAUDE_CODE_OAUTH_TOKEN) —
    // séparé du token runner/scan ci-dessus. No-op quand Postgres est down.
    let app_claude_auth =
        atelier_common::app_claude_auth::AppClaudeAuthStore::new(meta_pool.clone());
    // Statistiques d'utilisation (page /stats) : store des tables app_traffic_daily
    // / agent_turn_usage / app_build_runs. Purge de rétention + réconciliation des
    // builds laissés `running` par un restart d'Atelier, au boot. No-op sans pool.
    let usage_stats = atelier_common::usage_stats::UsageStatsStore::new(meta_pool.clone());
    usage_stats.prune_old().await;
    usage_stats.reconcile_interrupted_builds().await;
    let docs_index = open_docs_index(&meta_pool, &docs_dir).await;

    // Apps supervisor wiring. The registries (apps + ports) live in the shared
    // `atelier_meta` Postgres in a single `applications` table — app and port in
    // one transactional row, so the old apps.json/port-registry.json desync (and
    // its boot-time `reconcile` hack) can no longer happen. Postgres is therefore
    // a hard dependency for supervision: fail fast if the pool is unavailable
    // (mirrors the previous `.expect()` on the local-file registry load).
    let registry_pool = meta_pool
        .clone()
        .expect("control-plane Postgres (atelier_meta) required for the app registry");
    // One-shot backfill from the legacy JSON/SQLite files if the DB is empty.
    if let Err(err) = backfill_control_plane(&registry_pool, &apps_data_dir, &apps_state_dir).await {
        warn!(?err, "control-plane backfill skipped/failed");
    }
    let app_registry = atelier_apps::AppRegistry::new(registry_pool.clone())
        .await
        .expect("Failed to load app registry from Postgres");
    let port_registry =
        atelier_apps::PortRegistry::new(registry_pool.clone(), 3001)
            .await
            .expect("Failed to load port registry from Postgres");

    let supervisor = Arc::new(atelier_apps::AppSupervisor::new(
        app_registry.clone(),
        port_registry.clone(),
        events.app_state.clone(),
    ));
    let context_generator = Arc::new(atelier_apps::context::ContextGenerator::new(
        apps_src_root.clone(),
        base_domain.clone(),
        mcp_endpoint.clone(),
    ));
    info!(
        apps = app_registry.list().await.len(),
        "atelier-apps supervisor wired (Phase 9 prep)"
    );

    // Adopt existing transient units for apps marked Running. À chaque boot
    // d'Atelier, on raccroche les processus déjà supervisés par systemd
    // (sinon /api/apps/.../status renvoie null jusqu'au prochain control).
    if let Err(err) = supervisor.start_all_running().await {
        warn!(?err, "supervisor.start_all_running failed");
    }

    // Watcher inotify des sources d'apps → event WS `source:changed` : l'explorateur
    // du Studio se rafraîchit tout seul (remplace le bouton refresh manuel). Partage
    // le même `Arc<EventBus>` que celui passé à `ApiState::new` ci-dessous.
    source_watcher::spawn_source_watcher(events.clone(), apps_src_root.clone());

    // Câblage d'auth SDK pour le watcher (closures plutôt qu'une dép du watcher vers
    // atelier-common) : (1) provider de token frais injecté au stdin de chaque scan
    // (ré-auth sans restart) ; (2) sink de panne — dédup atomique (claim agent_auth)
    // puis UNE notification plateforme rouge. Un token mort touche tous les scans du
    // sweep → le débounce évite le spam.
    let token_provider: atelier_watcher::TokenProvider = {
        let aa = agent_auth.clone();
        std::sync::Arc::new(move || {
            let aa = aa.clone();
            Box::pin(async move { aa.token().await })
        })
    };
    let on_auth_failure: atelier_watcher::AuthFailureSink = {
        let aa = agent_auth.clone();
        let notif = notifications.clone();
        std::sync::Arc::new(move |msg: String| {
            let (aa, notif) = (aa.clone(), notif.clone());
            tokio::spawn(async move {
                if aa
                    .record_failure(&msg, atelier_common::agent_auth::notify_interval_secs())
                    .await
                {
                    let _ = notif
                        .push(
                            None,
                            "system",
                            "notice",
                            "error",
                            "Authentification Claude expirée",
                            Some(&format!(
                                "Le token OAuth abonnement du runner est expiré/révoqué — scans et \
                                 agent bloqués. Renouvelle-le (`claude setup-token`) puis \
                                 Paramètres → Authentification Claude. Détail : {msg}"
                            )),
                        )
                        .await;
                }
            });
        })
    };

    // Surveillance IA (sécurité + code_review + business). Migrate schema, spawn
    // git_watcher + sweep scheduler loops. Runs manuels, sweep automatique
    // (manuel ou planifié). Le scan-agent est le Claude Agent SDK (runner
    // `scan.js` en hr-studio, OAuth abonnement). Noop si pas de DSN.
    let surveillance = init_surveillance(
        &app_registry,
        &apps_src_root,
        &mcp_endpoint,
        Some(token_provider),
        Some(on_auth_failure),
    )
    .await;

    // Sauvegarde restic+rclone vers Samba. Noop si pas de DSN ; runs manuels
    // (scheduler présent mais désactivé tant que schedule_enabled=false).
    let backup = init_backup(backup_sources).await;

    // Intégration Homeroute : appelle l'API reverse-proxy existante de hr-api
    // (:4000) pour créer/retirer des routes hostname pour les apps. Réutilise le
    // pool control-plane (settings + mapping slug→host) et le bus d'événements ;
    // dégradé en 503 si Postgres absent. La liaison est désactivée par défaut
    // (toggle dans la page Paramètres). Les hostnames ciblent le port d'Atelier
    // lui-même : le middleware host-gate sert l'app sous /apps/{slug}/ (sa base
    // de build) et redirige le reste — cibler le port de l'app est cassé par
    // construction (assets en base absolue → fallback SPA → JS en text/html).
    let atelier_http_port: u16 = http_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4100);
    let homeroute = atelier_api::clients::homeroute_service::HomerouteService::new(
        atelier_common::homeroute::HomerouteStore::new(meta_pool.clone()),
        app_registry.clone(),
        events.clone(),
        atelier_http_port,
    );
    // Seed de la map hostname→slug du host-gate (rechargée ensuite à chaque
    // mutation d'assignation + par le heartbeat).
    homeroute.reload_host_map().await;

    let state = ApiState::new(
        docs_dir.clone(),
        docs_index,
        git,
        apps_state_dir.clone(),
        dv,
        task_store,
        open_tabs,
        conversation_meta,
        issues,
        notifications,
        agent_auth,
        app_claude_auth,
        apps_src_root,
        apps_runtime_root,
        events,
        app_registry,
        port_registry,
        supervisor,
        context_generator,
        logs,
        surveillance,
        backup,
        homeroute,
        usage_stats,
    );
    info!(
        slugs = ?state.preserve_prefix_slugs,
        "apps_proxy: prefix-preserving (no-strip) slugs"
    );

    // Flush périodique du compteur de trafic proxy → `app_traffic_daily` (page
    // /stats). Toutes les 60 s : drain mémoire + UPSERT incrémental ; en cas
    // d'échec SQL, ré-injection des compteurs (aucune perte tant qu'Atelier vit).
    {
        let proxy_stats = state.proxy_stats.clone();
        let usage = state.usage_stats.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tick.tick().await;
                let rows = proxy_stats.drain();
                if rows.is_empty() {
                    continue;
                }
                if let Err(e) = usage.flush_traffic(&rows).await {
                    warn!(error = %e, "stats: flush trafic échoué — ré-injection des compteurs");
                    proxy_stats.merge_back(rows);
                }
            }
        });
    }

    // Historique builds/ships (page /stats) : subscriber central du canal
    // `app_build`. Zéro modif des émetteurs — `started` → INSERT (kind déduit de
    // la phase : `ship` si phase="ship", sinon `build`), `finished`/`error` →
    // clôture. Un run laissé ouvert (crash) est réconcilié au boot suivant.
    {
        let usage = state.usage_stats.clone();
        let mut rx = state.events.app_build.subscribe();
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            let mut inflight: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            loop {
                match rx.recv().await {
                    Ok(ev) => match ev.status.as_str() {
                        "started" => {
                            // Un `started` préexistant pour ce slug n'a jamais reçu son
                            // terminal (build.sh tué, curl terminal perdu) : le clôturer
                            // en `interrupted` avant d'en ouvrir un nouveau, sinon il
                            // resterait `running` jusqu'au prochain boot-reconcile.
                            if let Some(orphan) = inflight.remove(&ev.slug) {
                                usage
                                    .build_finished(&orphan, "interrupted", Some("remplacé par un nouveau build (terminal manquant)"))
                                    .await;
                            }
                            let kind = if ev.phase.as_deref() == Some("ship") {
                                "ship"
                            } else {
                                "build"
                            };
                            if let Some(id) = usage.build_started(&ev.slug, kind).await {
                                inflight.insert(ev.slug.clone(), id);
                            }
                        }
                        "finished" => {
                            if let Some(id) = inflight.remove(&ev.slug) {
                                usage.build_finished(&id, "success", None).await;
                            }
                        }
                        "error" => {
                            if let Some(id) = inflight.remove(&ev.slug) {
                                usage.build_finished(&id, "error", ev.error.as_deref()).await;
                            }
                        }
                        _ => {} // "step" et autres : ignorés
                    },
                    Err(RecvError::Lagged(n)) => {
                        warn!(dropped = n, "stats: build subscriber lagged");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    // Heartbeat Homeroute : enregistre CET environnement auprès de Homeroute au
    // boot puis toutes les ~5 min, pour qu'il apparaisse « en ligne » dans la page
    // Environnements. No-op silencieux si la liaison est désactivée / sans token /
    // Postgres indisponible (la boucle vit jusqu'au shutdown du process).
    {
        let hr = state.homeroute.clone();
        tokio::spawn(async move { hr.heartbeat_loop().await });
    }

    // Boot env reconcile sweep. Renders each app's `.env` as a clean projection
    // of (platform-computed + user) env: injects the dataverse/logging contract,
    // GCs vestigial vars (HR_FLOW_*, …), imports any residual hand-seeded vars
    // into the structured model. Replaces the old dead `sync_dv_env_all`.
    //
    // Gated DRY-RUN by default (logs the plan, writes nothing) so a migration can
    // be inspected before it touches the 5 live apps' `.env`. Set
    // `ATELIER_ENV_RECONCILE_APPLY=1` to actually write. Running apps are already
    // adopted above and only pick up the new `.env` on their next restart.
    {
        let apply = std::env::var("ATELIER_ENV_RECONCILE_APPLY").ok().as_deref() == Some("1");
        let ctx = atelier_api::mcp::apps_ops::AppsContext::from_api_state(&state);
        let reports = ctx.reconcile_all_env(!apply).await;
        info!(apply, apps = reports.len(), "boot env reconcile sweep complete");

        // Boot context regen : le contexte généré (rules/skills/.mcp.json) suit
        // le BINAIRE — un deploy Atelier qui change les renderers se propage aux
        // workspaces dès le restart, sans attendre un AppUpdate ni un
        // `studio.refresh_all` manuel. Idempotent (write_if_changed) ; CLAUDE.md
        // agent-owned intouché (write_if_missing) ; purge aussi les artefacts
        // obsolètes (scripts de skills retirés, rules legacy).
        let (ok, total) = ctx.regenerate_all_contexts().await;
        info!(ok, total, "boot context regen complete");
    }

    let web_dist_opt = if web_dist.is_dir() { Some(web_dist) } else { None };
    let app = atelier_api::router(state, web_dist_opt);

    let listener = TcpListener::bind(&http_addr)
        .await
        .with_context(|| format!("bind HTTP {http_addr}"))?;
    info!(addr = %http_addr, "http listener bound");

    let http_task = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            error!(?err, "http server exited");
        }
    });

    let ipc_task = tokio::spawn(serve_ipc(ipc_sock.clone()));

    shutdown_signal().await;
    info!("shutdown signal received");

    // Drain des conversations agent en vol AVANT de couper le serveur : chaque run reçoit
    // un arrêt propre (interrupt du tour + EOF stdin → le SDK flush un transcript
    // RESUMABLE), pour qu'un `make deploy` ne tronque jamais un tour (sinon la session
    // devient non-relançable). Budget borné, sous le TimeoutStopSec systemd.
    let drain_budget = Duration::from_secs(
        std::env::var("ATELIER_AGENT_DRAIN_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(45),
    );
    atelier_api::routes::agent::drain_agent_runs(drain_budget).await;

    if ipc_sock.exists() {
        if let Err(err) = tokio::fs::remove_file(&ipc_sock).await {
            warn!(?err, path = %ipc_sock.display(), "failed to remove ipc socket");
        }
    }

    http_task.abort();
    ipc_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), http_task).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), ipc_task).await;

    info!("atelier stopped");
    Ok(())
}

/// Bootstrap the surveillance service. Reuses `ATELIER_DV_ADMIN_URL` (the
/// dataverse admin DSN) to CREATE DATABASE `atelier_meta` on first boot,
/// run its migrations, and spawn the git_watcher + sweep scheduler loops.
/// `seed_apps` carries the registry's slugs + names + stack hints (used for
/// prompts, git_watcher and the sweep app list). If the env var is missing, the
/// service starts in noop mode.
///
/// The scan engine is the **Claude Agent SDK** (OAuth subscription, never an API
/// key): `scan.js` is spawned as `hr-studio`, reusing the `ATELIER_AGENT_*`
/// paths + `ATELIER_SCAN_RUNNER`; the agent records findings via MCP at
/// `<ATELIER_MCP_ENDPOINT>?scope=surveillance` (read-only whitelist), MCP token
/// from `MCP_TOKEN` (passed on the runner's stdin). The automatic *sweep*
/// (manual or scheduled) reuses this exact path. Tunables (all optional):
///   - `ATELIER_SCAN_MODEL`         (unset → SDK subscription default = Opus)
///   - `ATELIER_SCAN_EFFORT`        (default "max"; "none" to omit — e.g. Haiku)
///   - `ATELIER_SCAN_TIMEOUT_SECS`  (default 600)
///   - `ATELIER_SCAN_MAX_CONCURRENT`(default 3 — an app's 3 scans run together)
async fn init_surveillance(
    registry: &atelier_apps::AppRegistry,
    apps_src_root: &PathBuf,
    mcp_endpoint: &str,
    token_provider: Option<atelier_watcher::TokenProvider>,
    on_auth_failure: Option<atelier_watcher::AuthFailureSink>,
) -> SurveillanceService {
    let admin_dsn = std::env::var("ATELIER_DV_ADMIN_URL")
        .ok()
        .filter(|s| !s.is_empty());
    if admin_dsn.is_none() {
        warn!("ATELIER_DV_ADMIN_URL absent — surveillance in noop mode");
    }
    let seed_apps: Vec<atelier_watcher::AppMeta> = registry
        .list()
        .await
        .into_iter()
        .map(|a| atelier_watcher::AppMeta {
            slug: a.slug,
            name: a.name,
            stack: a.stack,
        })
        .collect();

    let timeout_secs = std::env::var("ATELIER_SCAN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600u64);
    // Default 3 so the sweep can run an app's three scans (security / code_review
    // / business) truly simultaneously; the sweep is single-flight + barriered
    // app-by-app, so at most 3 scan subprocesses ever run at once.
    let max_concurrent = std::env::var("ATELIER_SCAN_MAX_CONCURRENT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3usize);

    let driver_cfg = atelier_watcher::ClaudeScanConfig {
        node_bin: std::env::var("ATELIER_AGENT_NODE_BIN")
            .unwrap_or_else(|_| "/usr/bin/node".to_string()),
        run_as_user: std::env::var("ATELIER_AGENT_USER")
            .unwrap_or_else(|_| "hr-studio".to_string()),
        claude_config_dir: std::env::var("ATELIER_AGENT_CLAUDE_CONFIG_DIR")
            .unwrap_or_else(|_| "/var/lib/hr-studio/.claude".to_string()),
        scan_script: std::env::var("ATELIER_SCAN_RUNNER")
            .unwrap_or_else(|_| "/opt/atelier/runner/src/scan.js".to_string()),
        // The scope param selects the server-side read-only whitelist.
        mcp_endpoint: format!("{mcp_endpoint}?scope=surveillance"),
        model: std::env::var("ATELIER_SCAN_MODEL").ok().filter(|s| !s.is_empty()),
        // Deepest analysis by default. Set ATELIER_SCAN_EFFORT=none to omit (e.g. Haiku).
        effort: Some(
            std::env::var("ATELIER_SCAN_EFFORT")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "max".to_string()),
        )
        .filter(|s| s != "none"),
        timeout: Duration::from_secs(timeout_secs),
    };
    info!(max_concurrent, "surveillance scan engine: Claude Agent SDK");

    SurveillanceService::start(
        SurveillanceConfig {
            admin_dsn,
            db_name: None,
            seed_apps,
            apps_src_root: apps_src_root.clone(),
            driver: driver_cfg,
            max_concurrent,
        },
        token_provider,
        on_auth_failure,
    )
    .await
}

/// Bootstrap du service de sauvegarde. Réutilise `ATELIER_DV_ADMIN_URL` pour
/// CREATE DATABASE `atelier_meta` (si besoin) + migrations. Les binaires sont
/// configurables (defaults restic/rclone/pg_dumpall). Noop si DSN absent.
///
///   - `ATELIER_RESTIC_BIN`      (default "restic")
///   - `ATELIER_RCLONE_BIN`      (default "rclone")
///   - `ATELIER_PG_DUMPALL_BIN`  (default "pg_dumpall")
///   - `ATELIER_BACKUP_PG_USER`  (default "postgres")
async fn init_backup(sources: SourcePaths) -> BackupService {
    let admin_dsn = std::env::var("ATELIER_DV_ADMIN_URL")
        .ok()
        .filter(|s| !s.is_empty());
    if admin_dsn.is_none() {
        warn!("ATELIER_DV_ADMIN_URL absent — backup in noop mode");
    }
    BackupService::start(BackupServiceConfig {
        admin_dsn,
        db_name: None,
        sources,
        restic_bin: std::env::var("ATELIER_RESTIC_BIN").unwrap_or_else(|_| "restic".to_string()),
        rclone_bin: std::env::var("ATELIER_RCLONE_BIN").unwrap_or_else(|_| "rclone".to_string()),
        pg_dumpall_bin: std::env::var("ATELIER_PG_DUMPALL_BIN")
            .unwrap_or_else(|_| "pg_dumpall".to_string()),
        pg_run_user: std::env::var("ATELIER_BACKUP_PG_USER").unwrap_or_else(|_| "postgres".to_string()),
    })
    .await
}

fn init_tracing(logs: LogIngestService) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_target(true);
    let logging_layer = LoggingLayer::new(logs, "atelier");
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(logging_layer)
        .init();
}

/// Bootstrap the Postgres-backed log ingest service. Reads:
///
/// - `ATELIER_LOGS_DB_ADMIN_URL` — admin DSN (typically the dataverse admin)
///   used to CREATE DATABASE/CREATE ROLE on first boot. Optional if the DB
///   and writer role already exist.
/// - `ATELIER_LOGS_DB_URL` — writer DSN used for INSERT/SELECT on
///   `events_log`. If absent, falls back to the admin DSN swapped to
///   `atelier_logs`.
/// - `ATELIER_LOGS_WRITER_PASSWORD` — only consulted on the very first boot
///   when the writer role doesn't yet exist.
///
/// If neither admin nor writer DSN is set, the service starts in noop mode
/// (logs go to stdout only, no Postgres persistence). This keeps `atelier`
/// bootable even when Postgres is unreachable.
async fn init_logs_ingest() -> LogIngestService {
    let admin_dsn = std::env::var("ATELIER_LOGS_DB_ADMIN_URL")
        .or_else(|_| std::env::var("ATELIER_DV_ADMIN_URL"))
        .ok()
        .filter(|s| !s.is_empty());
    let writer_dsn = std::env::var("ATELIER_LOGS_DB_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let writer_password = std::env::var("ATELIER_LOGS_WRITER_PASSWORD")
        .ok()
        .filter(|s| !s.is_empty());

    if admin_dsn.is_none() && writer_dsn.is_none() {
        warn!("ATELIER_LOGS_DB_URL / ATELIER_DV_ADMIN_URL absent — log ingest in noop mode");
    }

    let cfg = LogIngestConfig {
        admin_dsn,
        writer_dsn,
        writer_password,
        ..LogIngestConfig::default()
    };
    LogIngestService::start(cfg).await
}

/// Seed `/opt/atelier/data/{apps,port-registry}.json` from the read-only mirror
/// `/var/lib/atelier/state/` if Atelier's canonical writer copy is missing.
/// Idempotent : ne touche pas aux fichiers existants.
fn seed_apps_data(data_dir: &PathBuf, state_dir: &PathBuf) {
    if let Err(err) = std::fs::create_dir_all(data_dir) {
        warn!(?err, path = %data_dir.display(), "failed to create apps data dir");
        return;
    }
    for file in ["apps.json", "port-registry.json"] {
        let dst = data_dir.join(file);
        if dst.exists() {
            continue;
        }
        let src = state_dir.join(file);
        if !src.exists() {
            continue;
        }
        match std::fs::copy(&src, &dst) {
            Ok(n) => info!(bytes = n, src = %src.display(), dst = %dst.display(), "seeded apps data"),
            Err(err) => warn!(?err, src = %src.display(), "seed apps data failed"),
        }
    }
}

/// One-shot, idempotent import of the legacy file-based control plane into the
/// `applications` table. Runs only when the table is empty (fresh cutover);
/// once populated, Postgres is the source of truth and this is a no-op.
///
/// Merges `apps.json` (app metadata) with `port-registry.json` (slug→port) into
/// one row per app. The legacy files are left in place as a rollback safety net.
async fn backfill_control_plane(
    pool: &atelier_common::control_db::sqlx::PgPool,
    apps_data_dir: &PathBuf,
    state_dir: &PathBuf,
) -> anyhow::Result<()> {
    use atelier_common::control_db::sqlx::{Row, query};

    // Ensure the legacy files are present (seed from the read-only mirror) so a
    // first boot after the schema migration can import them.
    seed_apps_data(apps_data_dir, state_dir);

    let row = query("SELECT COUNT(*) AS n FROM applications")
        .fetch_one(pool)
        .await?;
    let existing: i64 = row.get("n");
    if existing > 0 {
        return Ok(());
    }

    let apps_path = apps_data_dir.join("apps.json");
    let ports_path = apps_data_dir.join("port-registry.json");
    if !apps_path.exists() {
        info!("backfill: no apps.json — starting with an empty registry");
        return Ok(());
    }

    let apps: Vec<atelier_apps::Application> = {
        let bytes = std::fs::read(&apps_path)?;
        if bytes.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&bytes)?
        }
    };
    let ports: std::collections::BTreeMap<String, u16> = if ports_path.exists() {
        let bytes = std::fs::read(&ports_path)?;
        if bytes.is_empty() {
            Default::default()
        } else {
            // {base_port, assignments:{slug:port}} — only assignments matter here.
            let v: serde_json::Value = serde_json::from_slice(&bytes)?;
            serde_json::from_value(v.get("assignments").cloned().unwrap_or_default())
                .unwrap_or_default()
        }
    } else {
        Default::default()
    };

    let mut imported = 0u32;
    for mut app in apps {
        // The port registry file is authoritative for the port (apps.json's copy
        // is the one that "drifts"); fall back to the app's own port otherwise.
        let port = ports.get(&app.slug).copied().unwrap_or(app.port);
        app.port = port;
        let data = serde_json::to_value(&app)?;
        let port_col: Option<i32> = if port == 0 { None } else { Some(port as i32) };
        query(
            "INSERT INTO applications (slug, port, state, data, updated_at) \
             VALUES ($1, $2, $3, $4, now()) ON CONFLICT (slug) DO NOTHING",
        )
        .bind(&app.slug)
        .bind(port_col)
        .bind(app.state.as_str())
        .bind(&data)
        .execute(pool)
        .await?;
        imported += 1;
    }
    info!(imported, "control-plane backfill complete (apps.json/port-registry.json → Postgres)");
    Ok(())
}

/// Open the shared control-plane Postgres pool (`atelier_meta`) via the
/// dataverse admin DSN and apply the control-plane DDL. Returns `None` when the
/// DSN is absent or Postgres is unreachable — the control-plane stores then run
/// in degraded mode (matching the soft-dependency behaviour of dv/logs/surveillance).
async fn init_control_db() -> Option<atelier_common::control_db::sqlx::PgPool> {
    let admin_dsn = match std::env::var("ATELIER_DV_ADMIN_URL").ok().filter(|s| !s.is_empty()) {
        Some(u) => u,
        None => {
            warn!("ATELIER_DV_ADMIN_URL absent — control-plane Postgres désactivé (mode dégradé)");
            return None;
        }
    };
    match atelier_common::control_db::bootstrap(&admin_dsn).await {
        Ok(pool) => {
            info!("control-plane Postgres ready (atelier_meta)");
            Some(pool)
        }
        Err(err) => {
            warn!(?err, "control-plane Postgres bootstrap failed — mode dégradé");
            None
        }
    }
}

async fn init_dv(state_dir: &PathBuf) -> Option<Arc<atelier_dataverse::manager::DataverseManager>> {
    let admin_dsn = match std::env::var("ATELIER_DV_ADMIN_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            warn!("ATELIER_DV_ADMIN_URL absent — /api/dv désactivé");
            return None;
        }
    };
    let secrets_path = state_dir.join("dataverse-secrets.json");
    if !secrets_path.exists() {
        warn!(path = %secrets_path.display(), "dataverse-secrets.json absent — /api/dv désactivé");
        return None;
    }

    let mgr = match atelier_dataverse::manager::DataverseManager::connect_admin(
        admin_dsn,
        atelier_dataverse::provisioning::ProvisioningConfig::default(),
        Some(secrets_path.clone()),
    )
    .await
    {
        Ok(m) => m,
        Err(err) => {
            warn!(?err, "DataverseManager init failed — /api/dv désactivé");
            return None;
        }
    };

    // Override des DSN per-slug pour rediriger 127.0.0.1 → 10.0.0.254.
    let dv_host = std::env::var("ATELIER_DV_HOST").unwrap_or_else(|_| DEFAULT_DV_HOST.to_string());
    if let Ok(bytes) = std::fs::read(&secrets_path) {
        if let Ok(secrets) =
            serde_json::from_slice::<atelier_dataverse::manager::SecretsFile>(&bytes)
        {
            let mut applied = 0;
            for (slug, sec) in secrets.apps.iter() {
                let swapped = sec
                    .dsn
                    .replace("127.0.0.1", &dv_host)
                    .replace("@localhost:", &format!("@{dv_host}:"));
                mgr.set_dsn_override(slug.clone(), swapped).await;
                applied += 1;
            }
            info!(
                count = applied,
                host = %dv_host,
                "Dataverse DSN overrides loaded"
            );
        }
    }

    Some(Arc::new(mgr))
}

/// Build the Postgres-backed docs search index over the shared `atelier_meta`
/// pool, rebuilding it from the filesystem if the table is empty. Returns `None`
/// when the control-plane pool is absent (Postgres down) — search then degrades
/// to 503.
async fn open_docs_index(
    meta_pool: &Option<atelier_common::control_db::sqlx::PgPool>,
    docs_dir: &PathBuf,
) -> Option<Arc<atelier_docs::Index>> {
    let pool = meta_pool.clone()?;
    match atelier_docs::Index::new_or_rebuild(pool, docs_dir.clone()).await {
        Ok(idx) => {
            let count = idx.count().await.unwrap_or(0);
            info!(entries = count, "docs index ready (atelier_meta)");
            Some(Arc::new(idx))
        }
        Err(err) => {
            warn!(?err, "failed to open docs index — search disabled");
            None
        }
    }
}

async fn serve_ipc(path: PathBuf) -> Result<()> {
    if path.exists() {
        if let Err(err) = tokio::fs::remove_file(&path).await {
            warn!(?err, path = %path.display(), "failed to remove stale ipc socket");
        }
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("bind IPC {}", path.display()))?;
    info!(path = %path.display(), "ipc listener bound");

    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(c) => c,
            Err(err) => {
                warn!(?err, "ipc accept failed");
                continue;
            }
        };
        drop(stream);
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
