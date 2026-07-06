//! App-tree filesystem helpers for newly created apps.
//!
//! Historique : ce module portait le scaffolding par stack (templates
//! embarqués + defaults `run_command`/`build_command` par variante
//! `AppStack`). Supprimé lors de la généricisation des stacks (2026-07-06) :
//! la plateforme est stack-agnostique, une app naît **vide** et c'est la
//! première conversation Studio qui génère le projet et configure les
//! commandes via `app.update`. Ne reste ici que la normalisation des
//! permissions de l'arbre.
//!
//! # INVARIANT — workspace de l'app (Studio)
//!
//! `app.src_dir()` (`{slug}/src/`) est **à la fois** le dossier des sources de
//! l'app ET le workspace ouvert par le Studio (agent Claude Code). Tout fichier qui doit être
//! édité/lu par l'agent Claude Code (ex : un `README.md` initial, un
//! `.env.example`) DOIT être placé directement sous `src/`.
//!
//! La génération de `CLAUDE.md`, `.claude/`, `.mcp.json` relève de
//! [`atelier_apps::context`] (appelé par le handler AppCreate juste après la
//! création de l'arbre).

use std::path::Path;

use tracing::{info, warn};

/// Resolve the user that should OWN app-tree artefacts and RUN builds.
/// Precedence:
/// 1. `ATELIER_BUILD_AS_USER` when set and non-empty (explicit config);
/// 2. `hr-studio` when Atelier runs as **root** — a root-owned tree is
///    unbuildable/uneditable by the `hr-studio` Studio agent, so the platform
///    must never fall back to root even with the env var unset;
/// 3. `None` in a non-root dev environment — Atelier already runs as a regular
///    user, so artefacts are naturally that user's and no sudo/chown-owner is
///    required.
///
/// Shared by [`normalize_app_tree_perms`] (chown owner) and
/// `apps_ops::wrap_local_cmd` (build sudo target) so the two never drift.
pub fn build_as_user() -> Option<String> {
    if let Some(user) = std::env::var("ATELIER_BUILD_AS_USER")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return Some(user);
    }
    // SAFETY: geteuid() is always safe — no args, no global state mutation.
    if unsafe { libc::geteuid() } == 0 {
        return Some("hr-studio".to_string());
    }
    None
}

/// Normalize ownership/perms of a freshly-created app tree so the build user
/// (`ATELIER_BUILD_AS_USER`) and the agent group (`ATELIER_RULES_GROUP`,
/// default `hr-studio`) can both work in it: owner build-user, group
/// rules-group, dirs setgid + group-rwx, files group-rw. Matches the layout
/// of the historical apps (`romain:hr-studio`, dirs 2775, files 664). WHY:
/// Atelier runs as root, so everything scaffolded here is root-owned 0755 by
/// default — the Studio agent's workspace would be read-only and the first
/// build/`git init` (run as the build user) would die on Permission denied.
/// Best-effort: failures degrade to a warn, creation must not abort on a
/// perms tweak.
pub async fn normalize_app_tree_perms(dir: &Path) {
    let group = std::env::var("ATELIER_RULES_GROUP").unwrap_or_else(|_| "hr-studio".to_string());
    let chown_spec = match build_as_user() {
        Some(user) => format!("{user}:{group}"),
        None => format!(":{group}"),
    };
    let script = format!(
        "chown -R '{chown_spec}' '{d}' && chmod -R g+rwX '{d}' && find '{d}' -type d -exec chmod g+s {{}} +",
        d = dir.display()
    );
    match tokio::process::Command::new("sh").arg("-c").arg(&script).output().await {
        Ok(o) if !o.status.success() => {
            warn!(
                dir = %dir.display(),
                stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                "normalize_app_tree_perms failed (non-fatal)"
            );
        }
        Err(e) => {
            warn!(dir = %dir.display(), err = %e, "normalize_app_tree_perms spawn failed (non-fatal)");
        }
        _ => info!(dir = %dir.display(), owner_group = %chown_spec, "app tree perms normalized"),
    }
}
