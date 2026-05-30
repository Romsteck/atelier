//! Legacy → v2 migration. Idempotent and non-breaking on already-migrated apps.
//!
//! Decision (per plan): rupture nette — drop the old `structure.md` / `features.md` /
//! `backend.md` / `notes.md` files. Keep only the `meta.json` (preserving `name`, `stack`,
//! `description`, `logo`) and stamp `schema_version: 2`. Create the empty subdirs and a
//! placeholder `overview.md`. The agent reconstructs the structured docs as it works.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::SCHEMA_VERSION;
use crate::fs::{Store, now_iso8601};
use crate::model::{Frontmatter, Meta, encode_entry};

const LEGACY_FILES: &[&str] = &["structure.md", "features.md", "backend.md", "notes.md"];

/// Result of a migration pass.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MigrateReport {
    pub migrated_apps: Vec<String>,
    pub already_v2: Vec<String>,
    pub errors: Vec<String>,
}

/// Run migration on every app under `{root}`. Idempotent — apps already at `schema_version: 2`
/// are skipped. Non-existent root is treated as a no-op.
pub fn run_all(root: impl AsRef<Path>) -> MigrateReport {
    let root = root.as_ref();
    let mut report = MigrateReport::default();
    if !root.exists() {
        info!(?root, "Docs root does not exist yet — skipping migration");
        return report;
    }
    let store = Store::new(root.to_path_buf());
    let app_ids = match store.list_app_ids() {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!("list_app_ids failed: {e}"));
            return report;
        }
    };
    for app_id in app_ids {
        match migrate_app(&store, &app_id, false) {
            Ok(true) => report.migrated_apps.push(app_id),
            Ok(false) => report.already_v2.push(app_id),
            Err(e) => report.errors.push(format!("{app_id}: {e}")),
        }
    }
    info!(
        migrated = report.migrated_apps.len(),
        already_v2 = report.already_v2.len(),
        errors = report.errors.len(),
        "Docs migration complete"
    );
    report
}

/// Run a dry-run report without writing anything.
pub fn dry_run(root: impl AsRef<Path>) -> MigrateReport {
    let root = root.as_ref();
    let mut report = MigrateReport::default();
    if !root.exists() {
        return report;
    }
    let store = Store::new(root.to_path_buf());
    let app_ids = match store.list_app_ids() {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(format!("list_app_ids failed: {e}"));
            return report;
        }
    };
    for app_id in app_ids {
        match needs_migration(&store, &app_id) {
            Ok(true) => report.migrated_apps.push(app_id),
            Ok(false) => report.already_v2.push(app_id),
            Err(e) => report.errors.push(format!("{app_id}: {e}")),
        }
    }
    report
}

fn needs_migration(store: &Store, app_id: &str) -> Result<bool, String> {
    let dir = store.app_dir(app_id);
    let meta_path = dir.join("meta.json");
    if !meta_path.exists() {
        // No meta yet — treat as fresh app, migration is the same as initialization.
        return Ok(true);
    }
    let raw = std::fs::read_to_string(&meta_path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::json!({}));
    let version = v
        .get("schema_version")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    Ok(version < SCHEMA_VERSION as u64)
}

/// Migrate a single app. Returns Ok(true) if migration was performed, Ok(false) if already v2.
/// `dry` is currently unused but kept for symmetry with `dry_run` if we extend in the future.
fn migrate_app(store: &Store, app_id: &str, _dry: bool) -> Result<bool, String> {
    if !needs_migration(store, app_id)? {
        return Ok(false);
    }

    let dir = store.app_dir(app_id);

    // 1. Read existing meta to preserve name/stack/description/logo.
    let meta_path = dir.join("meta.json");
    let preserved: serde_json::Value = std::fs::read_to_string(&meta_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(serde_json::json!({}));

    let meta = Meta {
        name: preserved
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(app_id)
            .to_string(),
        stack: preserved
            .get("stack")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        description: preserved
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        logo: preserved
            .get("logo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        schema_version: SCHEMA_VERSION,
    };

    // 2. Delete legacy section files.
    for f in LEGACY_FILES {
        let p = dir.join(f);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(&p) {
                warn!(app_id, file = f, error = %e, "Failed to remove legacy doc file");
            }
        }
    }

    // 3. Ensure new layout (subdirs).
    store.ensure_layout(app_id).map_err(|e| e.to_string())?;

    // 4. Write fresh meta.
    store.write_meta(app_id, &meta).map_err(|e| e.to_string())?;

    // 5. Create placeholder overview.md if absent.
    let overview_path = dir.join("overview.md");
    if !overview_path.exists() {
        let mut fm = Frontmatter::default();
        fm.title = Some(meta.name.clone());
        fm.summary = Some("Documentation à régénérer (migration v2).".into());
        fm.updated_at = Some(now_iso8601());
        let body = "# Vue d'ensemble\n\n\
                    > _Documentation à régénérer après la migration v2._\n\n\
                    Cette app vient d'être migrée vers la nouvelle structure de documentation. \
                    L'agent doit reconstruire l'overview, les écrans, les features et les composants \
                    en se basant sur le code et les besoins utilisateur.\n";
        let raw = encode_entry(&fm, body);
        std::fs::write(&overview_path, raw).map_err(|e| e.to_string())?;
    }

    info!(app_id, "Migrated app docs to v2");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("atelier-docs-migrate-{pid}-{n}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_legacy(root: &std::path::Path, app_id: &str) {
        let dir = root.join(app_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            r#"{"name":"Home","stack":"flutter","description":"d","logo":""}"#,
        )
        .unwrap();
        std::fs::write(dir.join("structure.md"), "old structure").unwrap();
        std::fs::write(dir.join("features.md"), "old features").unwrap();
        std::fs::write(dir.join("backend.md"), "old backend").unwrap();
        std::fs::write(dir.join("notes.md"), "old notes").unwrap();
    }

    #[test]
    fn migrates_legacy_app() {
        let root = tmp_root();
        write_legacy(&root, "home");

        let report = run_all(&root);
        assert_eq!(report.migrated_apps, vec!["home".to_string()]);
        assert!(report.errors.is_empty());

        // Legacy files gone
        let dir = root.join("home");
        assert!(!dir.join("structure.md").exists());
        assert!(!dir.join("features.md").exists());
        assert!(!dir.join("backend.md").exists());
        assert!(!dir.join("notes.md").exists());

        // New layout present
        assert!(dir.join("screens").is_dir());
        assert!(dir.join("features").is_dir());
        assert!(dir.join("components").is_dir());
        assert!(dir.join("diagrams").is_dir());
        assert!(dir.join("overview.md").exists());

        // Meta preserved + schema_version=2
        let raw = std::fs::read_to_string(dir.join("meta.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["schema_version"], serde_json::json!(2));
        assert_eq!(v["name"], serde_json::json!("Home"));
        assert_eq!(v["stack"], serde_json::json!("flutter"));
    }

    #[test]
    fn idempotent_on_v2_app() {
        let root = tmp_root();
        write_legacy(&root, "home");
        let _ = run_all(&root); // first pass migrates
        let report = run_all(&root); // second pass should be a no-op
        assert!(report.migrated_apps.is_empty());
        assert_eq!(report.already_v2, vec!["home".to_string()]);
    }

    #[test]
    fn empty_root_is_safe() {
        let root = tmp_root();
        let report = run_all(&root);
        assert!(report.migrated_apps.is_empty());
        assert!(report.errors.is_empty());
    }
}
