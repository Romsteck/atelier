//! Filesystem layer: read/write entries and diagrams under `{root}/{app_id}/`.
//!
//! Layout per app:
//! ```text
//! {app_id}/
//! ├── meta.json
//! ├── overview.md
//! ├── screens/{name}.md
//! ├── features/{name}.md
//! ├── components/{name}.md
//! └── diagrams/{type}-{name}.mmd
//! ```

use std::path::{Path, PathBuf};

use crate::model::{
    DocEntry, DocType, EntrySummary, Frontmatter, Meta, Overview, OverviewIndex, OverviewStats,
    Scope, decode_entry, encode_entry, validate_app_id, validate_entry_name,
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("invalid app_id")]
    InvalidAppId,
    #[error("invalid entry name")]
    InvalidName,
    #[error("app docs not found: {0}")]
    AppNotFound(String),
    #[error("entry not found: {app_id}/{doc_type}/{name}")]
    EntryNotFound {
        app_id: String,
        doc_type: String,
        name: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("operation refused: {0}")]
    Refused(String),
}

/// Filesystem store rooted at `{root}` (typically `/var/lib/atelier/docs`).
#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Path to an app's docs directory: `{root}/{app_id}`.
    pub fn app_dir(&self, app_id: &str) -> PathBuf {
        self.root.join(app_id)
    }

    /// Path to an entry file. For overview, returns `{app_id}/overview.md`.
    pub fn entry_path(&self, app_id: &str, doc_type: DocType, name: &str) -> PathBuf {
        match doc_type.subdir() {
            None => self.app_dir(app_id).join("overview.md"),
            Some(sub) => self.app_dir(app_id).join(sub).join(format!("{name}.md")),
        }
    }

    /// Path to a diagram file `diagrams/{type}-{name}.mmd`. For overview, name is `"overview"`.
    pub fn diagram_path(&self, app_id: &str, doc_type: DocType, name: &str) -> PathBuf {
        let safe_name = if doc_type == DocType::Overview {
            "overview"
        } else {
            name
        };
        self.app_dir(app_id)
            .join("diagrams")
            .join(format!("{}-{}.mmd", doc_type.as_str(), safe_name))
    }

    /// List all app_ids that have a `meta.json`. Hidden dirs (`_index.sqlite` etc.) are skipped.
    pub fn list_app_ids(&self) -> Result<Vec<String>, StoreError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('_') || name.starts_with('.') {
                continue;
            }
            if !validate_app_id(&name) {
                continue;
            }
            out.push(name);
        }
        out.sort();
        Ok(out)
    }

    pub fn read_meta(&self, app_id: &str) -> Result<Meta, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let path = self.app_dir(app_id).join("meta.json");
        if !path.exists() {
            return Err(StoreError::AppNotFound(app_id.to_string()));
        }
        let raw = std::fs::read_to_string(&path)?;
        let meta: Meta = serde_json::from_str(&raw).unwrap_or_else(|_| Meta::new(app_id));
        Ok(meta)
    }

    pub fn write_meta(&self, app_id: &str, meta: &Meta) -> Result<(), StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let dir = self.app_dir(app_id);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("meta.json");
        let body = serde_json::to_string_pretty(meta)?;
        std::fs::write(&path, body)?;
        Ok(())
    }

    /// Ensure the v2 directory layout exists for this app (creates subdirs and overview if missing).
    pub fn ensure_layout(&self, app_id: &str) -> Result<(), StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let dir = self.app_dir(app_id);
        std::fs::create_dir_all(dir.join("screens"))?;
        std::fs::create_dir_all(dir.join("features"))?;
        std::fs::create_dir_all(dir.join("components"))?;
        std::fs::create_dir_all(dir.join("diagrams"))?;
        Ok(())
    }

    /// Read a single entry. For overview, `name` is ignored (we always read `overview.md`).
    pub fn read_entry(
        &self,
        app_id: &str,
        doc_type: DocType,
        name: &str,
    ) -> Result<DocEntry, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let effective_name = if doc_type == DocType::Overview {
            "overview".to_string()
        } else {
            if !validate_entry_name(name) {
                return Err(StoreError::InvalidName);
            }
            name.to_string()
        };
        let path = self.entry_path(app_id, doc_type, &effective_name);
        if !path.exists() {
            return Err(StoreError::EntryNotFound {
                app_id: app_id.to_string(),
                doc_type: doc_type.as_str().to_string(),
                name: effective_name,
            });
        }
        let raw = std::fs::read_to_string(&path)?;
        let (frontmatter, body) = decode_entry(&raw);
        Ok(DocEntry {
            app_id: app_id.to_string(),
            doc_type,
            name: effective_name,
            frontmatter,
            body,
        })
    }

    /// Write or create an entry. Auto-creates parent dirs.
    /// Always stamps `updated_at` to now (UTC) and refreshes `diagram` flag to match disk.
    pub fn write_entry(
        &self,
        app_id: &str,
        doc_type: DocType,
        name: &str,
        mut frontmatter: Frontmatter,
        body: &str,
    ) -> Result<DocEntry, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let effective_name = if doc_type == DocType::Overview {
            "overview".to_string()
        } else {
            if !validate_entry_name(name) {
                return Err(StoreError::InvalidName);
            }
            name.to_string()
        };

        // Validate scope sanity for features.
        if doc_type == DocType::Feature {
            if let Some(ref s) = frontmatter.scope {
                if Scope::from_frontmatter(s).is_none() {
                    return Err(StoreError::Refused(format!(
                        "invalid scope '{s}', expected 'global' or 'screen:<name>'"
                    )));
                }
            }
        } else {
            // Strip scope for non-feature types — it's meaningless and would confuse readers.
            frontmatter.scope = None;
            frontmatter.parent_screen = None;
        }

        self.ensure_layout(app_id)?;

        // Refresh diagram flag based on actual disk state.
        let diagram_path = self.diagram_path(app_id, doc_type, &effective_name);
        frontmatter.diagram = diagram_path.exists();

        // Stamp update time.
        frontmatter.updated_at = Some(now_iso8601());

        let path = self.entry_path(app_id, doc_type, &effective_name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = encode_entry(&frontmatter, body);
        std::fs::write(&path, raw)?;

        Ok(DocEntry {
            app_id: app_id.to_string(),
            doc_type,
            name: effective_name,
            frontmatter,
            body: body.to_string(),
        })
    }

    /// Delete an entry. Refuses to delete the overview.
    pub fn delete_entry(
        &self,
        app_id: &str,
        doc_type: DocType,
        name: &str,
    ) -> Result<bool, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        if doc_type == DocType::Overview {
            return Err(StoreError::Refused("cannot delete the overview".into()));
        }
        if !validate_entry_name(name) {
            return Err(StoreError::InvalidName);
        }
        let path = self.entry_path(app_id, doc_type, name);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path)?;
        // Also remove diagram if any.
        let diag = self.diagram_path(app_id, doc_type, name);
        if diag.exists() {
            let _ = std::fs::remove_file(&diag);
        }
        Ok(true)
    }

    /// Read mermaid diagram. Returns Ok(None) if no diagram is attached.
    pub fn read_diagram(
        &self,
        app_id: &str,
        doc_type: DocType,
        name: &str,
    ) -> Result<Option<String>, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let effective_name = if doc_type == DocType::Overview {
            "overview".to_string()
        } else {
            if !validate_entry_name(name) {
                return Err(StoreError::InvalidName);
            }
            name.to_string()
        };
        let path = self.diagram_path(app_id, doc_type, &effective_name);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(std::fs::read_to_string(&path)?))
    }

    /// Write mermaid diagram. Max size: 32 KB. Updates the `diagram: true` flag in the
    /// associated entry's frontmatter.
    pub fn write_diagram(
        &self,
        app_id: &str,
        doc_type: DocType,
        name: &str,
        mermaid: &str,
    ) -> Result<(), StoreError> {
        if mermaid.len() > 32 * 1024 {
            return Err(StoreError::Refused(format!(
                "diagram too large: {} bytes (max 32 KB)",
                mermaid.len()
            )));
        }
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let effective_name = if doc_type == DocType::Overview {
            "overview".to_string()
        } else {
            if !validate_entry_name(name) {
                return Err(StoreError::InvalidName);
            }
            name.to_string()
        };
        self.ensure_layout(app_id)?;
        let path = self.diagram_path(app_id, doc_type, &effective_name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, mermaid)?;

        // Update the entry's frontmatter `diagram: true` so readers stay in sync.
        if let Ok(entry) = self.read_entry(app_id, doc_type, &effective_name) {
            let mut fm = entry.frontmatter;
            fm.diagram = true;
            fm.updated_at = Some(now_iso8601());
            let raw = encode_entry(&fm, &entry.body);
            let entry_path = self.entry_path(app_id, doc_type, &effective_name);
            let _ = std::fs::write(&entry_path, raw);
        }

        Ok(())
    }

    /// List entries (summaries) for an app, optionally filtered by type. Overview is excluded.
    pub fn list_entries(
        &self,
        app_id: &str,
        doc_type: Option<DocType>,
    ) -> Result<Vec<EntrySummary>, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        let mut out = Vec::new();
        let types = match doc_type {
            Some(t) if t != DocType::Overview => vec![t],
            None => vec![DocType::Screen, DocType::Feature, DocType::Component],
            Some(_) => return Ok(out), // overview not listable
        };
        for t in types {
            let Some(sub) = t.subdir() else { continue };
            let dir = self.app_dir(app_id).join(sub);
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().to_string();
                let Some(stem) = file_name.strip_suffix(".md") else {
                    continue;
                };
                if !validate_entry_name(stem) {
                    continue;
                }
                let path = entry.path();
                let raw = std::fs::read_to_string(&path).unwrap_or_default();
                let (fm, _body) = decode_entry(&raw);
                let has_diagram = self.diagram_path(app_id, t, stem).exists();
                out.push(EntrySummary {
                    doc_type: t,
                    name: stem.to_string(),
                    title: fm.title,
                    summary: fm.summary.map(truncate_summary),
                    scope: fm.scope,
                    parent_screen: fm.parent_screen,
                    has_diagram,
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Build the full Overview struct (meta + overview entry + index + stats).
    pub fn overview(&self, app_id: &str) -> Result<Overview, StoreError> {
        if !validate_app_id(app_id) {
            return Err(StoreError::InvalidAppId);
        }
        if !self.app_dir(app_id).exists() {
            return Err(StoreError::AppNotFound(app_id.to_string()));
        }
        let meta = self
            .read_meta(app_id)
            .unwrap_or_else(|_| Meta::new(app_id));

        let overview_entry = self.read_entry(app_id, DocType::Overview, "overview").ok();

        let entries = self.list_entries(app_id, None)?;
        let mut screens = Vec::new();
        let mut features = Vec::new();
        let mut components = Vec::new();
        let mut with_diagram = 0u32;
        for e in entries {
            if e.has_diagram {
                with_diagram += 1;
            }
            match e.doc_type {
                DocType::Screen => screens.push(e),
                DocType::Feature => features.push(e),
                DocType::Component => components.push(e),
                DocType::Overview => {}
            }
        }
        let stats = OverviewStats {
            screens: screens.len() as u32,
            features: features.len() as u32,
            components: components.len() as u32,
            with_diagram,
            has_overview: overview_entry.is_some(),
        };

        Ok(Overview {
            app_id: app_id.to_string(),
            meta,
            overview: overview_entry,
            index: OverviewIndex {
                screens,
                features,
                components,
            },
            stats,
        })
    }
}

/// Truncate a summary to 120 chars with ellipsis, preserving char boundaries.
fn truncate_summary(mut s: String) -> String {
    const MAX: usize = 120;
    if s.len() <= MAX {
        return s;
    }
    // floor_char_boundary is unstable on stable as of edition 2024 — find a safe cut manually.
    let mut cut = MAX;
    while !s.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    s.truncate(cut);
    s.push('…');
    s
}

/// ISO-8601 timestamp `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso8601() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Iterate over every `(app_id, type, name)` found on disk under `{root}`.
/// Used by the index to rebuild from filesystem.
pub fn walk_entries(root: &Path) -> Result<Vec<(String, DocType, String)>, StoreError> {
    let store = Store::new(root.to_path_buf());
    let mut out = Vec::new();
    for app_id in store.list_app_ids()? {
        // Overview lives at {app_id}/overview.md
        if store.app_dir(&app_id).join("overview.md").exists() {
            out.push((app_id.clone(), DocType::Overview, "overview".to_string()));
        }
        for t in [DocType::Screen, DocType::Feature, DocType::Component] {
            let Some(sub) = t.subdir() else { continue };
            let dir = store.app_dir(&app_id).join(sub);
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().to_string();
                let Some(stem) = file_name.strip_suffix(".md") else {
                    continue;
                };
                if !validate_entry_name(stem) {
                    continue;
                }
                out.push((app_id.clone(), t, stem.to_string()));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("atelier-docs-test-{pid}-{n}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn meta_roundtrip() {
        let root = tmp_root();
        let s = Store::new(&root);
        let mut meta = Meta::new("home");
        meta.stack = "flutter".into();
        meta.description = "Page d'accueil".into();
        s.write_meta("home", &meta).unwrap();
        let m2 = s.read_meta("home").unwrap();
        assert_eq!(m2.stack, "flutter");
        assert_eq!(m2.schema_version, crate::SCHEMA_VERSION);
    }

    #[test]
    fn entry_roundtrip_and_overview() {
        let root = tmp_root();
        let s = Store::new(&root);
        let meta = Meta::new("home");
        s.write_meta("home", &meta).unwrap();

        let mut fm = Frontmatter::default();
        fm.title = Some("Home screen".into());
        fm.summary = Some("Liste des apps".into());
        s.write_entry(
            "home",
            DocType::Screen,
            "home",
            fm,
            "## Description\n\nAccueil.",
        )
        .unwrap();

        let mut fm = Frontmatter::default();
        fm.title = Some("Auth login".into());
        fm.summary = Some("Connexion utilisateur".into());
        fm.scope = Some("global".into());
        s.write_entry("home", DocType::Feature, "auth-login", fm, "Body")
            .unwrap();

        let mut fm = Frontmatter::default();
        fm.title = Some("Vue d'ensemble".into());
        s.write_entry("home", DocType::Overview, "overview", fm, "Pitch utilisateur.")
            .unwrap();

        let ov = s.overview("home").unwrap();
        assert_eq!(ov.stats.screens, 1);
        assert_eq!(ov.stats.features, 1);
        assert_eq!(ov.stats.components, 0);
        assert!(ov.stats.has_overview);
        assert_eq!(ov.overview.as_ref().unwrap().frontmatter.title.as_deref(), Some("Vue d'ensemble"));
        assert_eq!(ov.index.screens[0].name, "home");
    }

    #[test]
    fn diagram_roundtrip() {
        let root = tmp_root();
        let s = Store::new(&root);
        let meta = Meta::new("home");
        s.write_meta("home", &meta).unwrap();
        s.write_entry(
            "home",
            DocType::Screen,
            "home",
            Frontmatter::default(),
            "Body",
        )
        .unwrap();
        s.write_diagram("home", DocType::Screen, "home", "flowchart LR\n  a-->b")
            .unwrap();
        let d = s.read_diagram("home", DocType::Screen, "home").unwrap();
        assert!(d.is_some());
        // Frontmatter should be flagged
        let entry = s.read_entry("home", DocType::Screen, "home").unwrap();
        assert!(entry.frontmatter.diagram);
    }

    #[test]
    fn delete_overview_refused() {
        let root = tmp_root();
        let s = Store::new(&root);
        let err = s.delete_entry("home", DocType::Overview, "overview");
        assert!(matches!(err, Err(StoreError::Refused(_))));
    }

    #[test]
    fn invalid_app_id_rejected() {
        let root = tmp_root();
        let s = Store::new(&root);
        let r = s.read_meta("a/b");
        assert!(matches!(r, Err(StoreError::InvalidAppId)));
    }
}
