use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use chrono::DateTime;
use tokio::process::Command;
use tracing::{error, info, warn};

use crate::github::GitHubClient;
use crate::types::{
    BranchInfo, CommitActivityBucket, CommitDetail, CommitInfo, FileChange, FileStatus, GitConfig,
    MirrorConfig, RepoInfo, RepoVisibility, SshKeyInfo,
};

/// Borne de taille d'un patch renvoyé par `get_commit_detail` (au-delà → tronqué).
const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

pub struct GitService {
    repos_dir: PathBuf,
}

impl GitService {
    /// Build a service rooted at a custom bare-repos directory.
    /// Atelier passes `ATELIER_GIT_REPOS_DIR` (défaut `/var/lib/atelier/git/repos`).
    pub fn with_repos_dir(repos_dir: impl Into<PathBuf>) -> Self {
        Self {
            repos_dir: repos_dir.into(),
        }
    }

    pub fn repos_dir(&self) -> &Path {
        &self.repos_dir
    }

    /// Directory holding `repos/`, `ssh/` and `config.json` — derived from the
    /// configured repos dir's parent so the SSH key + config follow wherever
    /// the repos live (set via `ATELIER_GIT_REPOS_DIR`). Falls back to the
    /// repos dir itself if it has no parent.
    fn git_root(&self) -> PathBuf {
        self.repos_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.repos_dir.clone())
    }

    fn ssh_dir(&self) -> PathBuf {
        self.git_root().join("ssh")
    }

    fn ssh_key_path(&self) -> PathBuf {
        self.ssh_dir().join("id_ed25519")
    }

    fn ssh_pub_key_path(&self) -> PathBuf {
        self.ssh_dir().join("id_ed25519.pub")
    }

    fn config_path(&self) -> PathBuf {
        self.git_root().join("config.json")
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

        let limit = limit.clamp(1, 500);

        // Séparateurs de contrôle : RS (0x1e) préfixe chaque commit, US (0x1f)
        // sépare les champs du header. Ces octets ne peuvent pas apparaître
        // dans un sujet/auteur git → parsing déterministe. `--numstat` ajoute,
        // après le header, une ligne `add\tdel\tpath` par fichier modifié.
        let output = Command::new("git")
            .args([
                "log",
                &format!("-{limit}"),
                "--numstat",
                "--no-color",
                "--format=%x1ecommit%x1f%H%x1f%an%x1f%ae%x1f%aI%x1f%s",
            ])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git log")?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_commit_log(&stdout))
    }

    /// Timeline de commits agrégée par jour (calendrier de contributions).
    /// Bucket sur la **date du committer** (`%cd`) dans le fuseau local
    /// enregistré du commit (`--date=short`, pas `short-local`) — c'est ce
    /// qu'affiche `git log` et ce qu'attend un heatmap GitHub-like. Renvoie un
    /// vecteur épars trié (jours sans commit omis ; le front remplit la grille).
    pub async fn get_commit_activity(
        &self,
        slug: &str,
        days: u32,
    ) -> anyhow::Result<Vec<CommitActivityBucket>> {
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }
        if !self.has_commits(&repo_path).await {
            return Ok(Vec::new());
        }

        let output = Command::new("git")
            .args([
                "log",
                &format!("--since={days}.days.ago"),
                "--date=short",
                "--pretty=%cd",
                "--no-color",
            ])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git log for activity")?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        for line in stdout.lines() {
            let date = line.trim();
            // "YYYY-MM-DD" attendu ; on ignore toute ligne mal formée.
            if date.len() != 10 || !date.as_bytes().iter().enumerate().all(valid_date_byte) {
                continue;
            }
            *counts.entry(date.to_string()).or_insert(0) += 1;
        }

        Ok(counts
            .into_iter()
            .map(|(date, count)| CommitActivityBucket { date, count })
            .collect())
    }

    /// Détail complet d'un commit : métadonnées, fichiers modifiés (status +
    /// add/del) et diff unifié brut (capé à `MAX_PATCH_BYTES`). `sha` est
    /// validé hex-only en amont (anti-injection d'argument git).
    pub async fn get_commit_detail(&self, slug: &str, sha: &str) -> anyhow::Result<CommitDetail> {
        validate_sha(sha)?;
        let repo_path = self.repo_path(slug);
        if !repo_path.exists() {
            bail!("Repository '{slug}' not found");
        }

        // --- Passe 1 : métadonnées (sans patch). 10 champs séparés par US. ---
        let meta_fmt =
            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%s%x1f%b";
        let out = Command::new("git")
            .args(["show", "--no-color", "--no-patch", meta_fmt, sha])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git show (metadata)")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("commit '{sha}' not found: {stderr}");
        }
        let meta = String::from_utf8_lossy(&out.stdout);
        let parts: Vec<&str> = meta.splitn(10, '\x1f').collect();
        if parts.len() < 10 {
            bail!("Malformed git show metadata");
        }
        let author_date = parse_git_date(parts[4]);
        let committer_date = parse_git_date(parts[7]);
        let parents: Vec<String> = parts[1]
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        // --- Passe 2 : fichiers. name-status (status + chemins) et numstat
        // (add/del) sortent un enregistrement par fichier dans le même ordre →
        // on les zippe positionnellement. ---
        let name_out = Command::new("git")
            .args(["show", "--no-color", "--format=", "--name-status", sha])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git show (name-status)")?;
        let num_out = Command::new("git")
            .args(["show", "--no-color", "--format=", "--numstat", sha])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git show (numstat)")?;

        let name_str = String::from_utf8_lossy(&name_out.stdout);
        let num_str = String::from_utf8_lossy(&num_out.stdout);
        let stats = parse_numstat_lines(&num_str);
        let files: Vec<FileChange> = parse_name_status(&name_str)
            .into_iter()
            .enumerate()
            .map(|(i, (status, path, old_path))| {
                let (additions, deletions) = stats.get(i).copied().unwrap_or((0, 0));
                FileChange {
                    path,
                    old_path,
                    status,
                    additions,
                    deletions,
                }
            })
            .collect();
        let additions: u32 = files.iter().map(|f| f.additions).sum();
        let deletions: u32 = files.iter().map(|f| f.deletions).sum();

        // --- Passe 3 : patch unifié (capé). ---
        let patch_out = Command::new("git")
            .args(["show", "--no-color", "--format=", "-p", sha])
            .current_dir(&repo_path)
            .output()
            .await
            .context("Failed to run git show (patch)")?;
        let patch_full = String::from_utf8_lossy(&patch_out.stdout);
        let (patch, truncated) = if patch_full.len() > MAX_PATCH_BYTES {
            let mut end = MAX_PATCH_BYTES;
            while end > 0 && !patch_full.is_char_boundary(end) {
                end -= 1;
            }
            (patch_full[..end].to_string(), true)
        } else {
            (patch_full.into_owned(), false)
        };

        Ok(CommitDetail {
            hash: parts[0].to_string(),
            author_name: parts[2].to_string(),
            author_email: parts[3].to_string(),
            author_date,
            committer_name: parts[5].to_string(),
            committer_email: parts[6].to_string(),
            committer_date,
            parents,
            subject: parts[8].to_string(),
            body: parts[9].trim_end().to_string(),
            files,
            additions,
            deletions,
            patch,
            truncated,
        })
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
        let ssh_dir = self.ssh_dir();
        let key_path = self.ssh_key_path();
        let pub_key_path = self.ssh_pub_key_path();

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
        tokio::fs::create_dir_all(&ssh_dir)
            .await
            .context("Failed to create SSH directory")?;

        // Generate ED25519 key
        let output = Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-C", "atelier-git", "-f"])
            .arg(&key_path)
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
        let pub_key_path = self.ssh_pub_key_path();

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
        let config_path = self.config_path();
        if !config_path.exists() {
            return Ok(GitConfig::default());
        }

        let contents = tokio::fs::read_to_string(&config_path)
            .await
            .context("Failed to read git config")?;

        let config: GitConfig =
            serde_json::from_str(&contents).context("Failed to parse git config")?;

        Ok(config)
    }

    pub async fn save_config(&self, config: &GitConfig) -> anyhow::Result<()> {
        let config_path = self.config_path();

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let json =
            serde_json::to_string_pretty(config).context("Failed to serialize git config")?;

        // Atomic write: write to tmp then rename
        let tmp_path = PathBuf::from(format!("{}.tmp", config_path.display()));
        tokio::fs::write(&tmp_path, json.as_bytes())
            .await
            .context("Failed to write tmp config")?;
        tokio::fs::rename(&tmp_path, &config_path)
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
        let ssh_key = self.ssh_key_path();
        let hook_script = format!(
            r#"#!/bin/bash
# Async mirror push — does not block the local git push
nohup bash -c 'GIT_SSH_COMMAND="ssh -i {key} -o StrictHostKeyChecking=no" git push --mirror github' &>/dev/null &
"#,
            key = ssh_key.display()
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
                format!(
                    "ssh -i {} -o StrictHostKeyChecking=no",
                    self.ssh_key_path().display()
                ),
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
    curl -s -X POST http://127.0.0.1:4100/api/hooks/git-push \
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

/// Valide un SHA git pour usage en argument de commande : hex-only, 4..=40.
/// Bloque toute injection d'option (`--upload-pack=…`), ref, `..`, traversal.
fn validate_sha(sha: &str) -> anyhow::Result<()> {
    let ok = (4..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit());
    if !ok {
        bail!("invalid commit sha");
    }
    Ok(())
}

/// Octet valide à la position `i` d'une date "YYYY-MM-DD" (digits + tirets en 4 et 7).
fn valid_date_byte((i, b): (usize, &u8)) -> bool {
    if i == 4 || i == 7 {
        *b == b'-'
    } else {
        b.is_ascii_digit()
    }
}

fn parse_git_date(s: &str) -> DateTime<chrono::Utc> {
    DateTime::parse_from_rfc3339(s.trim())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
}

/// Parse la sortie de
/// `git log --numstat --format=%x1ecommit%x1f%H%x1f%an%x1f%ae%x1f%aI%x1f%s`.
/// Chaque commit est un chunk préfixé par RS+`commit`+US ; le header (jusqu'au
/// 1er `\n`) porte les champs séparés par US, suivi des lignes numstat.
fn parse_commit_log(stdout: &str) -> Vec<CommitInfo> {
    let mut commits = Vec::new();
    for chunk in stdout.split('\x1e') {
        // Le chunk initial (avant le 1er RS) est vide → strip_prefix échoue → skip.
        let chunk = match chunk.strip_prefix("commit\x1f") {
            Some(c) => c,
            None => continue,
        };
        let (header, rest) = chunk.split_once('\n').unwrap_or((chunk, ""));
        let parts: Vec<&str> = header.splitn(5, '\x1f').collect();
        if parts.len() < 5 {
            warn!(header, "Malformed git log header");
            continue;
        }
        let date = parse_git_date(parts[3]);

        let mut additions = 0u32;
        let mut deletions = 0u32;
        let mut files_changed = 0u32;
        for (add, del) in parse_numstat_lines(rest) {
            additions += add;
            deletions += del;
            files_changed += 1;
        }

        commits.push(CommitInfo {
            hash: parts[0].to_string(),
            author_name: parts[1].to_string(),
            author_email: parts[2].to_string(),
            date,
            message: parts[4].to_string(),
            additions,
            deletions,
            files_changed,
        });
    }
    commits
}

/// Parse des lignes numstat `add\tdel\tpath` (une par fichier modifié, dans
/// l'ordre du diff). Les fichiers binaires affichent `-\t-` → 0/0. Le chemin
/// est ignoré (seules les stats comptent ici).
fn parse_numstat_lines(s: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for line in s.lines() {
        if line.is_empty() {
            continue;
        }
        let mut cols = line.splitn(3, '\t');
        let add = cols.next().unwrap_or("");
        let del = cols.next().unwrap_or("");
        // Pas de 3e colonne (chemin) → ce n'est pas une ligne numstat.
        if cols.next().is_none() {
            continue;
        }
        out.push((add.parse::<u32>().unwrap_or(0), del.parse::<u32>().unwrap_or(0)));
    }
    out
}

/// Parse `git show --name-status` → `(status, new_path, old_path?)` par fichier.
/// Renommages/copies : `R100\told\tnew` → status R, path=new, old_path=Some(old).
fn parse_name_status(s: &str) -> Vec<(FileStatus, String, Option<String>)> {
    let mut out = Vec::new();
    for line in s.lines() {
        if line.is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let code = cols.next().unwrap_or("");
        let first = code.chars().next().unwrap_or('X');
        let status = FileStatus::from_letter(first);
        let (path, old_path) = if matches!(first, 'R' | 'C') {
            let old = cols.next().unwrap_or("").to_string();
            let new = cols.next().unwrap_or("").to_string();
            (new, Some(old))
        } else {
            (cols.next().unwrap_or("").to_string(), None)
        };
        if path.is_empty() {
            continue;
        }
        out.push((status, path, old_path));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RS: char = '\x1e';
    const US: char = '\x1f';

    fn log_block(hash: &str, subject: &str, numstat: &[&str]) -> String {
        let header = format!(
            "{RS}commit{US}{hash}{US}Romain{US}r@x{US}2026-06-01T10:00:00+02:00{US}{subject}\n"
        );
        let body = numstat
            .iter()
            .map(|l| format!("{l}\n"))
            .collect::<String>();
        // git insère une ligne vide entre le header et le bloc numstat.
        format!("{header}\n{body}")
    }

    #[test]
    fn parses_basic_commit_with_stats() {
        let log = log_block("abc123", "feat: x", &["10\t2\tsrc/a.rs", "0\t5\tsrc/b.rs"]);
        let commits = parse_commit_log(&log);
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!(c.hash, "abc123");
        assert_eq!(c.message, "feat: x");
        assert_eq!(c.additions, 10);
        assert_eq!(c.deletions, 7);
        assert_eq!(c.files_changed, 2);
    }

    #[test]
    fn binary_files_count_but_dont_add() {
        let log = log_block("def456", "bin", &["-\t-\timg.png", "3\t1\tnote.txt"]);
        let commits = parse_commit_log(&log);
        let c = &commits[0];
        assert_eq!(c.additions, 3);
        assert_eq!(c.deletions, 1);
        assert_eq!(c.files_changed, 2); // le binaire compte comme fichier changé
    }

    #[test]
    fn empty_or_merge_commit_has_zero_stats() {
        // Pas de ligne numstat (merge / empty commit).
        let log = log_block("merge0", "Merge branch 'x'", &[]);
        let commits = parse_commit_log(&log);
        let c = &commits[0];
        assert_eq!((c.additions, c.deletions, c.files_changed), (0, 0, 0));
    }

    #[test]
    fn parses_multiple_commits() {
        let mut log = log_block("c1", "first", &["1\t0\ta"]);
        log.push_str(&log_block("c2", "second", &["2\t2\tb", "4\t0\tc"]));
        let commits = parse_commit_log(&log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "c1");
        assert_eq!(commits[0].files_changed, 1);
        assert_eq!(commits[1].additions, 6);
        assert_eq!(commits[1].files_changed, 2);
    }

    #[test]
    fn name_status_handles_rename() {
        let parsed = parse_name_status("R100\told/path.rs\tnew/path.rs\nM\tsrc/x.rs\n");
        assert_eq!(parsed.len(), 2);
        assert!(matches!(parsed[0].0, FileStatus::R));
        assert_eq!(parsed[0].1, "new/path.rs");
        assert_eq!(parsed[0].2.as_deref(), Some("old/path.rs"));
        assert!(matches!(parsed[1].0, FileStatus::M));
        assert_eq!(parsed[1].1, "src/x.rs");
        assert_eq!(parsed[1].2, None);
    }

    #[test]
    fn numstat_lines_skip_blanks_and_binaries() {
        let stats = parse_numstat_lines("\n10\t2\ta.rs\n-\t-\tb.png\n");
        assert_eq!(stats, vec![(10, 2), (0, 0)]);
    }

    #[test]
    fn validate_sha_accepts_hex_rejects_injection() {
        assert!(validate_sha("abc123").is_ok());
        assert!(validate_sha("0123456789abcdef0123456789abcdef01234567").is_ok());
        assert!(validate_sha("HEAD").is_err());
        assert!(validate_sha("--upload-pack=evil").is_err());
        assert!(validate_sha("../etc").is_err());
        assert!(validate_sha("abc").is_err()); // trop court
        assert!(validate_sha("").is_err());
    }
}
