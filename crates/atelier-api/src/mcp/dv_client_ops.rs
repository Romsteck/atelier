//! `dv_regen_client` — regenerate an app's typed dataverse client crate
//! (`dv-{slug}`) from its LIVE schema, in-process.
//!
//! WHY this exists: the standalone `atelier-dv-codegen` binary is not on the
//! host PATH and nothing in the app build chain invokes it, so the promise
//! "src/ regenerated from the gateway $schema on every build" was never wired
//! up — `schema.lock` silently drifted from the live schema (e.g. wallet stuck
//! at v8 while the DB was at v15). This tool closes that gap: the agent calls
//! it after any schema change (db_create_table/db_add_column/…), it reads the
//! schema straight from the engine (no HTTP, no bearer token on a command
//! line), regenerates the crate, and re-normalises perms so the next build (run
//! as the build user / hr-studio agent) can overwrite the root-written files.

use std::path::{Path, PathBuf};

use atelier_apps::types::valid_slug;
use atelier_ipc::types::IpcResponse;

use super::apps_ops::AppsContext;
use super::scaffold;

/// Regenerate the `dv-{slug}` client crate for `slug` from its live schema.
pub async fn dv_regen_client(ctx: &AppsContext, slug: String) -> IpcResponse {
    if !valid_slug(&slug) {
        return IpcResponse::err("invalid slug");
    }
    let app = match ctx.supervisor.registry.get(&slug).await {
        Some(a) => a,
        None => return IpcResponse::err(format!("app not found: {slug}")),
    };
    if !app.has_db {
        return IpcResponse::err(format!(
            "app '{slug}' has no dataverse DB (has_db=false) — nothing to generate"
        ));
    }

    let src_dir = app.src_dir();
    let crate_dir = match locate_dv_client_dir(&src_dir, &slug) {
        Some(d) => d,
        None => {
            return IpcResponse::err(format!(
                "no dv-client crate found under {} (looked for src/server/dv-client, \
                 src/dv-client, or a Cargo.toml named \"dv-{}\" within 4 levels). \
                 This tool regenerates an EXISTING typed client crate — scaffold it first.",
                src_dir.display(),
                slug.replace('_', "-"),
            ));
        }
    };

    // Live schema straight from the dataverse engine — identical source to
    // `db_get_schema`. No HTTP round-trip, no token in argv.
    let engine = match ctx.dv_engine_for(&slug).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let schema = match engine.get_schema().await {
        Ok(s) => s,
        Err(e) => return IpcResponse::err(format!("get_schema: {e}")),
    };

    let gc = match atelier_dv_codegen::generate_crate(&slug, &schema) {
        Ok(g) => g,
        Err(e) => return IpcResponse::err(format!("generate dv-client: {e}")),
    };
    let changed = match atelier_dv_codegen::write_crate(&crate_dir, &gc) {
        Ok(c) => c,
        Err(e) => {
            return IpcResponse::err(format!(
                "write dv-client into {}: {e}",
                crate_dir.display()
            ));
        }
    };

    // Atelier runs as root, so the files just written are root-owned — the
    // build user / hr-studio agent could not overwrite them on the next regen,
    // nor `cargo build` into `target/`. Re-normalise only when something moved
    // (a recursive chown isn't free). Scoped to the crate dir.
    if !changed.is_empty() {
        scaffold::normalize_app_tree_perms(&crate_dir).await;
    }

    IpcResponse::ok_data(serde_json::json!({
        "slug": slug,
        "crate_dir": crate_dir.display().to_string(),
        "schema_version": gc.schema_version,
        "schema_sha256": gc.schema_sha256,
        "changed": changed,
        "note": "client typé régénéré — rebuild (skill 0-build) puis restart pour le prendre en compte",
    }))
}

/// Locate the `dv-{slug}` client crate directory inside an app's source tree.
/// Tries the two conventional layouts first (cheap), then a bounded walk for a
/// `Cargo.toml` whose package name is `dv-{slug}` (with `_`→`-`).
fn locate_dv_client_dir(src_dir: &Path, slug: &str) -> Option<PathBuf> {
    for c in [
        src_dir.join("server").join("dv-client"),
        src_dir.join("dv-client"),
    ] {
        if c.join("Cargo.toml").is_file() {
            return Some(c);
        }
    }
    let crate_name = format!("dv-{}", slug.replace('_', "-"));
    find_crate_by_name(src_dir, &crate_name, 0)
}

/// Bounded recursive search (depth ≤ 4) for the crate whose `Cargo.toml`
/// declares `name = "<crate_name>"`. Skips build/vcs dirs to stay fast.
fn find_crate_by_name(dir: &Path, crate_name: &str, depth: usize) -> Option<PathBuf> {
    if depth > 4 {
        return None;
    }
    let cargo = dir.join("Cargo.toml");
    if cargo.is_file() {
        if let Ok(txt) = std::fs::read_to_string(&cargo) {
            let needle = format!("\"{crate_name}\"");
            if txt.lines().any(|l| {
                let l = l.trim();
                l.starts_with("name") && l.contains('=') && l.contains(&needle)
            }) {
                return Some(dir.to_path_buf());
            }
        }
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(name, "target" | "node_modules" | ".git" | ".next" | "dist") {
            continue;
        }
        if let Some(found) = find_crate_by_name(&path, crate_name, depth + 1) {
            return Some(found);
        }
    }
    None
}
