use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::SurveillanceEvent;
use crate::findings::FindingsStore;
use crate::gitutil;
use crate::memory::MemoryStore;

const POLL_INTERVAL: Duration = Duration::from_secs(300);

/// Polls each app's working repo for commits matching
/// `fix(surveillance:<id>): ...` and auto-resolves the referenced finding.
/// Best-effort: a missing repo or git binary just yields no matches.
pub struct GitWatcher {
    apps_src_root: PathBuf,
    slugs: Vec<String>,
    findings: FindingsStore,
    memory: MemoryStore,
    tx: broadcast::Sender<SurveillanceEvent>,
}

impl GitWatcher {
    pub fn new(
        apps_src_root: PathBuf,
        slugs: Vec<String>,
        findings: FindingsStore,
        memory: MemoryStore,
        tx: broadcast::Sender<SurveillanceEvent>,
    ) -> Self {
        Self {
            apps_src_root,
            slugs,
            findings,
            memory,
            tx,
        }
    }

    pub async fn run_loop(self) {
        // On boot, look back 1h to catch commits made while Atelier was down.
        let mut since = Utc::now() - chrono::Duration::hours(1);
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        loop {
            ticker.tick().await;
            let window_start = since;
            since = Utc::now();
            for slug in &self.slugs {
                let src = self.apps_src_root.join(slug).join("src");
                let commits = gitutil::log_since(&src, &window_start.to_rfc3339()).await;
                for (sha, subject) in commits {
                    let Some(fid) = gitutil::parse_surveillance_ref(&subject) else {
                        continue;
                    };
                    self.resolve(slug, fid, &sha, &subject).await;
                }
            }
        }
    }

    async fn resolve(&self, slug: &str, fid: i64, sha: &str, subject: &str) {
        // Ownership check: the finding must belong to this app.
        match self.findings.get(fid).await {
            Ok(Some(f)) if f.slug == slug => {}
            Ok(Some(_)) => {
                warn!(slug = %slug, finding = fid, "commit references finding of another app — skip");
                return;
            }
            Ok(None) => {
                warn!(slug = %slug, finding = fid, "commit references unknown finding — skip");
                return;
            }
            Err(e) => {
                warn!(slug = %slug, finding = fid, ?e, "git_watcher: finding lookup failed");
                return;
            }
        }
        match self.findings.resolve(fid, Some(sha)).await {
            Ok(true) => {
                info!(slug = %slug, finding = fid, sha = %&sha[..sha.len().min(8)], "auto-resolved finding from commit");
                let value = serde_json::json!({
                    "finding_id": fid,
                    "commit_sha": sha,
                    "subject": subject,
                    "completed_at": Utc::now(),
                });
                let key = format!("finding:{fid}");
                let _ = self
                    .memory
                    .upsert(slug, "applied_fix", &key, &value, None)
                    .await;
                let _ = self.tx.send(SurveillanceEvent {
                    kind: "finding".to_string(),
                    slug: slug.to_string(),
                    action: "resolve".to_string(),
                });
            }
            Ok(false) => {} // already resolved — idempotent
            Err(e) => warn!(slug = %slug, finding = fid, ?e, "git_watcher: resolve failed"),
        }
    }
}
