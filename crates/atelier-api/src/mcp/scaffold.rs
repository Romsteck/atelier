//! Scaffold minimal source trees for newly created apps.
//!
//! Templates are embedded at compile time from
//! `crates/atelier-apps/templates/{stack}/`. Files are written
//! idempotently — anything already present on disk is left untouched.
//!
//! # INVARIANT — workspace de l'app (Studio)
//!
//! `app.src_dir()` (`{slug}/src/`) est **à la fois** le dossier des sources de
//! l'app ET le workspace ouvert par le Studio (agent Claude Code). Tout fichier qui doit être
//! édité/lu par l'agent Claude Code (ex : un `README.md` initial, un
//! `.env.example`) DOIT être placé directement sous `src/`.
//!
//! La génération de `CLAUDE.md`, `.claude/`, `.mcp.json` relève de
//! [`atelier_apps::context`] (appelé par le handler AppCreate juste après ce
//! scaffold). Ne les écris pas ici.

use std::path::Path;

use atelier_apps::types::{AppStack, Application};
use tracing::{info, warn};

const T_AXUM_CARGO: &str = include_str!("../../../atelier-apps/templates/axum/Cargo.toml");
const T_AXUM_MAIN: &str = include_str!("../../../atelier-apps/templates/axum/src/main.rs");

const T_AXUMVITE_CARGO: &str = include_str!("../../../atelier-apps/templates/axum-vite/Cargo.toml");
const T_AXUMVITE_MAIN: &str = include_str!("../../../atelier-apps/templates/axum-vite/src/main.rs");
const T_AXUMVITE_PKG: &str = include_str!("../../../atelier-apps/templates/axum-vite/web/package.json");
const T_AXUMVITE_VITE: &str = include_str!("../../../atelier-apps/templates/axum-vite/web/vite.config.ts");
const T_AXUMVITE_HTML: &str = include_str!("../../../atelier-apps/templates/axum-vite/web/index.html");

const T_NEXT_PKG: &str = include_str!("../../../atelier-apps/templates/next-js/package.json");
const T_NEXT_CFG: &str = include_str!("../../../atelier-apps/templates/next-js/next.config.js");
const T_NEXT_PAGE: &str = include_str!("../../../atelier-apps/templates/next-js/app/page.tsx");
const T_NEXT_LAYOUT: &str = include_str!("../../../atelier-apps/templates/next-js/app/layout.tsx");

#[tracing::instrument(skip(app), fields(slug = %app.slug, stack = ?app.stack))]
pub async fn scaffold_stack_template(app: &Application) -> anyhow::Result<()> {
    scaffold_stack_template_at(app, &app.src_dir()).await
}

/// Variante explicite : scaffold le template stack dans `src` (au lieu de
/// `app.src_dir()` hardcodé). Utilisé par AppCreate (héritage de l'époque où
/// les sources vivaient sur un hôte de build séparé : génération en tmpdir
/// puis rsync UP ; désormais on écrit directement dans le src/runtime root).
#[tracing::instrument(skip(app, src), fields(slug = %app.slug, stack = ?app.stack, target = %src.display()))]
pub async fn scaffold_stack_template_at(app: &Application, src: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(src).await?;

    match app.stack {
        AppStack::Axum => {
            write_if_missing(&src.join("Cargo.toml"), &subst(T_AXUM_CARGO, &app.slug)).await?;
            write_if_missing(&src.join("src/main.rs"), &subst(T_AXUM_MAIN, &app.slug)).await?;
        }
        AppStack::AxumVite => {
            write_if_missing(&src.join("Cargo.toml"), &subst(T_AXUMVITE_CARGO, &app.slug)).await?;
            write_if_missing(&src.join("src/main.rs"), &subst(T_AXUMVITE_MAIN, &app.slug)).await?;
            write_if_missing(&src.join("web/package.json"), &subst(T_AXUMVITE_PKG, &app.slug)).await?;
            write_if_missing(&src.join("web/vite.config.ts"), &subst(T_AXUMVITE_VITE, &app.slug)).await?;
            write_if_missing(&src.join("web/index.html"), &subst(T_AXUMVITE_HTML, &app.slug)).await?;
        }
        AppStack::NextJs => {
            write_if_missing(&src.join("package.json"), &subst(T_NEXT_PKG, &app.slug)).await?;
            write_if_missing(&src.join("next.config.js"), &subst(T_NEXT_CFG, &app.slug)).await?;
            write_if_missing(&src.join("app/page.tsx"), &subst(T_NEXT_PAGE, &app.slug)).await?;
            write_if_missing(&src.join("app/layout.tsx"), &subst(T_NEXT_LAYOUT, &app.slug)).await?;
        }
        AppStack::Flutter => {
            // Flutter app scaffold not implemented — users bring their own project.
        }
    }

    info!(slug = %app.slug, target = %src.display(), "scaffold template applied");
    Ok(())
}

/// Compute a sensible default `run_command` for the given stack.
pub fn default_run_command(app: &Application) -> String {
    match app.stack {
        AppStack::Axum | AppStack::AxumVite => format!("./target/release/{}", app.slug),
        AppStack::NextJs => "npm run start -- -p $PORT".to_string(),
        AppStack::Flutter => String::new(),
    }
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
    let owner = std::env::var("ATELIER_BUILD_AS_USER")
        .ok()
        .filter(|s| !s.is_empty());
    let chown_spec = match owner {
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

fn subst(template: &str, slug: &str) -> String {
    template.replace("{SLUG}", slug)
}

async fn write_if_missing(path: &Path, content: &str) -> anyhow::Result<()> {
    if path.exists() {
        info!(path = %path.display(), "scaffold: skip existing file");
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            warn!(path = %parent.display(), error = %e, "scaffold: mkdir failed");
            return Err(e.into());
        }
    }
    tokio::fs::write(path, content).await?;
    info!(path = %path.display(), bytes = content.len(), "scaffold: wrote file");
    Ok(())
}
