//! `atelier-dv-codegen` — turn an app's dataverse `DatabaseSchema` into a
//! typed Rust client crate (`dv-{slug}`).
//!
//! Two consumers share this library:
//! - the standalone `atelier-dv-codegen` binary (feature `cli`), which fetches
//!   the schema over HTTPS and writes the crate to disk;
//! - Atelier's in-process `dv_regen_client` MCP tool, which already holds the
//!   live `DatabaseSchema` (via the dataverse engine) and writes the crate
//!   directly — no HTTP, no bearer token on the command line.
//!
//! Output layout of a generated crate:
//! - `Cargo.toml`  (committed, stable across regenerations)
//! - `.gitignore`  (committed, ignores the generated `src/`)
//! - `schema.lock` (committed: schema_version + sha256 + slug)
//! - `src/lib.rs`  (generated, gitignored)

pub mod generator;

use anyhow::{Context, Result};
use atelier_dataverse::DatabaseSchema;
use sha2::Digest;
use std::path::Path;

/// Relative paths (within the crate dir) of the files this generator owns.
/// Order is stable so callers can present a deterministic changed-file list.
pub const CRATE_FILES: [&str; 4] = ["Cargo.toml", ".gitignore", "schema.lock", "src/lib.rs"];

/// The in-memory result of generating a `dv-{slug}` crate: every owned file's
/// content plus the schema fingerprint recorded in `schema.lock`.
pub struct GeneratedCrate {
    /// `(relative_path, content)` for each file in [`CRATE_FILES`].
    pub files: Vec<(&'static str, String)>,
    pub schema_version: u64,
    pub schema_sha256: String,
}

/// Canonical fingerprint of a schema. Owned by the library so the CLI and the
/// in-process tool agree byte-for-byte: both hash `serde_json::to_string` of the
/// schema in declaration order, rather than whatever JSON text a given transport
/// happened to produce.
///
/// The top-level `updated_at` is normalised out first: `engine.get_schema()`
/// stamps it with `Utc::now()` on EVERY read, so hashing it would make the
/// fingerprint — and thus the committed `schema.lock` — change on every
/// `dv_regen_client` call even for an identical schema, breaking idempotency
/// (`write_crate` would always report `schema.lock` as changed, triggering a
/// needless chown + a misleading "schema moved" signal) and dirtying git.
/// Table-level timestamps are persisted and move only on real DDL, so they stay
/// in the hash and still signal genuine drift. A one-time `schema.lock` churn on
/// the first regen after this change is expected (the lock had drifted anyway).
pub fn schema_sha256(schema: &DatabaseSchema) -> Result<String> {
    let mut canonical = schema.clone();
    canonical.updated_at = None;
    let json = serde_json::to_string(&canonical).context("serialise schema for hashing")?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(json.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Produce the full contents of a `dv-{slug}` crate from its schema.
pub fn generate_crate(slug: &str, schema: &DatabaseSchema) -> Result<GeneratedCrate> {
    let hash = schema_sha256(schema)?;
    let cargo_toml = generator::generate_cargo_toml(slug);
    let gitignore = "src/\n".to_string();
    let lock = format!(
        "schema_version={}\nschema_sha256={}\nslug={}\n",
        schema.version, hash, slug
    );
    let lib_rs = generator::generate_lib(slug, schema)?;
    Ok(GeneratedCrate {
        files: vec![
            ("Cargo.toml", cargo_toml),
            (".gitignore", gitignore),
            ("schema.lock", lock),
            ("src/lib.rs", lib_rs),
        ],
        schema_version: schema.version,
        schema_sha256: hash,
    })
}

/// Write a generated crate into `out_dir`, touching only files whose content
/// changed (see [`write_if_different`]). Returns the relative paths that were
/// actually written, so callers can report a precise diff and skip perms
/// fix-ups / rebuilds when nothing moved.
pub fn write_crate(out_dir: &Path, gc: &GeneratedCrate) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(out_dir)?;
    std::fs::create_dir_all(out_dir.join("src"))?;
    let mut changed = Vec::new();
    for (rel, content) in &gc.files {
        if write_if_different(&out_dir.join(rel), content)? {
            changed.push((*rel).to_string());
        }
    }
    Ok(changed)
}

/// Write `content` to `path` only if it differs from what's already there.
/// Returns `true` if the file was written. Avoids spurious `mtime` bumps that
/// would re-trigger downstream `cargo build` invocations.
pub fn write_if_different(path: &Path, content: &str) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    std::fs::write(path, content)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_dataverse::DatabaseSchema;
    use chrono::{TimeZone, Utc};

    /// The schema fingerprint MUST ignore the volatile top-level `updated_at`
    /// (re-stamped `Utc::now()` on every `engine.get_schema()`), else
    /// `dv_regen_client` rewrites the committed `schema.lock` on every call
    /// even when nothing changed. A real change (version bump) still moves it.
    #[test]
    fn schema_hash_ignores_volatile_updated_at() {
        let base = DatabaseSchema { tables: vec![], relations: vec![], version: 7, updated_at: None };
        let mut a = base.clone();
        a.updated_at = Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
        let mut b = base.clone();
        b.updated_at = Some(Utc.with_ymd_and_hms(2026, 7, 6, 15, 0, 0).unwrap());
        assert_eq!(
            schema_sha256(&a).unwrap(),
            schema_sha256(&b).unwrap(),
            "differing only in the volatile updated_at must not change the hash"
        );

        let mut c = a.clone();
        c.version = 8;
        assert_ne!(
            schema_sha256(&a).unwrap(),
            schema_sha256(&c).unwrap(),
            "a real schema change (version bump) must change the hash"
        );
    }
}
