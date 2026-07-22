use std::path::Path;
use tokio::process::Command;
use tracing::info;

async fn git(user: &str, cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    anyhow::ensure!(cwd.is_dir(), "working tree absent: {}", cwd.display());
    let mut cmd = if user.is_empty() {
        let mut c = Command::new("git");
        c.arg("-C").arg(cwd);
        c
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n")
            .arg("-H")
            .arg("-u")
            .arg(user)
            .arg("--")
            .arg("git")
            .arg("-C")
            .arg(cwd);
        c
    };
    cmd.args(args);
    let out = cmd.output().await?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub async fn head_sha(user: &str, cwd: &Path) -> anyhow::Result<String> {
    git(user, cwd, &["rev-parse", "HEAD"]).await
}

pub async fn status_porcelain(user: &str, cwd: &Path) -> anyhow::Result<String> {
    git(
        user,
        cwd,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .await
}

/// Recover a successful detached Atelier commit when its HTTP report and
/// runtime marker were both lost across a restart.
pub async fn find_backlog_commit(
    user: &str,
    cwd: &Path,
    item_id: i64,
) -> anyhow::Result<Option<String>> {
    let log = git(user, cwd, &["log", "-n", "100", "--format=%H%x1f%s"]).await?;
    let marker = format!("(backlog:{item_id})");
    Ok(log.lines().find_map(|line| {
        let (sha, subject) = line.split_once('\u{1f}')?;
        if subject.contains(&marker) {
            Some(sha.to_string())
        } else {
            None
        }
    }))
}

/// Commit the pre-existing tree before autonomous work. The returned SHA is the
/// checkpoint; `None` means the tree was already clean.
pub async fn checkpoint(user: &str, cwd: &Path, scope: &str) -> anyhow::Result<Option<String>> {
    let status = status_porcelain(user, cwd).await?;
    if status.trim().is_empty() {
        return Ok(None);
    }
    git(user, cwd, &["add", "-A"]).await?;
    let body: String = status.lines().take(200).collect::<Vec<_>>().join("\n");
    let message =
        format!("chore({scope}): snapshot pré-autonome\n\nFichiers avant run Pilote :\n{body}");
    git_with_identity(
        user,
        cwd,
        &["commit", "-m", &message],
        "Romain (checkpoint)",
        "pilot-checkpoint@atelier.local",
    )
    .await?;
    Ok(Some(head_sha(user, cwd).await?))
}

pub async fn commit(user: &str, cwd: &Path, message: &str) -> anyhow::Result<String> {
    git(user, cwd, &["add", "-A"]).await?;
    git_with_identity(
        user,
        cwd,
        &["commit", "-m", message],
        "Atelier Pilote",
        "pilot@atelier.local",
    )
    .await?;
    head_sha(user, cwd).await
}

async fn git_with_identity(
    user: &str,
    cwd: &Path,
    args: &[&str],
    name: &str,
    email: &str,
) -> anyhow::Result<String> {
    anyhow::ensure!(cwd.is_dir(), "working tree absent: {}", cwd.display());
    let mut cmd = if user.is_empty() {
        let mut c = Command::new("git");
        c.arg("-C").arg(cwd);
        c
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n")
            .arg("-H")
            .arg("-u")
            .arg(user)
            .arg("--")
            .arg("git")
            .arg("-C")
            .arg(cwd);
        c
    };
    // Un agent peut avoir posé un hook via Bash pendant son run : le commit
    // orchestrateur (checkpoint comme commit final) ne doit JAMAIS l'exécuter.
    cmd.arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg(format!("user.name={name}"))
        .arg("-c")
        .arg(format!("user.email={email}"))
        .args(args);
    let out = cmd.output().await?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Push best-effort vers l'upstream configuré. Non bloquant par design : un
/// push raté n'invalide jamais un run (le commit local reste la vérité, la
/// bande « État des dépôts » montrera le retard). Timeout court : le remote
/// des apps est le bare repo local, un blocage réseau ne doit pas geler la nuit.
pub async fn push(user: &str, cwd: &Path) -> anyhow::Result<()> {
    let fut = git(user, cwd, &["push"]);
    match tokio::time::timeout(std::time::Duration::from_secs(60), fut).await {
        Ok(Ok(_)) => {
            info!(cwd = %cwd.display(), "pilot push ok");
            Ok(())
        }
        Ok(Err(e)) => anyhow::bail!(e),
        Err(_) => anyhow::bail!("git push: timeout 60 s"),
    }
}

/// Snapshot de l'état d'un dépôt pour la bande « État des dépôts » du Backlog.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoStatus {
    pub scope: String,
    /// Fichiers non committés (working tree + index + untracked).
    pub dirty: i64,
    /// Commits locaux non poussés ; `None` = pas d'upstream configuré.
    pub ahead: Option<i64>,
    pub has_upstream: bool,
    pub last_commit: Option<RepoCommit>,
    /// Dépôt illisible (absent, HEAD unborn…) — la bande l'affiche tel quel.
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoCommit {
    pub sha: String,
    pub subject: String,
    pub at: i64,
}

pub async fn repo_status(user: &str, cwd: &Path, scope: &str) -> RepoStatus {
    let mut st = RepoStatus {
        scope: scope.to_string(),
        dirty: 0,
        ahead: None,
        has_upstream: false,
        last_commit: None,
        error: None,
    };
    match status_porcelain(user, cwd).await {
        Ok(s) => st.dirty = s.lines().filter(|l| !l.trim().is_empty()).count() as i64,
        Err(e) => {
            st.error = Some(e.to_string());
            return st;
        }
    }
    // `@{u}` échoue proprement quand la branche n'a pas d'upstream (hevy au
    // premier boot) ou que HEAD est unborn — has_upstream=false, pas une erreur.
    if let Ok(n) = git(user, cwd, &["rev-list", "--count", "@{u}..HEAD"]).await {
        st.has_upstream = true;
        st.ahead = n.trim().parse::<i64>().ok();
    }
    if let Ok(line) = git(user, cwd, &["log", "-1", "--format=%h\u{1f}%s\u{1f}%ct"]).await {
        let mut parts = line.split('\u{1f}');
        if let (Some(sha), Some(subject), Some(ct)) = (parts.next(), parts.next(), parts.next()) {
            st.last_commit = Some(RepoCommit {
                sha: sha.to_string(),
                subject: subject.to_string(),
                at: ct.trim().parse().unwrap_or(0),
            });
        }
    }
    st
}

/// Deterministic rollback to the orchestrator-owned pre-agent SHA. Never call
/// before `checkpoint`: the caller guarantees all prior human work is committed.
pub async fn rollback(user: &str, cwd: &Path, sha: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "sha de rollback invalide"
    );
    info!(cwd = %cwd.display(), sha = %sha, "pilot rollback");
    git(user, cwd, &["reset", "--hard", sha]).await?;
    git(user, cwd, &["clean", "-fd"]).await?;
    Ok(())
}
