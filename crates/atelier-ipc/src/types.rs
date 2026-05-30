use serde::{Deserialize, Serialize};

// ── IPC Request (client → hr-netcore) ───────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcRequest {
    ReloadConfig,
    DnsCacheStats,
    DnsStatus,
    DnsStaticRecords,
    DnsAddStaticRecord {
        name: String,
        record_type: String,
        value: String,
        ttl: u32,
    },
    DnsRemoveStaticRecordsByValue {
        value: String,
    },
    /// Replace the full set of static records owned by a given source.
    /// User records (managed_by = None) and records owned by other sources are preserved.
    DnsSetManagedRecords {
        owner: String,
        records: Vec<StaticRecordDto>,
    },
    DhcpLeases,
    AdblockStats,
    AdblockWhitelistList,
    AdblockWhitelistAdd {
        domain: String,
    },
    AdblockWhitelistRemove {
        domain: String,
    },
    AdblockUpdate,
    AdblockSearch {
        query: String,
        limit: Option<usize>,
    },
    ServiceStatus,
}

// ── IPC Response (hr-netcore → client) ──────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl IpcResponse {
    pub fn ok_empty() -> Self {
        Self {
            ok: true,
            error: None,
            data: None,
        }
    }

    pub fn ok_data(data: impl Serialize) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(serde_json::to_value(data).unwrap_or_default()),
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: None,
        }
    }
}

// ── DTOs (Data Transfer Objects) ────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct LeaseInfo {
    pub ip: String,
    pub mac: String,
    pub hostname: Option<String>,
    pub expiry: u64,
    pub client_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DnsCacheStatsData {
    pub cache_size: usize,
    pub adblock_enabled: bool,
    pub adblock_domains: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DnsStatusData {
    pub active: bool,
    pub port: u16,
    pub upstream_servers: Vec<String>,
    pub cache_size: usize,
    pub local_domain: String,
    pub adblock_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticRecordDto {
    pub name: String,
    pub record_type: String,
    pub value: String,
    pub ttl: u32,
    /// Some("hr-edge") = auto-généré depuis le reverse proxy.
    /// None = record utilisateur. Présent dans la réponse pour permettre à l'UI
    /// de distinguer les deux univers ; ignoré côté serveur lors de l'écriture
    /// (l'owner est stamped par le handler).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DnsStaticRecordsData {
    pub records: Vec<StaticRecordDto>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdblockSourceInfo {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdblockStatsData {
    pub domain_count: usize,
    pub enabled: bool,
    pub sources: Vec<AdblockSourceInfo>,
    pub last_update: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdblockUpdateResult {
    pub total_domains: usize,
    pub sources: Vec<AdblockSourceResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdblockSourceResult {
    pub name: String,
    pub domains: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdblockSearchResult {
    pub query: String,
    pub is_blocked: bool,
    pub results: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceStatusEntry {
    pub name: String,
    pub state: String,
    pub priority: String,
    pub restart_count: u32,
    pub last_state_change: u64,
    pub error: Option<String>,
}

