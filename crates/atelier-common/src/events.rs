use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Bus d'événements pour la communication inter-services
pub struct EventBus {
    /// Changements de statut hôtes (monitoring → websocket)
    pub host_status: broadcast::Sender<HostStatusEvent>,
    /// Notifications de changement de config (API → services pour reload)
    pub config_changed: broadcast::Sender<ConfigChangeEvent>,
    /// Agent status change events (registry → websocket)
    pub agent_status: broadcast::Sender<AgentStatusEvent>,
    /// Agent metrics events (registry → websocket)
    pub agent_metrics: broadcast::Sender<AgentMetricsEvent>,
    /// Agent update events (registry → websocket)
    pub agent_update: broadcast::Sender<AgentUpdateEvent>,
    /// Host metrics events (host-agent → websocket)
    pub host_metrics: broadcast::Sender<HostMetricsEvent>,
    /// Host power state events (registry → proxy/websocket for WOD progress)
    pub host_power: broadcast::Sender<HostPowerEvent>,
    /// Certificate ready events (ACME → main for dynamic TLS loading)
    pub cert_ready: broadcast::Sender<CertReadyEvent>,
    /// Unified update scan events (registry → websocket)
    pub update_scan: broadcast::Sender<UpdateScanEvent>,
    /// Task update events (task store → websocket)
    pub task_update: broadcast::Sender<crate::tasks::TaskUpdateEvent>,
    /// Energy metrics events (energy poller → websocket)
    pub energy_metrics: broadcast::Sender<EnergyMetricsEvent>,
    /// Log entry events (logging layer → websocket for live log viewer)
    pub log_entry: broadcast::Sender<crate::logging::LogEntry>,
    /// App state change events (supervisor → websocket for live status)
    pub app_state: broadcast::Sender<AppStateEvent>,
    /// App build progress events (supervisor build pipeline → websocket)
    pub app_build: broadcast::Sender<AppBuildEvent>,
    /// Source file change events (filesystem watcher → websocket for the Studio
    /// file-explorer auto-refresh). Coarse per-slug : le front relit l'arbre.
    pub source_changed: broadcast::Sender<SourceChangedEvent>,
    /// Per-app todos change events (todos manager → websocket for Studio right-panel)
    pub app_todos: broadcast::Sender<AppTodosEvent>,
    /// Agent SDK run events (Node runner NDJSON → websocket for live chat stream).
    /// Buffer larger than the others: token deltas can burst.
    pub agent: broadcast::Sender<AgentEvent>,
    /// Studio open-tabs state change (a PUT to `/agent/open-tabs` → websocket) so
    /// every connected browser (incl. other PCs) re-syncs its open tab set live.
    pub agent_open_tabs: broadcast::Sender<AgentOpenTabsEvent>,
    /// Studio top-level tab selection change (a PUT to `/studio/tab` → websocket)
    /// so an already-open Studio tab switches live (homepage deep-link path).
    pub studio_tab: broadcast::Sender<StudioTabEvent>,
    /// Homeroute reverse-proxy route change (assign/remove/toggle/settings →
    /// websocket) so the Settings page reloads its app-routes view live.
    pub homeroute_routes: broadcast::Sender<HomerouteRoutesEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            host_status: broadcast::channel(64).0,
            config_changed: broadcast::channel(16).0,
            agent_status: broadcast::channel(64).0,
            agent_metrics: broadcast::channel(64).0,
            agent_update: broadcast::channel(64).0,
            host_metrics: broadcast::channel(64).0,
            host_power: broadcast::channel(64).0,
            cert_ready: broadcast::channel(16).0,
            update_scan: broadcast::channel(256).0,
            task_update: broadcast::channel(64).0,
            energy_metrics: broadcast::channel(64).0,
            log_entry: broadcast::channel(512).0,
            app_state: broadcast::channel(64).0,
            app_build: broadcast::channel(128).0,
            source_changed: broadcast::channel(128).0,
            app_todos: broadcast::channel(64).0,
            agent: broadcast::channel(2048).0,
            agent_open_tabs: broadcast::channel(64).0,
            studio_tab: broadcast::channel(64).0,
            homeroute_routes: broadcast::channel(16).0,
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatusEvent {
    pub host_id: String,
    pub status: String,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigChangeEvent {
    ProxyRoutes,
    DnsDhcp,
    Adblock,
    Users,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusEvent {
    pub app_id: String,
    pub slug: String,
    pub status: String,
    /// Optional step description for deployment progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Agent metrics event (registry → websocket for frontend display).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetricsEvent {
    pub app_id: String,
    pub memory_bytes: u64,
    pub cpu_percent: f32,
}

/// Agent update status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentUpdateStatus {
    /// Update message sent to agent.
    Notified,
    /// Agent reconnected after update.
    Reconnected,
    /// Agent version verified as expected.
    VersionVerified,
    /// Update failed (agent did not reconnect or wrong version).
    Failed,
}

/// Agent update event (registry → websocket for update progress).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUpdateEvent {
    pub app_id: String,
    pub slug: String,
    pub status: AgentUpdateStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}


/// Host metrics event (host-agent → websocket for frontend display).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMetricsEvent {
    pub host_id: String,
    pub cpu_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
}

/// Power state of a remote host (state machine for WOL/shutdown/reboot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPowerState {
    Online,
    Offline,
    WakingUp,
    ShuttingDown,
    Rebooting,
}

impl std::fmt::Display for HostPowerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::WakingUp => write!(f, "waking_up"),
            Self::ShuttingDown => write!(f, "shutting_down"),
            Self::Rebooting => write!(f, "rebooting"),
        }
    }
}

/// Host power state change event (registry → proxy SSE / websocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostPowerEvent {
    pub host_id: String,
    pub state: HostPowerState,
    pub message: String,
}

/// Result of a wake host request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeResult {
    /// WOL magic packet was sent.
    WolSent,
    /// Host is already waking up (WOL dedup).
    AlreadyWaking,
    /// Host is already online.
    AlreadyOnline,
}

/// Power action for conflict checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Shutdown,
    Reboot,
}

/// Emitted when a new TLS certificate is ready to be loaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertReadyEvent {
    pub slug: String,
    pub wildcard_domain: String,
    pub cert_path: String,
    pub key_path: String,
}

/// Unified update scan event (scan progress + upgrade progress).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum UpdateScanEvent {
    ScanStarted {
        scan_id: String,
    },
    TargetScanned {
        scan_id: String,
        target: UpdateTarget,
    },
    ScanComplete {
        scan_id: String,
    },
    UpgradeStarted {
        target_id: String,
        category: String,
    },
    UpgradeOutput {
        target_id: String,
        line: String,
    },
    UpgradeComplete {
        target_id: String,
        category: String,
        success: bool,
        error: Option<String>,
    },
}

/// Unified update target — represents one scannable host or container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTarget {
    pub id: String,
    pub name: String,
    pub target_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub online: bool,
    pub os_upgradable: u32,
    pub os_security: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_version_latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_cli_installed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_cli_latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_server_installed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_server_latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_ext_installed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_ext_latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_error: Option<String>,
    pub scanned_at: String,
}

/// Per-core CPU metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreMetrics {
    pub core_id: u32,
    pub frequency_mhz: u32,
    pub governor: String,
    pub min_freq_mhz: u32,
    pub max_freq_mhz: u32,
}

/// App state change event (supervisor → websocket for live status display).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStateEvent {
    pub slug: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub port: u16,
    pub uptime_secs: u64,
    pub restart_count: u32,
}

/// App build progress event (orchestrator build pipeline → websocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppBuildEvent {
    pub slug: String,
    /// One of: "started" | "step" | "finished" | "error"
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<u32>,
    /// e.g. "ssh-probe" | "rsync-up" | "compile" | "rsync-back" | "restart"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Émis (debouncé) quand un fichier sous `{slug}/src` change — watcher inotify →
/// websocket pour l'auto-refresh de l'explorateur du Studio. Coarse par slug :
/// le front relit l'arbre, on ne transporte pas le chemin précis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceChangedEvent {
    pub slug: String,
}

/// Per-app todos change event (todos manager → websocket for Studio panel).
/// `todos` is a full snapshot of the app's todo list (kept as generic JSON
/// values to avoid a dependency cycle with `atelier-apps`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTodosEvent {
    pub slug: String,
    pub todos: Vec<serde_json::Value>,
}

/// Live event from an agent run (the Node runner's NDJSON, normalized + tagged).
/// `run_id` identifies the live process; `session_id` (once the SDK reports it via
/// the runner's first `system` line) is the STABLE conversation key the frontend
/// routes by — a conversation keeps its `session_id` across resumes while `run_id`
/// changes per process. `seq` orders events across the whole session. `kind`
/// mirrors the runner's `t` field plus backend lifecycle markers (`started`/`done`).
/// `data` carries the payload as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvent {
    pub run_id: String,
    /// SDK session id — `None` on the early `started` event (before the runner's
    /// first `system` line), `Some` thereafter. Frontend routes by `session_id`
    /// and falls back to `run_id` for that early window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub slug: String,
    pub seq: u64,
    /// "started" | "system" | "assistant_delta" | "thinking_delta" |
    /// "tool_use" | "tool_result" | "question" | "result" | "turn_done" |
    /// "error" | "done"
    pub kind: String,
    pub data: serde_json::Value,
}

/// Studio open-tabs state for one app (full snapshot — last write wins). Emitted
/// on every `PUT /api/apps/{slug}/agent/open-tabs` so connected clients reconcile
/// their open tab set + active tab. `tabs` is the ordered descriptor array (kept
/// as generic JSON: the shape is owned by the frontend's RESTORE_TABS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOpenTabsEvent {
    pub slug: String,
    pub tabs: serde_json::Value,
    #[serde(default)]
    pub active: Option<String>,
}

/// Studio TOP-LEVEL tab selection for one app (code/preview/db/…/surveillance),
/// persisted per app in `agent_open_tabs.studio_tab`. Emitted on every
/// `PUT /api/apps/{slug}/studio/tab` so an ALREADY-OPEN Studio tab (which holds a
/// live WS connection) switches instantly — this is how a homepage deep-link
/// reaches a Studio tab without any URL/cross-tab trick. `kind` carries the
/// surveillance sub-scan when the deep-link targets it (else None).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioTabEvent {
    pub slug: String,
    pub tab: String,
    #[serde(default)]
    pub kind: Option<String>,
}

/// Homeroute route change event (homeroute service → websocket). Coarse: the
/// front reloads its `/api/homeroute/app-routes` view on any change. `action` is
/// "assigned" | "removed" | "toggled" | "settings".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomerouteRoutesEvent {
    pub slug: String,
    pub action: String,
}

/// Energy metrics event (energy poller → websocket for frontend display).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergyMetricsEvent {
    pub host_id: String,
    pub host_name: String,
    pub online: bool,
    pub temperature: Option<f64>,
    pub cpu_percent: f32,
    pub frequency_ghz: f64,
    pub frequency_min_ghz: Option<f64>,
    pub frequency_max_ghz: Option<f64>,
    pub governor: String,
    pub mode: String,
    pub cores: usize,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_core: Option<Vec<CoreMetrics>>,
}
