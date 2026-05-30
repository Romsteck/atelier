//! SQLite FTS5 index over docs entries. Filesystem is the source of truth — this index is
//! purely a search accelerator and is reconstructible from the filesystem at any time.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use tracing::{info, warn};

use crate::fs::{Store, StoreError, walk_entries};
use crate::model::{DocEntry, DocType};

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("FTS5 not available in the bundled SQLite — falling back to LIKE search")]
    Fts5Unavailable,
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// One search hit returned by `Index::search`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub app_id: String,
    pub doc_type: String,
    pub name: String,
    pub title: Option<String>,
    /// BM25 score (lower = better match in FTS5).
    pub score: f64,
    pub snippet: String,
    /// Which field matched: `title`, `summary`, or `body`.
    pub matched_field: String,
}

/// SQLite-backed index. Wraps a single `Connection` behind a `Mutex` — operations are sync
/// and cheap enough to hold the lock for the duration of a query.
pub struct Index {
    conn: Mutex<Connection>,
    pub fts5_available: bool,
}

impl Index {
    /// Open or create the index at `path`. Schema is initialized on first open.
    /// If FTS5 is not available in the bundled SQLite, search falls back to a LIKE-based scan
    /// over `doc_entries` (still functional, just slower and without ranking).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IndexError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        let fts5_available = check_fts5(&conn);
        if !fts5_available {
            warn!(
                "FTS5 not available in bundled SQLite — docs.search will fall back to LIKE scan"
            );
        }

        Self::init_schema(&conn, fts5_available)?;
        Ok(Self {
            conn: Mutex::new(conn),
            fts5_available,
        })
    }

    /// Open or create the index. If empty (no rows), rebuild from the filesystem at `docs_root`.
    pub fn open_or_rebuild(
        index_path: impl AsRef<Path>,
        docs_root: impl Into<PathBuf>,
    ) -> Result<Self, IndexError> {
        let docs_root: PathBuf = docs_root.into();
        let idx = Self::open(index_path)?;
        let count = idx.count()?;
        if count == 0 {
            info!("Docs index empty — rebuilding from filesystem");
            idx.rebuild_from_fs(&Store::new(docs_root))?;
        }
        Ok(idx)
    }

    fn init_schema(conn: &Connection, fts5: bool) -> Result<(), IndexError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS doc_entries (
                app_id        TEXT NOT NULL,
                doc_type      TEXT NOT NULL,
                name          TEXT NOT NULL,
                title         TEXT,
                summary       TEXT,
                scope         TEXT,
                parent_screen TEXT,
                code_refs     TEXT,
                links         TEXT,
                has_diagram   INTEGER NOT NULL DEFAULT 0,
                body          TEXT NOT NULL,
                updated_at    TEXT,
                PRIMARY KEY (app_id, doc_type, name)
            );
            CREATE INDEX IF NOT EXISTS doc_entries_app_idx ON doc_entries(app_id);
            CREATE INDEX IF NOT EXISTS doc_entries_type_idx ON doc_entries(doc_type);
            "#,
        )?;
        if fts5 {
            conn.execute_batch(
                r#"
                CREATE VIRTUAL TABLE IF NOT EXISTS doc_fts USING fts5(
                    app_id UNINDEXED,
                    doc_type UNINDEXED,
                    name UNINDEXED,
                    title,
                    summary,
                    body,
                    tokenize='unicode61 remove_diacritics 2'
                );
                "#,
            )?;
        }
        Ok(())
    }

    pub fn count(&self) -> Result<u64, IndexError> {
        let conn = self.conn.lock().expect("docs index mutex poisoned");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM doc_entries", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// Rebuild the index from scratch using the filesystem store as source of truth.
    pub fn rebuild_from_fs(&self, store: &Store) -> Result<u64, IndexError> {
        let entries = walk_entries(&store.root)?;
        {
            let conn = self.conn.lock().expect("docs index mutex poisoned");
            conn.execute("DELETE FROM doc_entries", [])?;
            if self.fts5_available {
                conn.execute("DELETE FROM doc_fts", [])?;
            }
        }
        let mut indexed = 0u64;
        for (app_id, doc_type, name) in entries {
            match store.read_entry(&app_id, doc_type, &name) {
                Ok(entry) => {
                    self.upsert(&entry)?;
                    indexed += 1;
                }
                Err(e) => warn!(
                    app_id, doc_type = doc_type.as_str(), name, error = %e,
                    "Failed to read entry while rebuilding docs index"
                ),
            }
        }
        info!(indexed, "Docs index rebuild complete");
        Ok(indexed)
    }

    /// Upsert a single entry into both `doc_entries` and `doc_fts`.
    pub fn upsert(&self, entry: &DocEntry) -> Result<(), IndexError> {
        let conn = self.conn.lock().expect("docs index mutex poisoned");
        let fm = &entry.frontmatter;
        let code_refs_json = serde_json::to_string(&fm.code_refs).unwrap_or_else(|_| "[]".into());
        let links_json = serde_json::to_string(&fm.links).unwrap_or_else(|_| "[]".into());
        conn.execute(
            r#"
            INSERT INTO doc_entries
                (app_id, doc_type, name, title, summary, scope, parent_screen,
                 code_refs, links, has_diagram, body, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(app_id, doc_type, name) DO UPDATE SET
                title         = excluded.title,
                summary       = excluded.summary,
                scope         = excluded.scope,
                parent_screen = excluded.parent_screen,
                code_refs     = excluded.code_refs,
                links         = excluded.links,
                has_diagram   = excluded.has_diagram,
                body          = excluded.body,
                updated_at    = excluded.updated_at
            "#,
            params![
                entry.app_id,
                entry.doc_type.as_str(),
                entry.name,
                fm.title,
                fm.summary,
                fm.scope,
                fm.parent_screen,
                code_refs_json,
                links_json,
                fm.diagram as i64,
                entry.body,
                fm.updated_at,
            ],
        )?;
        if self.fts5_available {
            // Delete then insert (FTS5 doesn't support UPSERT).
            conn.execute(
                "DELETE FROM doc_fts WHERE app_id=?1 AND doc_type=?2 AND name=?3",
                params![entry.app_id, entry.doc_type.as_str(), entry.name],
            )?;
            conn.execute(
                "INSERT INTO doc_fts (app_id, doc_type, name, title, summary, body) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.app_id,
                    entry.doc_type.as_str(),
                    entry.name,
                    fm.title.clone().unwrap_or_default(),
                    fm.summary.clone().unwrap_or_default(),
                    entry.body,
                ],
            )?;
        }
        Ok(())
    }

    pub fn remove(
        &self,
        app_id: &str,
        doc_type: DocType,
        name: &str,
    ) -> Result<(), IndexError> {
        let conn = self.conn.lock().expect("docs index mutex poisoned");
        conn.execute(
            "DELETE FROM doc_entries WHERE app_id=?1 AND doc_type=?2 AND name=?3",
            params![app_id, doc_type.as_str(), name],
        )?;
        if self.fts5_available {
            conn.execute(
                "DELETE FROM doc_fts WHERE app_id=?1 AND doc_type=?2 AND name=?3",
                params![app_id, doc_type.as_str(), name],
            )?;
        }
        Ok(())
    }

    pub fn remove_app(&self, app_id: &str) -> Result<(), IndexError> {
        let conn = self.conn.lock().expect("docs index mutex poisoned");
        conn.execute("DELETE FROM doc_entries WHERE app_id=?1", params![app_id])?;
        if self.fts5_available {
            conn.execute("DELETE FROM doc_fts WHERE app_id=?1", params![app_id])?;
        }
        Ok(())
    }

    /// Full-text search. If `app_id` is given, results are scoped to that app. `doc_type`
    /// optionally filters by type. Limit defaults to 20, capped at 100.
    pub fn search(
        &self,
        query: &str,
        app_id: Option<&str>,
        doc_type: Option<DocType>,
        limit: Option<u32>,
    ) -> Result<Vec<SearchHit>, IndexError> {
        let limit = limit.unwrap_or(20).min(100) as i64;
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        if self.fts5_available {
            self.search_fts5(query, app_id, doc_type, limit)
        } else {
            self.search_like(query, app_id, doc_type, limit)
        }
    }

    fn search_fts5(
        &self,
        query: &str,
        app_id: Option<&str>,
        doc_type: Option<DocType>,
        limit: i64,
    ) -> Result<Vec<SearchHit>, IndexError> {
        let safe_query = sanitize_fts5_query(query);
        let conn = self.conn.lock().expect("docs index mutex poisoned");
        let mut sql = String::from(
            "SELECT app_id, doc_type, name, title, \
             snippet(doc_fts, 5, '<mark>', '</mark>', '…', 12) AS snip, \
             bm25(doc_fts) AS score \
             FROM doc_fts WHERE doc_fts MATCH ?1",
        );
        let mut binds: Vec<String> = vec![safe_query];
        let mut idx = 2;
        if let Some(a) = app_id {
            sql.push_str(&format!(" AND app_id = ?{idx}"));
            binds.push(a.to_string());
            idx += 1;
        }
        if let Some(t) = doc_type {
            sql.push_str(&format!(" AND doc_type = ?{idx}"));
            binds.push(t.as_str().to_string());
            // idx not reused
            let _ = idx;
        }
        sql.push_str(&format!(" ORDER BY score LIMIT {limit}"));

        let mut stmt = conn.prepare(&sql)?;
        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_dyn.as_slice(), |row| {
            Ok(SearchHit {
                app_id: row.get::<_, String>(0)?,
                doc_type: row.get::<_, String>(1)?,
                name: row.get::<_, String>(2)?,
                title: row.get::<_, Option<String>>(3)?,
                snippet: row.get::<_, String>(4)?,
                score: row.get::<_, f64>(5)?,
                matched_field: "fts".to_string(),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn search_like(
        &self,
        query: &str,
        app_id: Option<&str>,
        doc_type: Option<DocType>,
        limit: i64,
    ) -> Result<Vec<SearchHit>, IndexError> {
        let needle = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let conn = self.conn.lock().expect("docs index mutex poisoned");
        let mut sql = String::from(
            "SELECT app_id, doc_type, name, title, summary, body \
             FROM doc_entries \
             WHERE (LOWER(title) LIKE LOWER(?1) ESCAPE '\\' \
                    OR LOWER(summary) LIKE LOWER(?1) ESCAPE '\\' \
                    OR LOWER(body) LIKE LOWER(?1) ESCAPE '\\')",
        );
        let mut binds: Vec<String> = vec![needle];
        let mut idx = 2;
        if let Some(a) = app_id {
            sql.push_str(&format!(" AND app_id = ?{idx}"));
            binds.push(a.to_string());
            idx += 1;
        }
        if let Some(t) = doc_type {
            sql.push_str(&format!(" AND doc_type = ?{idx}"));
            binds.push(t.as_str().to_string());
        }
        sql.push_str(&format!(" LIMIT {limit}"));
        let mut stmt = conn.prepare(&sql)?;
        let params_dyn: Vec<&dyn rusqlite::ToSql> =
            binds.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let q_lower = query.to_lowercase();
        let rows = stmt.query_map(params_dyn.as_slice(), |row| {
            let app_id: String = row.get(0)?;
            let doc_type: String = row.get(1)?;
            let name: String = row.get(2)?;
            let title: Option<String> = row.get(3)?;
            let summary: Option<String> = row.get(4)?;
            let body: String = row.get(5)?;
            let (snip, field) = make_like_snippet(&title, &summary, &body, &q_lower);
            Ok(SearchHit {
                app_id,
                doc_type,
                name,
                title,
                score: 0.0,
                snippet: snip,
                matched_field: field,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

fn check_fts5(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM pragma_compile_options() WHERE compile_options LIKE 'ENABLE_FTS5%'",
        [],
        |_| Ok(()),
    )
    .optional()
    .unwrap_or(None)
    .is_some()
}

/// Sanitize a user query for FTS5 MATCH: escape special chars and quote tokens.
/// We split on whitespace, drop tokens shorter than 2 chars, and wrap each token in double quotes
/// (which makes them literal phrase tokens in FTS5). This avoids syntax errors on user input
/// containing `:`, `-`, `(`, `*` etc., while keeping reasonable matching behavior.
fn sanitize_fts5_query(q: &str) -> String {
    let parts: Vec<String> = q
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| {
            let cleaned: String = t
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, '\'' | '-' | '_' | '.'))
                .collect();
            format!("\"{}\"", cleaned.replace('"', ""))
        })
        .filter(|t| t.len() > 2) // skip the empty `""` case
        .collect();
    if parts.is_empty() {
        // Fallback: a token that won't match anything.
        return "\"____nonematch____\"".to_string();
    }
    parts.join(" ")
}

fn make_like_snippet(
    title: &Option<String>,
    summary: &Option<String>,
    body: &str,
    needle_lower: &str,
) -> (String, String) {
    for (text, field) in [
        (title.as_deref().unwrap_or(""), "title"),
        (summary.as_deref().unwrap_or(""), "summary"),
        (body, "body"),
    ] {
        let lower = text.to_lowercase();
        if let Some(pos) = lower.find(needle_lower) {
            let start = pos.saturating_sub(40);
            let end = (pos + needle_lower.len() + 40).min(text.len());
            let mut s = start;
            while !text.is_char_boundary(s) && s > 0 {
                s -= 1;
            }
            let mut e = end;
            while !text.is_char_boundary(e) && e < text.len() {
                e += 1;
            }
            let mut out = String::new();
            if s > 0 {
                out.push('…');
            }
            out.push_str(&text[s..e]);
            if e < text.len() {
                out.push('…');
            }
            return (out, field.to_string());
        }
    }
    (
        body.chars().take(120).collect::<String>(),
        "body".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DocEntry, Frontmatter};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp_index_path() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("atelier-docs-idx-{pid}-{n}.sqlite"))
    }

    fn make_entry(app_id: &str, doc_type: DocType, name: &str, body: &str) -> DocEntry {
        let mut fm = Frontmatter::default();
        fm.title = Some(format!("{name} title"));
        fm.summary = Some(format!("{name} summary"));
        DocEntry {
            app_id: app_id.into(),
            doc_type,
            name: name.into(),
            frontmatter: fm,
            body: body.into(),
        }
    }

    #[test]
    fn fts5_check_runs() {
        let path = tmp_index_path();
        let _ = std::fs::remove_file(&path);
        let idx = Index::open(&path).unwrap();
        // We expect FTS5 to be available with rusqlite bundled, but the test should not
        // fail either way: fallback path is exercised in `search_basic`.
        let _ = idx.fts5_available;
    }

    #[test]
    fn upsert_and_search() {
        let path = tmp_index_path();
        let _ = std::fs::remove_file(&path);
        let idx = Index::open(&path).unwrap();
        let e1 = make_entry(
            "home",
            DocType::Feature,
            "auth-login",
            "Connexion via OAuth Google et email/password",
        );
        let e2 = make_entry(
            "home",
            DocType::Feature,
            "search",
            "Recherche dans la liste des apps installées",
        );
        idx.upsert(&e1).unwrap();
        idx.upsert(&e2).unwrap();
        let hits = idx.search("oauth", Some("home"), None, None).unwrap();
        assert!(hits.iter().any(|h| h.name == "auth-login"));
        let hits = idx
            .search("apps", Some("home"), Some(DocType::Feature), None)
            .unwrap();
        assert!(hits.iter().any(|h| h.name == "search"));
    }

    #[test]
    fn remove_works() {
        let path = tmp_index_path();
        let _ = std::fs::remove_file(&path);
        let idx = Index::open(&path).unwrap();
        let e = make_entry("home", DocType::Screen, "home", "Some unique-keyword-X here.");
        idx.upsert(&e).unwrap();
        assert!(!idx
            .search("unique-keyword-X", Some("home"), None, None)
            .unwrap()
            .is_empty());
        idx.remove("home", DocType::Screen, "home").unwrap();
        assert!(idx
            .search("unique-keyword-X", Some("home"), None, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn empty_query_returns_nothing() {
        let path = tmp_index_path();
        let _ = std::fs::remove_file(&path);
        let idx = Index::open(&path).unwrap();
        let hits = idx.search("   ", None, None, None).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn rebuild_from_fs_works() {
        use crate::fs::Store;
        let dir = std::env::temp_dir().join(format!(
            "atelier-docs-rebuild-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        store
            .write_meta("home", &crate::model::Meta::new("home"))
            .unwrap();
        let mut fm = Frontmatter::default();
        fm.title = Some("Login".into());
        fm.summary = Some("Logging in".into());
        store
            .write_entry(
                "home",
                DocType::Feature,
                "auth-login",
                fm,
                "OAuth Google",
            )
            .unwrap();

        let idx_path = dir.join("_index.sqlite");
        let _ = std::fs::remove_file(&idx_path);
        let idx = Index::open(&idx_path).unwrap();
        let n = idx.rebuild_from_fs(&store).unwrap();
        assert_eq!(n, 1);
        let hits = idx.search("oauth", Some("home"), None, None).unwrap();
        assert!(hits.iter().any(|h| h.name == "auth-login"));
    }
}
