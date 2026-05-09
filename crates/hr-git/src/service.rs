use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use chrono::DateTime;
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::github::GitHubClient;
use crate::types::{
    BranchInfo, CommitInfo, GitConfig, MirrorConfig, RepoInfo, RepoVisibility, SshKeyInfo,
};

const DEFAULT_REPOS_DIR: &str = "/opt/homeroute/data/git/repos";
const SSH_DIR: &str = "/opt/homeroute/data/git/ssh";
const SSH_KEY_PATH: &str = "/opt/homeroute/data/git/ssh/id_ed25519";
const CONFIG_PATH: &str = "/opt/homeroute/data/git/config.json";

pub struct GitService {
    repos_dir: PathBuf,
}

impl GitService {
    pub fn new() -> Self {
        Self {
            repos_dir: PathBuf::from(DEFAULT_REPOS_DIR),
        }
    }

    /// Build a service rooted at a custom bare-repos directory.
    /// Used by Atelier (CloudMaster) which reads a rsync'd mirror of the
    /// homeroute repos under /var/lib/atelier/git/repos.
    pub fn with_repos_dir(repos_dir: impl Into<PathBuf>) -> Self {
        Self {
            repos_dir: repos_dir.into(),
        }
    }

    pub fn repos_dir(&self) -> &Path {
        &self.repos_dir
    }

    pub async fn init(&self) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.repos_dir)
            .await
            .context("Failed to create repos directory")?;
        info!(path = %self.repos_dir.display(), "Git repos directory initialized");
        Ok(())
    }

    pub async fn create_repo(&self, slug: &str) -> anyhow::Result<PathBuf> {
        let repo_path = self.repo_path(slug);
        if repo_path.exists() {
            info!(slug, "Repository already exists");
            return Ok(repo_path);
        }

        let output = Command::new("git")
            .args(["init", "--bare"])
            .arg(&repo_path)
            .output()
            .await
            .context("Failed to run git init --bare")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git init --bare failed: {stderr}");
        }

        // Enable HTTP push
        let output = Command::new("git")
            .args(["config", "http.receivepack", "true"])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git config")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git config http.receivepack failed: {stderr}");
        }

        info!(slug, path = %repo_path.display(), "Repository created");

        // Auto-enable mirror if GitHub config is set
        if let Err(e) = self.auto_mirror_new_repo(slug).await {
            warn!(slug, error = %e, "Failed to auto-enable mirror for new repo");
        }

        Ok(repo_path)
    }

    pub async fn delete_repo(&self, slug: &str) -> anyhow::Result<()> {
        let repo_path = self.repo_path(slug);
        if repo_path.exists() {
            tokio::fs::remove_dir_all(&repo_path)
                .await
                .context("Failed to delete repository")?;
            info!(slug, "Repository deleted");
        }
        Ok(())
    }

    pub async fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>> {
        let mut repos = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.repos_dir)
            .await
            .context("Failed to read repos directory")?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if path.is_dir() && name.ends_with(".git") {
                let slug = name.trim_end_matches(".git").to_string();
                match self.build_repo_info(&slug, &path).await {
                    Ok(info) => repos.push(info),
                    Err(e) => warn!(slug, error = %e, "Failed to get repo info"),
                }
            }
        }

        repos.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(repos)
    }

    pub async fn get_repo(&self, slug: &str) -> anyhow::Result<Option<RepoInfo>> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            return Ok(None);
        }
        let info = self.build_repo_info(slug, &repo_path).await?;
        Ok(Some(info))
    }

    pub async fn get_commits(&self, slug: &str, limit: usize) -> anyhow::Result<Vec<CommitInfo>> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        // Check if repo has any commits
        if !self.has_commits(&repo_path).await {
            return Ok(Vec::new());
        }

        let output = Command::new("git")
            .args([
                "log",
                &format!("--format=%H|%an|%ae|%aI|%s"),
                &format!("-{limit}"),
            ])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git log")?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut commits = Vec::new();

        for line in stdout.lines() {
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() < 5 {
                warn!(line, "Malformed git log line");
                continue;
            }

            let date = DateTime::parse_from_rfc3339(parts[3])
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());

            commits.push(CommitInfo {
                hash: parts[0].to_string(),
                author_name: parts[1].to_string(),
                author_email: parts[2].to_string(),
                date,
                message: parts[4].to_string(),
            });
        }

        Ok(commits)
    }

    pub async fn get_branches(&self, slug: &str) -> anyhow::Result<Vec<BranchInfo>> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        let output = Command::new("git")
            .args(["branch"])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git branch")?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut branches = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let is_head = line.starts_with("* ");
            let name = line.trim_start_matches("* ").to_string();
            branches.push(BranchInfo { name, is_head });
        }

        Ok(branches)
    }

    pub fn repo_path(&self, slug: &str) -> PathBuf {
        self.repos_dir.join(format!("{slug}.git"))
    }

    pub fn repo_exists(&self, slug: &str) -> bool {
        self.repo_path(slug).exists()
    }

    // --- Phase 3: Mirror / SSH methods ---

    pub async fn generate_ssh_key(&self) -> anyhow::Result<SshKeyInfo> {
        let ssh_dir = Path::new(SSH_DIR);
        let pub_key_path = PathBuf::from(format!("{SSH_KEY_PATH}.pub"));

        // If key already exists, just return it
        if pub_key_path.exists() {
            let public_key = tokio::fs::read_to_string(&pub_key_path)
                .await
                .context("Failed to read existing SSH public key")?;
            return Ok(SshKeyInfo {
                public_key: public_key.trim().to_string(),
                exists: true,
            });
        }

        // Create SSH directory
        tokio::fs::create_dir_all(ssh_dir)
            .await
            .context("Failed to create SSH directory")?;

        // Generate ED25519 key
        let output = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                SSH_KEY_PATH,
                "-N",
                "",
                "-C",
                "homeroute-git",
            ])
            .output()
            .await
            .context("Failed to run ssh-keygen")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("ssh-keygen failed: {stderr}");
        }

        let public_key = tokio::fs::read_to_string(&pub_key_path)
            .await
            .context("Failed to read generated SSH public key")?;

        info!("SSH key generated for git mirror");

        Ok(SshKeyInfo {
            public_key: public_key.trim().to_string(),
            exists: true,
        })
    }

    pub async fn get_ssh_key(&self) -> anyhow::Result<SshKeyInfo> {
        let pub_key_path = PathBuf::from(format!("{SSH_KEY_PATH}.pub"));

        if !pub_key_path.exists() {
            return Ok(SshKeyInfo {
                public_key: String::new(),
                exists: false,
            });
        }

        let public_key = tokio::fs::read_to_string(&pub_key_path)
            .await
            .context("Failed to read SSH public key")?;

        Ok(SshKeyInfo {
            public_key: public_key.trim().to_string(),
            exists: true,
        })
    }

    pub async fn load_config(&self) -> anyhow::Result<GitConfig> {
        let config_path = Path::new(CONFIG_PATH);
        if !config_path.exists() {
            return Ok(GitConfig::default());
        }

        let contents = tokio::fs::read_to_string(config_path)
            .await
            .context("Failed to read git config")?;

        let config: GitConfig =
            serde_json::from_str(&contents).context("Failed to parse git config")?;

        Ok(config)
    }

    pub async fn save_config(&self, config: &GitConfig) -> anyhow::Result<()> {
        let config_path = Path::new(CONFIG_PATH);

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let json =
            serde_json::to_string_pretty(config).context("Failed to serialize git config")?;

        // Atomic write: write to tmp then rename
        let tmp_path = PathBuf::from(format!("{CONFIG_PATH}.tmp"));
        tokio::fs::write(&tmp_path, json.as_bytes())
            .await
            .context("Failed to write tmp config")?;
        tokio::fs::rename(&tmp_path, config_path)
            .await
            .context("Failed to rename tmp config")?;

        Ok(())
    }

    pub async fn enable_mirror(&self, slug: &str, org: &str) -> anyhow::Result<()> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        let ssh_url = format!("git@github.com:{}/{}.git", org, slug);

        // Add remote "github" (remove first if exists)
        let _ = Command::new("git")
            .args(["remote", "remove", "github"])
            .current_dir(&repo_path)
            .output()
            .await;

        let output = Command::new("git")
            .args(["remote", "add", "github", &ssh_url])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to add github remote")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to add github remote: {stderr}");
        }

        // Write post-receive hook
        let hooks_dir = repo_path.join("hooks");
        tokio::fs::create_dir_all(&hooks_dir).await?;

        let hook_path = hooks_dir.join("post-receive");
        let hook_script = format!(
            r#"#!/bin/bash
# Async mirror push — does not block the local git push
nohup bash -c 'GIT_SSH_COMMAND="ssh -i {SSH_KEY_PATH} -o StrictHostKeyChecking=no" git push --mirror github' &>/dev/null &
"#
        );

        tokio::fs::write(&hook_path, hook_script.as_bytes())
            .await
            .context("Failed to write post-receive hook")?;

        // chmod +x
        let output = Command::new("chmod")
            .args(["+x"])
            .arg(&hook_path)
            .output()
            .await
            .context("Failed to chmod hook")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("chmod +x failed: {stderr}");
        }

        info!(slug, ssh_url = %ssh_url, "Mirror enabled with post-receive hook");
        Ok(())
    }

    pub async fn disable_mirror(&self, slug: &str) -> anyhow::Result<()> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        // Remove remote
        let _ = Command::new("git")
            .args(["remote", "remove", "github"])
            .current_dir(&repo_path)
            .output()
            .await;

        // Remove hook
        let hook_path = repo_path.join("hooks/post-receive");
        if hook_path.exists() {
            tokio::fs::remove_file(&hook_path).await?;
        }

        info!(slug, "Mirror disabled");
        Ok(())
    }

    pub async fn trigger_sync(&self, slug: &str) -> anyhow::Result<()> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        let output = Command::new("git")
            .args(["push", "--mirror", "github"])
            .env(
                "GIT_SSH_COMMAND",
                format!("ssh -i {SSH_KEY_PATH} -o StrictHostKeyChecking=no"),
            )
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to push mirror")?;

        // Persist last_sync / last_error in config
        let mut config = self.load_config().await.unwrap_or_default();
        if let Some(mirror) = config.mirrors.get_mut(slug) {
            if output.status.success() {
                mirror.last_sync = Some(chrono::Utc::now());
                mirror.last_error = None;
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                mirror.last_error = Some(stderr.clone());
            }
            let _ = self.save_config(&config).await;
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(slug, stderr = %stderr, "Mirror sync failed");
            bail!("Mirror sync failed: {stderr}");
        }

        info!(slug, "Mirror sync completed");
        Ok(())
    }

    /// Auto-enable mirror for a newly created repo if GitHub org+token are configured.
    pub async fn auto_mirror_new_repo(&self, slug: &str) -> anyhow::Result<()> {
        let mut config = self.load_config().await?;

        let token = match config.github_token.as_ref() {
            Some(t) if !t.is_empty() => t.clone(),
            _ => return Ok(()), // No token configured, skip
        };

        let org = &config.github_org;
        if org.is_empty() {
            return Ok(()); // No org configured, skip
        }

        let gh = GitHubClient::new(token);

        // Create GitHub repo if it doesn't exist
        match gh.repo_exists(org, slug).await? {
            false => {
                gh.create_repo(slug, Some(org), true).await?;
            }
            true => {}
        }

        // Enable mirror in the bare repo
        self.enable_mirror(slug, org).await?;

        // Save mirror config
        let ssh_url = format!("git@github.com:{org}/{slug}.git");
        config.mirrors.insert(
            slug.to_string(),
            MirrorConfig {
                enabled: true,
                github_ssh_url: Some(ssh_url),
                visibility: RepoVisibility::Private,
                last_sync: None,
                last_error: None,
            },
        );

        self.save_config(&config).await?;
        info!(slug, "Auto-enabled mirror for new repo");
        Ok(())
    }

    // --- Pipeline hook ---

    /// Set up a post-receive hook that triggers pipeline on push to main/master.
    pub async fn setup_pipeline_hook(&self, slug: &str) -> anyhow::Result<()> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        let hooks_dir = repo_path.join("hooks");
        tokio::fs::create_dir_all(&hooks_dir).await?;
        let hook_path = hooks_dir.join("post-receive");

        // Pipeline trigger snippet
        let pipeline_snippet = format!(
            r#"
# Pipeline trigger — notify orchestrator on push to main/master
while read oldrev newrev refname; do
  if [ "$refname" = "refs/heads/main" ] || [ "$refname" = "refs/heads/master" ]; then
    curl -s -X POST http://10.0.0.254:4001/hooks/git-push \
      -H "Content-Type: application/json" \
      -d "{{\\"slug\\":\\"{slug}\\",\\"ref\\":\\"$refname\\",\\"commit\\":\\"$newrev\\"}}" &>/dev/null &
  fi
done
"#
        );

        if hook_path.exists() {
            // Append to existing hook (don't overwrite mirror hook)
            let existing = tokio::fs::read_to_string(&hook_path).await?;
            if existing.contains("hooks/git-push") {
                info!(slug, "Pipeline hook already present");
                return Ok(());
            }
            let updated = format!("{}\n{}", existing.trim(), pipeline_snippet);
            tokio::fs::write(&hook_path, updated.as_bytes()).await?;
        } else {
            // Create new hook
            let script = format!("#!/bin/bash\n{}", pipeline_snippet);
            tokio::fs::write(&hook_path, script.as_bytes()).await?;
        }

        // chmod +x
        let output = Command::new("chmod")
            .args(["+x"])
            .arg(&hook_path)
            .output()
            .await
            .context("Failed to chmod pipeline hook")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("chmod +x failed: {stderr}");
        }

        info!(slug, "Pipeline hook configured");
        Ok(())
    }

    // --- Private helpers ---

    async fn has_commits(&self, repo_path: &Path) -> bool {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo_path)
            .output()
            .await;

        matches!(output, Ok(o) if o.status.success())
    }

    async fn build_repo_info(&self, slug: &str, repo_path: &Path) -> anyhow::Result<RepoInfo> {
        let has_commits = self.has_commits(repo_path).await;

        // Get directory size
        let size_bytes = dir_size(repo_path).await.unwrap_or(0);

        // Get HEAD ref
        let head_ref = if has_commits {
            let output = Command::new("git")
                .args(["symbolic-ref", "--short", "HEAD"])
                .current_dir(repo_path)
                .output()
                .await;

            output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else {
            None
        };

        // Get commit count
        let commit_count = if has_commits {
            let output = Command::new("git")
                .args(["rev-list", "--count", "HEAD"])
                .current_dir(repo_path)
                .output()
                .await;

            output
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
                .unwrap_or(0)
        } else {
            0
        };

        // Get last commit date
        let last_commit = if has_commits {
            let output = Command::new("git")
                .args(["log", "-1", "--format=%aI"])
                .current_dir(repo_path)
                .output()
                .await;

            output.ok().filter(|o| o.status.success()).and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                DateTime::parse_from_rfc3339(&s)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .ok()
            })
        } else {
            None
        };

        // Get branches
        let branches = if has_commits {
            let output = Command::new("git")
                .args(["branch", "--format=%(refname:short)"])
                .current_dir(repo_path)
                .output()
                .await;

            output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(|l| l.to_string())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok(RepoInfo {
            slug: slug.to_string(),
            size_bytes,
            head_ref,
            commit_count,
            last_commit,
            branches,
        })
    }
}

/// Recursively compute directory size in bytes.
async fn dir_size(path: &Path) -> anyhow::Result<u64> {
    let output = Command::new("du").args(["-sb"]).arg(path).output().await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(size_str) = stdout.split_whitespace().next() {
            return Ok(size_str.parse::<u64>().unwrap_or(0));
        }
    }

    Ok(0)
}
