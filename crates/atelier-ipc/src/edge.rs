use crate::types::IpcResponse;
use serde::{Deserialize, Serialize};

// ── EdgeRequest (client -> hr-edge) ───────────────────────
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum EdgeRequest {
    // Route management
    SetAppRoute {
        domain: String,
        app_id: String,
        host_id: String,
        target_ip: String,
        target_port: u16,
        auth_required: bool,
        local_only: bool,
    },
    RemoveAppRoute {
        domain: String,
    },
    ListAppRoutes,

    // Proxy config
    ReloadConfig,
    GetProxyConfig,
    SaveProxyConfig {
        config: serde_json::Value,
    },

    // ACME
    AcmeStatus,
    AcmeListCertificates,
    AcmeRenewAll,

    // Auth (single user — login + session validation only)
    AuthLogin {
        username: String,
        password: String,
        client_ip: String,
    },
    AuthLogout {
        session_token: String,
    },
    AuthValidateSession {
        session_token: String,
    },
    AuthChangePassword {
        username: String,
        old_password: String,
        new_password: String,
    },

    // Stats / metrics
    GetStats,
}

// ── EdgeClient ───────────────────────────────────────────
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Client IPC pour communiquer avec hr-edge.
#[derive(Clone)]
pub struct EdgeClient {
    socket_path: PathBuf,
}

impl EdgeClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn request(&self, req: &EdgeRequest) -> Result<IpcResponse> {
        crate::transport::request(&self.socket_path, req, Duration::from_secs(5)).await
    }

    pub async fn request_with_timeout(
        &self,
        req: &EdgeRequest,
        timeout: Duration,
    ) -> Result<IpcResponse> {
        crate::transport::request(&self.socket_path, req, timeout).await
    }

    // Typed helpers

    pub async fn set_app_route(
        &self,
        domain: String,
        app_id: String,
        host_id: String,
        target_ip: String,
        target_port: u16,
        auth_required: bool,
        local_only: bool,
    ) -> Result<IpcResponse> {
        self.request(&EdgeRequest::SetAppRoute {
            domain,
            app_id,
            host_id,
            target_ip,
            target_port,
            auth_required,
            local_only,
        })
        .await
    }

    pub async fn remove_app_route(&self, domain: &str) -> Result<IpcResponse> {
        self.request(&EdgeRequest::RemoveAppRoute {
            domain: domain.to_string(),
        })
        .await
    }
}
