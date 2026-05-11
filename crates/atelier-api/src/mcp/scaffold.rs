//! Scaffold minimal source trees for newly created apps.
//!
//! Templates are embedded at compile time from
//! `crates/orchestrator/hr-apps/templates/{stack}/`. Files are written
//! idempotently — anything already present on disk is left untouched.
//!
//! # INVARIANT — workspace code-server
//!
//! `app.src_dir()` (`{slug}/src/`) est **à la fois** le dossier des sources de
//! l'app ET le workspace code-server du Studio. Tout fichier qui doit être
//! édité/lu par l'agent Claude Code (ex : un `README.md` initial, un
//! `.env.example`) DOIT être placé directement sous `src/`.
//!
//! La génération de `CLAUDE.md`, `.claude/`, `.mcp.json` relève de
//! [`hr_apps::context`] (appelé par le handler AppCreate juste après ce
//! scaffold). Ne les écris pas ici.

use std::path::Path;

use hr_apps::types::{AppStack, Application};
use tracing::{info, warn};

const T_AXUM_CARGO: &str = include_str!("../../../hr-apps/templates/axum/Cargo.toml");
const T_AXUM_MAIN: &str = include_str!("../../../hr-apps/templates/axum/src/main.rs");

const T_AXUMVITE_CARGO: &str = include_str!("../../../hr-apps/templates/axum-vite/Cargo.toml");
const T_AXUMVITE_MAIN: &str = include_str!("../../../hr-apps/templates/axum-vite/src/main.rs");
const T_AXUMVITE_PKG: &str = include_str!("../../../hr-apps/templates/axum-vite/web/package.json");
const T_AXUMVITE_VITE: &str = include_str!("../../../hr-apps/templates/axum-vite/web/vite.config.ts");
const T_AXUMVITE_HTML: &str = include_str!("../../../hr-apps/templates/axum-vite/web/index.html");

const T_NEXT_PKG: &str = include_str!("../../../hr-apps/templates/next-js/package.json");
const T_NEXT_CFG: &str = include_str!("../../../hr-apps/templates/next-js/next.config.js");
const T_NEXT_PAGE: &str = include_str!("../../../hr-apps/templates/next-js/app/page.tsx");
const T_NEXT_LAYOUT: &str = include_str!("../../../hr-apps/templates/next-js/app/layout.tsx");

#[tracing::instrument(skip(app), fields(slug = %app.slug, stack = ?app.stack))]
pub async fn scaffold_stack_template(app: &Application) -> anyhow::Result<()> {
    scaffold_stack_template_at(app, &app.src_dir()).await
}

/// Variante explicite : scaffold le template stack dans `src` (au lieu de
/// `app.src_dir()` hardcodé). Utilisé par AppCreate quand les sources vivent
/// sur CloudMaster — on génère localement dans un tmpdir puis on rsync UP.
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
