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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    pub date: DateTime<Utc>,
    pub message: String,
    // Stats de diff (vs premier parent). Défaut 0 : merges/empty commits ont
    // légitimement 0, et `#[serde(default)]` garde le contrat JSON additif.
    #[serde(default)]
    pub additions: u32,
    #[serde(default)]
    pub deletions: u32,
    #[serde(default)]
    pub files_changed: u32,
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

// --- Activité (heatmap GitHub-like) ---

/// Un jour de la timeline de commits. `date` = jour calendaire local du
/// committer tel qu'enregistré dans le commit (cf. `get_commit_activity`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitActivityBucket {
    pub date: String, // "YYYY-MM-DD"
    pub count: u32,
}

// --- Détail + diff d'un commit ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FileStatus {
    A, // added
    M, // modified
    D, // deleted
    R, // renamed
    C, // copied
    T, // typechange
    U, // unmerged
    X, // unknown
}

impl FileStatus {
    /// Mappe la première lettre du `--name-status` git (ex. `R100`, `M`, `A`).
    pub fn from_letter(c: char) -> Self {
        match c.to_ascii_uppercase() {
            'A' => Self::A,
            'M' => Self::M,
            'D' => Self::D,
            'R' => Self::R,
            'C' => Self::C,
            'T' => Self::T,
            'U' => Self::U,
            _ => Self::X,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>, // renommages/copies
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitDetail {
    pub hash: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: DateTime<Utc>,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: DateTime<Utc>,
    pub parents: Vec<String>,
    pub subject: String,
    pub body: String,
    pub files: Vec<FileChange>,
    pub additions: u32,
    pub deletions: u32,
    pub patch: String,
    pub truncated: bool,
}
