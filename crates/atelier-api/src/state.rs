use std::path::PathBuf;
use std::sync::Arc;

use atelier_backup::BackupService;
use atelier_logging::LogIngestService;
use atelier_watcher::SurveillanceService;
use atelier_pilot::PilotService;
use atelier_apps::{AppRegistry, AppSupervisor, PortRegistry};
use atelier_apps::context::ContextGenerator;
use atelier_common::agent_auth::AgentAuthStore;
use atelier_common::app_claude_auth::AppClaudeAuthStore;
use atelier_common::agent_ui_state::OpenTabsStore;
use atelier_common::codex_auth::CodexAuthStore;
use atelier_common::conversation_meta::ConversationMetaStore;
use atelier_common::events::EventBus;
use atelier_common::notification_store::NotificationStore;
use atelier_common::task_store::TaskStore;
use atelier_common::usage_stats::UsageStatsStore;
use crate::routes::stats::{ProxyStats, StatsCache};

#[derive(Clone)]
pub struct ApiState {
    // Docs
    pub docs_dir: PathBuf,
    pub docs_index: Option<Arc<atelier_docs::Index>>,

    // Git
    pub git: Arc<atelier_git::GitService>,

    // Apps : sources synced + canonical writer
    pub apps_state_dir: PathBuf,
    pub apps_src_root: PathBuf,
    pub apps_runtime_root: PathBuf,

    // Tasks
    pub task_store: Arc<TaskStore>,

    /// Studio open-tabs state (conversations/files/diffs/commits + active tab),
    /// per app, in `atelier_meta`. Source of truth for cross-PC tab sync; pairs
    /// with the `agent_open_tabs` WS broadcast. No-op when Postgres is down.
    pub open_tabs: OpenTabsStore,

    /// Réglages par conversation agent (modèle/effort/mode) dans
    /// `atelier_meta.agent_conversation_meta`. Source de vérité serveur pour que
    /// rouvrir une conversation depuis un autre PC restaure (et relance) le bon
    /// modèle/effort au lieu du défaut localStorage. No-op quand Postgres est down.
    pub conversation_meta: ConversationMetaStore,

    /// Notifications plateforme (canal agent → utilisateur) : tool MCP
    /// `notify_user` + journal automatique des actions des agents, dans
    /// `atelier_meta.platform_notifications`. Le store publie lui-même sur le
    /// canal `notify` de l'EventBus (relayé en WS `notify:event`). No-op/vide
    /// quand Postgres est down.
    pub notifications: NotificationStore,

    /// Authentification du Claude Agent SDK (token OAuth abonnement longue durée
    /// du runner/scan) dans `atelier_meta.agent_auth`. Lu FRAIS à chaque run pour
    /// injecter le token par stdin ; endpoints `/api/agent/sdk/auth`. No-op quand
    /// Postgres est down.
    pub agent_auth: AgentAuthStore,

    /// Token Claude destiné aux APPS opt-in (`Application.claude_access`), injecté
    /// comme var plateforme calculée `CLAUDE_CODE_OAUTH_TOKEN` — SÉPARÉ du token
    /// runner/scan (`agent_auth`) pour borner le rayon de fuite. Dans
    /// `atelier_meta.app_claude_auth` ; endpoints `/api/agent/apps-token`. Remplace
    /// le hack `CLAUDE_CONFIG_DIR` → dossier hr-studio (iss-d10ef97b). No-op quand
    /// Postgres est down.
    pub app_claude_auth: AppClaudeAuthStore,

    /// Authentification du moteur **Codex** (OAuth abonnement ChatGPT uniquement)
    /// dans `atelier_meta.codex_auth` ; endpoints `/api/agent/codex/auth*`. Ne
    /// porte qu'un SEED d'`auth.json` + la télémétrie : la vérité runtime est le
    /// fichier `$CODEX_HOME/auth.json` que le CLI rafraîchit seul (un device-login
    /// écrit le fichier sans passer par PG). No-op quand Postgres est down.
    pub codex_auth: CodexAuthStore,

    // Dataverse
    pub dv: Option<Arc<atelier_dataverse::manager::DataverseManager>>,

    // Apps supervisor (Phase 9 cutover) — Atelier devient le writer.
    pub events: Arc<EventBus>,
    pub app_registry: AppRegistry,
    pub port_registry: PortRegistry,
    pub supervisor: Arc<AppSupervisor>,
    pub context_generator: Arc<ContextGenerator>,

    /// Per-slug build/ship locks, created once at boot and shared by the HTTP
    /// `ship` route and the MCP `app.build`/`ship` tool handlers. Without a
    /// shared map each request rebuilds an empty one and the BUILD_BUSY guard
    /// never fires.
    pub build_locks:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,

    /// Centralized logging — Postgres-backed ring/flush ingest service. Used
    /// by the in-process tracing layer, the HTTP `/api/logs/ingest` endpoint,
    /// and the WebSocket live stream.
    pub logs: LogIngestService,

    /// Surveillance IA service (findings / cron / memory). Endpoints under
    /// `/api/findings` and `/api/apps/:slug/surveillance/*` return 503 when
    /// this service is in noop mode (Postgres unreachable at boot).
    pub surveillance: SurveillanceService,

    /// Service de sauvegarde (restic+rclone vers Samba). Endpoints sous
    /// `/api/backup/*` ; renvoie 503 en mode noop (Postgres injoignable au boot).
    pub backup: BackupService,

    /// Backlog projet + exécutions autonomes manuelles/nocturnes. Le service
    /// reste présent en mode dégradé et renvoie 503 si atelier_meta est absent.
    pub pilot: PilotService,

    /// Intégration reverse-proxy Homeroute : appelle l'API hr-api existante pour
    /// créer/retirer des routes hostname pour les apps. Endpoints sous
    /// `/api/homeroute/*` ; renvoie 503 si le control-plane Postgres est absent.
    pub homeroute: crate::clients::homeroute_service::HomerouteService,

    /// Statistiques d'utilisation (page `/stats`) : store des tables
    /// `app_traffic_daily` / `agent_turn_usage` / `app_build_runs` dans
    /// `atelier_meta` + agrégations. No-op/vide quand Postgres est down.
    pub usage_stats: UsageStatsStore,

    /// Compteur mémoire de trafic HTTP/WS par app, alimenté par le path-proxy
    /// (zéro écriture SQL par requête) et flushé périodiquement vers
    /// `usage_stats`. Partagé (Arc) : le proxy écrit, la boucle de flush lit.
    pub proxy_stats: Arc<ProxyStats>,

    /// Cache TTL en mémoire des endpoints `/stats` coûteux (dataverse, disque,
    /// git). Partagé par tous les handlers.
    pub stats_cache: Arc<StatsCache>,

    /// Slugs whose `/apps/{slug}` path prefix must be PRESERVED (no-strip) when
    /// proxying to the app — required by Next.js apps whose `basePath`/`assetPrefix`
    /// expect the prefix on every request. SPA (Vite) / Axum apps want the prefix
    /// stripped and are absent here. Parsed once at boot from
    /// `ATELIER_PRESERVE_PREFIX_SLUGS` (comma-separated); defaults to `{"www"}`.
    pub preserve_prefix_slugs: std::collections::HashSet<String>,
}

impl ApiState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        docs_dir: PathBuf,
        docs_index: Option<Arc<atelier_docs::Index>>,
        git: Arc<atelier_git::GitService>,
        apps_state_dir: PathBuf,
        dv: Option<Arc<atelier_dataverse::manager::DataverseManager>>,
        task_store: Arc<TaskStore>,
        open_tabs: OpenTabsStore,
        conversation_meta: ConversationMetaStore,
        notifications: NotificationStore,
        agent_auth: AgentAuthStore,
        app_claude_auth: AppClaudeAuthStore,
        codex_auth: CodexAuthStore,
        apps_src_root: PathBuf,
        apps_runtime_root: PathBuf,
        events: Arc<EventBus>,
        app_registry: AppRegistry,
        port_registry: PortRegistry,
        supervisor: Arc<AppSupervisor>,
        context_generator: Arc<ContextGenerator>,
        logs: LogIngestService,
        surveillance: SurveillanceService,
        backup: BackupService,
        pilot: PilotService,
        homeroute: crate::clients::homeroute_service::HomerouteService,
        usage_stats: UsageStatsStore,
    ) -> Self {
        Self {
            docs_dir,
            docs_index,
            git,
            apps_state_dir,
            apps_src_root,
            apps_runtime_root,
            task_store,
            open_tabs,
            conversation_meta,
            notifications,
            agent_auth,
            app_claude_auth,
            codex_auth,
            dv,
            events,
            app_registry,
            port_registry,
            supervisor,
            context_generator,
            build_locks: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            logs,
            surveillance,
            backup,
            pilot,
            homeroute,
            usage_stats,
            proxy_stats: Arc::new(ProxyStats::new()),
            stats_cache: Arc::new(StatsCache::new()),
            preserve_prefix_slugs: parse_preserve_prefix_slugs(),
        }
    }
}

/// Read `ATELIER_PRESERVE_PREFIX_SLUGS` (comma-separated app slugs) into a set.
/// Defaults to `{"www"}` when unset — `www` is the canonical path-routed Next.js
/// app, and this mirrors the `www` default of `ATELIER_NEXTJS_FALLBACK_SLUG`.
pub fn parse_preserve_prefix_slugs() -> std::collections::HashSet<String> {
    match std::env::var("ATELIER_PRESERVE_PREFIX_SLUGS") {
        Ok(raw) => raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => ["www".to_string()].into_iter().collect(),
    }
}
