use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::SurveillanceEvent;
use crate::findings::FindingsStore;
use crate::gitutil;
use crate::memory::MemoryStore;
use crate::service::BacklogSettledSink;
use crate::sqlx::{PgPool, query};

const POLL_INTERVAL: Duration = Duration::from_secs(300);

/// Polls each app's working repo for commits matching
/// `fix(surveillance:<id>): ...` and auto-resolves the referenced finding.
/// Best-effort: a missing repo or git binary just yields no matches.
pub struct GitWatcher {
    apps_src_root: PathBuf,
    slugs: Vec<String>,
    /// Racine du dépôt source d'Atelier (scope backlog 'atelier'). Les items
    /// Pilote de ce scope se ferment aussi par un commit manuel
    /// `fix(backlog:<id>)` — même best-effort que les repos d'apps.
    atelier_src_root: Option<PathBuf>,
    findings: FindingsStore,
    memory: MemoryStore,
    tx: broadcast::Sender<SurveillanceEvent>,
    pool: PgPool,
    /// Sink optionnel appelé après une clôture backlog réussie — Atelier y
    /// branche `BacklogStore::republish` (event `pilot:backlog`) pour que le
    /// kanban se rafraîchisse en live, sans dépendance watcher → pilot.
    on_backlog_settled: Option<BacklogSettledSink>,
}

impl GitWatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        apps_src_root: PathBuf,
        slugs: Vec<String>,
        atelier_src_root: Option<PathBuf>,
        findings: FindingsStore,
        memory: MemoryStore,
        tx: broadcast::Sender<SurveillanceEvent>,
        pool: PgPool,
        on_backlog_settled: Option<BacklogSettledSink>,
    ) -> Self {
        Self {
            apps_src_root,
            slugs,
            atelier_src_root,
            findings,
            memory,
            tx,
            pool,
            on_backlog_settled,
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
                    if let Some(fid) = gitutil::parse_surveillance_ref(&subject) {
                        self.resolve(slug, fid, &sha, &subject).await;
                    }
                    if let Some(item_id) = gitutil::parse_backlog_ref(&subject) {
                        self.settle_backlog(slug, item_id, &sha).await;
                    }
                }
            }
            // Repo Atelier (scope 'atelier') : seuls les refs backlog comptent —
            // la surveillance ne scanne pas Atelier, un `fix(surveillance:N)`
            // ici référencerait forcément la finding d'une app (ownership check
            // le rejetterait avec un warn parasite), donc on ne le parse pas.
            if let Some(root) = &self.atelier_src_root {
                let commits = gitutil::log_since(root, &window_start.to_rfc3339()).await;
                for (sha, subject) in commits {
                    if let Some(item_id) = gitutil::parse_backlog_ref(&subject) {
                        self.settle_backlog("atelier", item_id, &sha).await;
                    }
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

    async fn settle_backlog(&self, slug: &str, item_id: i64, sha: &str) {
        match query(
            r#"UPDATE backlog_items SET lane='done',exec_status='done',done_at=now(),updated_at=now(),
               needs_user=false,needs_user_reason=NULL,
               commit_shas=commit_shas || jsonb_build_array($3::text)
               WHERE id=$1 AND scope=$2 AND lane<>'done' AND exec_status NOT IN ('queued','running')"#,
        )
        .bind(item_id)
        .bind(slug)
        .bind(sha)
        .execute(&self.pool)
        .await
        {
            Ok(r) if r.rows_affected() > 0 => {
                info!(slug=%slug,item_id,sha=%&sha[..sha.len().min(8)],"closed backlog item from commit");
                // Republication live : le SQL ci-dessus contourne le store Pilote
                // (pas d'event `pilot:backlog`) — le sink comble ce trou pour l'UI.
                if let Some(cb) = &self.on_backlog_settled {
                    cb(item_id);
                }
            }
            Ok(_) => {}
            Err(e) => warn!(slug=%slug,item_id,?e,"git_watcher: backlog settle failed"),
        }
    }
}
