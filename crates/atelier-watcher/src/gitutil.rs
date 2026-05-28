use std::path::Path;

use tokio::process::Command;

/// Run `git -C <dir> <args...>` and return trimmed stdout, or None on failure
/// (not a repo, git missing, non-zero exit). Surveillance treats git as
/// best-effort — a missing repo just disables diff-aware / auto-resolve for
/// that app.
async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Current HEAD commit SHA of the working repo at `src_dir`.
pub async fn head_sha(src_dir: &Path) -> Option<String> {
    git(src_dir, &["rev-parse", "HEAD"]).await
}

/// Unified diff between `from_sha` and HEAD, with 100 lines of context (so a
/// reviewer sees enough around each hunk — see plan: diff-aware blind spot).
/// Returns None if the range is invalid or empty.
pub async fn diff_since(src_dir: &Path, from_sha: &str) -> Option<String> {
    let range = format!("{from_sha}..HEAD");
    let d = git(src_dir, &["diff", "-U100", &range]).await?;
    if d.trim().is_empty() { None } else { Some(d) }
}

/// One commit per line since `since_iso`, as `SHA\x1Fsubject`. Empty vec if
/// none / not a repo.
pub async fn log_since(src_dir: &Path, since_iso: &str) -> Vec<(String, String)> {
    let fmt = "--pretty=format:%H%x1f%s";
    let since_arg = format!("--since={since_iso}");
    let Some(out) = git(src_dir, &["log", &since_arg, fmt]).await else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(2, '\u{1f}');
            let sha = parts.next()?.to_string();
            let subj = parts.next().unwrap_or("").to_string();
            if sha.is_empty() { None } else { Some((sha, subj)) }
        })
        .collect()
}

/// Extract the finding id from a `fix(surveillance:<id>): ...` commit subject.
/// Returns None if the subject doesn't follow the convention.
pub fn parse_surveillance_ref(subject: &str) -> Option<i64> {
    let key = "surveillance:";
    let idx = subject.find(key)?;
    let rest = &subject[idx + key.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}
