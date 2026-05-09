use anyhow::{Context, bail};
use tokio::process::Command;
use tracing::info;

/// GitHub API client using `curl` to avoid extra dependencies.
pub struct GitHubClient {
    token: String,
}

impl GitHubClient {
    pub fn new(token: String) -> Self {
        Self { token }
    }

    /// Create a repository on GitHub and return its SSH URL.
    /// If `org` is Some, creates under that organization; otherwise creates for the authenticated user.
    pub async fn create_repo(
        &self,
        name: &str,
        org: Option<&str>,
        private: bool,
    ) -> anyhow::Result<String> {
        let url = match org {
            Some(o) => format!("https://api.github.com/orgs/{o}/repos"),
            None => "https://api.github.com/user/repos".to_string(),
        };

        let payload = serde_json::json!({
            "name": name,
            "private": private,
            "auto_init": false,
        });

        let output = Command::new("curl")
            .args([
                "-s",
                "-X",
                "POST",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "Content-Type: application/json",
                "-d",
                &payload.to_string(),
                &url,
            ])
            .output()
            .await
            .context("Failed to run curl for GitHub API")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("curl failed: {stderr}");
        }

        let body = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse GitHub API response")?;

        if let Some(ssh_url) = json.get("ssh_url").and_then(|v| v.as_str()) {
            info!(name, ssh_url, "GitHub repository created");
            Ok(ssh_url.to_string())
        } else if let Some(message) = json.get("message").and_then(|v| v.as_str()) {
            bail!("GitHub API error: {message}");
        } else {
            bail!("Unexpected GitHub API response: {body}");
        }
    }

    /// Check if a repository exists on GitHub.
    pub async fn repo_exists(&self, owner: &str, name: &str) -> anyhow::Result<bool> {
        let url = format!("https://api.github.com/repos/{owner}/{name}");

        let output = Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "-H",
                &format!("Authorization: Bearer {}", self.token),
                "-H",
                "Accept: application/vnd.github+json",
                &url,
            ])
            .output()
            .await
            .context("Failed to run curl for GitHub API")?;

        let status_code = String::from_utf8_lossy(&output.stdout).trim().to_string();

        match status_code.as_str() {
            "200" => Ok(true),
            "404" => Ok(false),
            other => bail!("Unexpected HTTP status from GitHub: {other}"),
        }
    }
}
