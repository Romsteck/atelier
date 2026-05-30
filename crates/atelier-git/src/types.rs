use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub slug: String,
    pub size_bytes: u64,
    pub head_ref: Option<String>,
    pub commit_count: u64,
    pub last_commit: Option<DateTime<Utc>>,
    pub branches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    pub date: DateTime<Utc>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
}

#[derive(Debug, Clone)]
pub struct CgiResponse {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

// Phase 3: GitHub mirror types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default)]
    pub github_token: Option<String>,
    #[serde(default)]
    pub github_org: String,
    #[serde(default)]
    pub mirrors: HashMap<String, MirrorConfig>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            github_token: None,
            github_org: String::new(),
            mirrors: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorConfig {
    pub enabled: bool,
    #[serde(default)]
    pub github_ssh_url: Option<String>,
    pub visibility: RepoVisibility,
    #[serde(default)]
    pub last_sync: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RepoVisibility {
    Public,
    Private,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKeyInfo {
    pub public_key: String,
    pub exists: bool,
}
