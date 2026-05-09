//! Library used by both the `hr-dataverse-migrate` CLI and the
//! `hr-orchestrator` to migrate one app's DB from SQLite legacy to
//! Postgres dataverse.
//!
//! Entry points:
//! - [`migrate_with_manager`] — high-level: caller already has a
//!   `DataverseManager` (typical for hr-orchestrator). Handles two
//!   cases: fresh provisioning, OR adopting an existing PG database
//!   (whose password is reset via `ALTER ROLE`).
//! - [`MigrationReport`] — what was copied, plus the secret that was
//!   persisted via the manager's secrets store.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value as JsonValue;
use sqlx_core::arguments::Arguments;
use sqlx_core::types::Json as SqlxJson;
use sqlx_postgres::PgArguments;
use tracing::{info, warn};

use hr_dataverse::{
    DataverseEngine, DataverseManager, FieldType as PgFieldType, ProvisioningResult,
};

#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// (table_name, copied_row_count) for each migrated table.
    pub copied: Vec<(String, i64)>,
    /// Tables intentionally skipped (e.g. Formula columns that need
    /// manual recreation). V1: empty.
    pub skipped: Vec<String>,
    /// The secret persisted by the manager. Caller can read it back
    /// from the secrets store; we surface it here for convenience.
    pub secret: ProvisioningResult,
    /// Whether we adopted an existing PG database vs provisioning fresh.
    pub adopted_existing: bool,
}

/// Migrate `slug`'s SQLite DB to Postgres using the caller's manager.
///
/// Side-effects on success:
/// - `app_{slug}` Postgres database exists and contains all rows from
///   `sqlite_path`.
/// - The manager has persisted the per-app secret to its secrets file
///   (`/opt/homeroute/data/dataverse-secrets.json` in production).
/// - The manager has cached the engine (and a DSN override) so the
///   first MCP call will hit the new pool without an extra round trip.
pub async fn migrate_with_manager(
    manager: &DataverseManager,
    slug: &str,
    sqlite_path: &Path,
) -> Result<MigrationReport> {
    if !sqlite_path.exists() {
        bail!("source sqlite not found: {}", sqlite_path.display());
    }

    info!(slug, source = %sqlite_path.display(), "reading source schema");
    let (tables, relations) = read_sqlite_schema(sqlite_path)
        .with_context(|| format!("read schema from {}", sqlite_path.display()))?;

    info!(
        tables = tables.len(),
        relations = relations.len(),
        "source schema loaded"
    );
    for t in &tables {
        info!("  - {} ({} cols)", t.name, t.columns.len());
    }

    // Detect which tables use UUID-shaped primary keys. Those tables
    // get created with [`IdStrategy::Uuid`] in the dataverse engine —
    // the original `id` strings are preserved during the copy so any
    // external links (URLs, mobile sync state, FK references) stay
    // valid. Tables without UUID PKs use the default Bigserial
    // strategy and let PG renumber.
    let uuid_pk_tables = detect_uuid_pk_tables(sqlite_path, &tables)?;
    if !uuid_pk_tables.is_empty() {
        info!(
            count = uuid_pk_tables.len(),
            "detected UUID primary keys — these tables will use IdStrategy::Uuid"
        );
        for t in &uuid_pk_tables {
            info!("  - {} (UUID PK)", t);
        }
    }

    // ── Fresh provisioning OR adopt existing PG ──────────────────────
    let (secret, adopted_existing) = if manager.exists(slug).await? {
        info!(slug, "PG database already exists — adopting (resetting role password)");
        let s = manager
            .adopt_existing(slug)
            .await
            .with_context(|| format!("adopt existing PG for slug {}", slug))?;
        (s, true)
    } else {
        info!(slug, "provisioning fresh PG database");
        let s = manager.provision(slug).await.context("provision")?;
        (s, false)
    };

    // The manager persists the secret to its secrets file on its own
    // (`provision` and `adopt_existing` both do). We register it as
    // an in-memory override so subsequent `engine_for` calls don't
    // need to re-read the file.
    manager.set_dsn_override(slug, secret.dsn.clone()).await;

    let outcome =
        do_migrate(manager, slug, &tables, &relations, sqlite_path, &uuid_pk_tables).await;

    let report = match outcome {
        Ok(report) => report,
        Err(e) => {
            // Best-effort: leave PG state alone (caller can decide to
            // drop / retry). Surface the error.
            return Err(e);
        }
    };

    Ok(MigrationReport {
        copied: report.copied,
        skipped: report.skipped,
        secret,
        adopted_existing,
    })
}

#[derive(Debug)]
struct InternalReport {
    copied: Vec<(String, i64)>,
    skipped: Vec<String>,
}

async fn do_migrate(
    manager: &DataverseManager,
    slug: &str,
    tables: &[SqliteTable],
    relations: &[SqliteRelation],
    sqlite_path: &Path,
    uuid_pk_tables: &std::collections::HashSet<String>,
) -> Result<InternalReport> {
    let engine = manager
        .engine_for(slug)
        .await
        .context("open dataverse engine")?;

    // ── 1. Recreate tables (skip those that already exist when adopting) ─
    let existing: std::collections::HashSet<String> = engine
        .list_tables()
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    for table in tables {
        if existing.contains(&table.name) {
            info!(
                table = %table.name,
                "table already exists in PG — skipping create_table (adoption mode)"
            );
            continue;
        }
        let id_strategy = if uuid_pk_tables.contains(&table.name) {
            hr_dataverse::IdStrategy::Uuid
        } else {
            hr_dataverse::IdStrategy::Bigserial
        };
        let def = build_table_definition(table, id_strategy)?;
        info!(
            table = %table.name,
            cols = def.columns.len(),
            id_strategy = ?id_strategy,
            "create_table"
        );
        engine
            .create_table(&def)
            .await
            .with_context(|| format!("create_table {}", table.name))?;
    }

    // ── 2. Skip FK constraints during migration ──────────────────────
    // Source IDs are NOT preserved (PG assigns BIGSERIAL fresh), so any
    // child.parent_id value would point to a row whose primary key has
    // changed in the target. Adding FK constraints before the copy
    // would block valid (if stale-pointer-bearing) INSERTs.
    //
    // Trade-off: the per-app PG database carries no relational
    // integrity at the storage layer. Lookup metadata still lives in
    // `_dv_columns` (lookup_target) — but with `lookup_target=None`
    // forced by `build_table_definition` for migration, that field is
    // empty here. Operators who want to restore FKs post-migration
    // can call `db.create_relation` once the agent has refactored the
    // code to recompute consistent IDs (or migrate IDs in place).
    let _unused_relations = relations;
    info!(
        "skipping {} foreign-key constraints during migration — \
         restore manually post-cleanup if integrity is required",
        relations.len()
    );

    // ── 3. Copy rows table by table ──────────────────────────────────
    //     If a table already has rows in PG (adoption with existing
    //     copies), we skip the copy and just validate counts.
    let mut copied: Vec<(String, i64)> = Vec::with_capacity(tables.len());
    let skipped: Vec<String> = Vec::new();

    for table in tables {
        let existing_count = engine.count_rows(&table.name).await.unwrap_or(0);
        let sqlite_count = sqlite_table_count(sqlite_path, &table.name)?;

        if existing_count == sqlite_count && existing_count > 0 {
            info!(
                table = %table.name,
                count = existing_count,
                "PG already has matching row count — skipping copy"
            );
            copied.push((table.name.clone(), existing_count));
            continue;
        }
        if existing_count > 0 && existing_count != sqlite_count {
            bail!(
                "PG table `{}` has {} rows but SQLite has {} — refusing to copy on top of \
                 partial state. Drop the PG table or use db.rollback_migration first.",
                table.name, existing_count, sqlite_count
            );
        }

        let preserve_id = uuid_pk_tables.contains(&table.name);
        let n = copy_rows(&engine, sqlite_path, table, preserve_id)
            .await
            .with_context(|| format!("copy rows of {}", table.name))?;
        copied.push((table.name.clone(), n));
    }

    // ── 4. Validate counts ───────────────────────────────────────────
    for (name, expected) in &copied {
        let actual = engine.count_rows(name).await?;
        if actual != *expected {
            bail!(
                "row count mismatch on {}: expected={}, count_rows={}",
                name, expected, actual
            );
        }
    }

    Ok(InternalReport { copied, skipped })
}

// ---------------------------------------------------------------------
// Internal types mirroring the legacy SQLite `_dv_*` shape
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SqliteColumn {
    name: String,
    field_type: String,
    required: bool,
    is_unique: bool,
    default_value: Option<String>,
    description: Option<String>,
    choices: Vec<String>,
    formula_expression: Option<String>,
}

#[derive(Debug, Clone)]
struct SqliteTable {
    name: String,
    description: Option<String>,
    columns: Vec<SqliteColumn>,
}

#[derive(Debug, Clone)]
struct SqliteRelation {
    from_table: String,
    from_column: String,
    to_table: String,
    to_column: String,
    relation_type: String,
    on_delete: String,
    on_update: String,
}

// ---------------------------------------------------------------------
// SQLite schema reading
// ---------------------------------------------------------------------

fn read_sqlite_schema(path: &Path) -> Result<(Vec<SqliteTable>, Vec<SqliteRelation>)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let has_dv_tables: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_dv_tables')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_dv_tables {
        bail!(
            "no `_dv_tables` metadata found in {} — this app was never managed by hr-db, \
             nothing to migrate (or migrate raw schema by hand)",
            path.display()
        );
    }

    let mut tstmt = conn.prepare("SELECT name, description FROM _dv_tables ORDER BY name")?;
    let trows = tstmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;

    let mut tables: Vec<SqliteTable> = Vec::new();
    for row in trows {
        let (name, description) = row?;
        let columns = read_columns(&conn, &name)?;
        tables.push(SqliteTable { name, description, columns });
    }

    let mut rstmt = conn.prepare(
        "SELECT from_table, from_column, to_table, to_column, relation_type, on_delete, on_update \
         FROM _dv_relations ORDER BY id",
    )?;
    let rrows = rstmt.query_map([], |r| {
        Ok(SqliteRelation {
            from_table: r.get(0)?,
            from_column: r.get(1)?,
            to_table: r.get(2)?,
            to_column: r.get(3)?,
            relation_type: r.get(4)?,
            on_delete: r.get(5)?,
            on_update: r.get(6)?,
        })
    })?;
    let relations: Vec<SqliteRelation> = rrows.filter_map(|r| r.ok()).collect();

    Ok((tables, relations))
}

fn read_columns(conn: &Connection, table: &str) -> Result<Vec<SqliteColumn>> {
    let mut stmt = conn.prepare(
        "SELECT name, field_type, required, is_unique, default_value, description, \
                choices, formula_expression \
         FROM _dv_columns WHERE table_name = ?1 ORDER BY position, name",
    )?;
    let rows = stmt.query_map([table], |r| {
        let choices_json: Option<String> = r.get(6)?;
        let choices: Vec<String> = choices_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        Ok(SqliteColumn {
            name: r.get(0)?,
            field_type: r.get::<_, String>(1)?,
            required: r.get(2)?,
            is_unique: r.get(3)?,
            default_value: r.get(4)?,
            description: r.get(5)?,
            choices,
            formula_expression: r.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn sqlite_table_count(path: &Path, table: &str) -> Result<i64> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let n: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{}\"", table), [], |r| r.get(0))?;
    Ok(n)
}

// ---------------------------------------------------------------------
// Schema translation
// ---------------------------------------------------------------------

fn build_table_definition(
    t: &SqliteTable,
    id_strategy: hr_dataverse::IdStrategy,
) -> Result<hr_dataverse::TableDefinition> {
    let now = Utc::now();
    let mut cols: Vec<hr_dataverse::ColumnDefinition> = Vec::with_capacity(t.columns.len());
    for c in &t.columns {
        if matches!(c.name.as_str(), "id" | "created_at" | "updated_at") {
            continue;
        }
        let ft = parse_field_type(&c.field_type)?;
        if matches!(ft, PgFieldType::Formula) {
            warn!(
                table = %t.name,
                column = %c.name,
                "skipping Formula column — recreate it manually with formula_expression"
            );
            continue;
        }
        cols.push(hr_dataverse::ColumnDefinition {
            name: c.name.clone(),
            field_type: ft,
            // **Constraints are deliberately relaxed during migration.**
            // Source SQLite data routinely violates the schema declared
            // in `_dv_columns` (e.g. trader's `virtual_positions.entry_date`
            // is NOT NULL but unparseable values get dropped to NULL by
            // our tolerant Date parser; or `is_unique` columns have
            // duplicate historical rows). Forcing NOT NULL / UNIQUE at
            // table-creation time would block legitimate data copies.
            // The metadata still describes the *intended* shape; the
            // operator re-applies constraints manually post-cleanup
            // (`ALTER TABLE … ALTER COLUMN … SET NOT NULL` etc.).
            required: false,
            unique: false,
            default_value: c.default_value.as_deref().map(|d| translate_sqlite_default(d, ft)),
            description: c.description.clone(),
            choices: c.choices.clone(),
            formula_expression: c.formula_expression.clone(),
            lookup_target: None,
        });
    }
    Ok(hr_dataverse::TableDefinition {
        name: t.name.clone(),
        slug: t.name.clone(),
        columns: cols,
        description: t.description.clone(),
        id_strategy,
        created_at: now,
        updated_at: now,
    })
}

fn parse_field_type(s: &str) -> Result<PgFieldType> {
    PgFieldType::from_code(s).ok_or_else(|| anyhow!("unknown field_type code in legacy DB: {}", s))
}

/// Inspect a sample of `id` values in each user table and return the
/// set of tables whose `id` column looks UUID-shaped. Those tables are
/// migrated with [`hr_dataverse::IdStrategy::Uuid`] so the original
/// UUID values survive — keeping any external links (URLs, mobile
/// sync state, FK references in other columns) valid.
fn detect_uuid_pk_tables(
    sqlite_path: &Path,
    tables: &[SqliteTable],
) -> Result<std::collections::HashSet<String>> {
    let conn = Connection::open_with_flags(sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let uuid_re = regex_uuid();

    let mut found: std::collections::HashSet<String> = std::collections::HashSet::new();

    for t in tables {
        // SQLite stores `id` as whatever type was declared. We sample
        // up to 5 non-null values and see if they look like UUIDs.
        let sql = format!("SELECT id FROM \"{}\" WHERE id IS NOT NULL LIMIT 5", t.name);
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => continue, // table may not have an `id` column
        };
        let mut rows = match stmt.query([]) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut seen_uuid = 0usize;
        let mut seen_total = 0usize;
        while let Ok(Some(row)) = rows.next() {
            seen_total += 1;
            if let Ok(s) = row.get::<_, String>(0) {
                if uuid_re.is_match(s.trim()) {
                    seen_uuid += 1;
                }
            }
        }
        // Majority-vote heuristic: more than half of sampled values
        // look like UUIDs → flag the table.
        if seen_total >= 1 && seen_uuid * 2 >= seen_total {
            found.insert(t.name.clone());
        }
    }

    Ok(found)
}

fn regex_uuid() -> SimpleUuidPattern {
    // We don't depend on the `regex` crate; a hand-rolled matcher
    // covers the canonical UUID shape (36 chars, 8-4-4-4-12 hex).
    SimpleUuidPattern
}

/// Lightweight UUID matcher — avoids pulling the `regex` crate just
/// for one heuristic.
struct SimpleUuidPattern;
impl SimpleUuidPattern {
    fn is_match(&self, s: &str) -> bool {
        let bytes = s.as_bytes();
        if bytes.len() != 36 {
            return false;
        }
        for (i, b) in bytes.iter().enumerate() {
            let is_hyphen_position = matches!(i, 8 | 13 | 18 | 23);
            if is_hyphen_position {
                if *b != b'-' {
                    return false;
                }
            } else if !b.is_ascii_hexdigit() {
                return false;
            }
        }
        true
    }
}

/// Translate a `default_value` literal from SQLite-flavored syntax (as
/// stored in the legacy `_dv_columns.default_value`) to a Postgres
/// equivalent that can be spliced verbatim into a `DEFAULT …` clause.
///
/// Idempotent: values already in PG form (e.g. `now()`, `true`,
/// `'hello'`) pass through unchanged.
///
/// Common SQLite → PG translations covered:
/// - `datetime('now')` (and variants) → `now()` for DateTime columns,
///   `now()::text` for Text columns
/// - `1` / `0` for Boolean columns → `true` / `false`
/// - bare strings for Text/Choice/Email/etc. → SQL-quoted `'string'`
///   (with internal quotes escaped)
/// - numeric and Json values pass through
fn translate_sqlite_default(raw: &str, ft: PgFieldType) -> String {
    use PgFieldType::*;
    let v = raw.trim();
    let lower = v.to_lowercase();

    // Recognise SQLite's "datetime('now')" family — also accept
    // already-translated `now()` for idempotence.
    let is_now_call = lower.contains("datetime('now')")
        || lower.contains("datetime(\"now\")")
        || lower.contains("date('now')")
        || lower.contains("time('now')")
        || lower == "now()"
        || lower == "current_timestamp";

    match ft {
        Boolean => match lower.as_str() {
            "1" | "true" | "'true'" => "true".into(),
            "0" | "false" | "'false'" => "false".into(),
            _ => v.to_string(),
        },
        DateTime | Date | Time => {
            if is_now_call {
                "now()".into()
            } else {
                v.to_string()
            }
        }
        Text | Email | Url | Phone | Choice | Duration => {
            if is_now_call {
                // SQLite stored datetime as TEXT — keep it as TEXT in PG.
                "now()::text".into()
            } else if v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2 {
                // already SQL-quoted
                v.to_string()
            } else if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
                // double-quoted (SQL identifier syntax) — re-quote as string
                format!("'{}'", v[1..v.len() - 1].replace('\'', "''"))
            } else {
                // bare token — wrap as SQL string literal
                format!("'{}'", v.replace('\'', "''"))
            }
        }
        Number | AutoIncrement | Lookup | Decimal | Currency | Percent | Json | Uuid
        | MultiChoice | Formula => v.to_string(),
    }
}

fn build_relation(r: &SqliteRelation) -> Result<hr_dataverse::RelationDefinition> {
    use hr_dataverse::{CascadeAction, CascadeRules, RelationType};
    let rt = match r.relation_type.as_str() {
        "one_to_many" => RelationType::OneToMany,
        "many_to_many" => RelationType::ManyToMany,
        "self_referential" => RelationType::SelfReferential,
        other => bail!("unknown relation_type in legacy DB: {}", other),
    };
    let parse_act = |s: &str| {
        CascadeAction::from_code(s)
            .ok_or_else(|| anyhow!("unknown cascade action: {}", s))
    };
    Ok(hr_dataverse::RelationDefinition {
        from_table: r.from_table.clone(),
        from_column: r.from_column.clone(),
        to_table: r.to_table.clone(),
        to_column: r.to_column.clone(),
        relation_type: rt,
        cascade: CascadeRules {
            on_delete: parse_act(&r.on_delete)?,
            on_update: parse_act(&r.on_update)?,
        },
    })
}

// ---------------------------------------------------------------------
// Row copy
// ---------------------------------------------------------------------

async fn copy_rows(
    engine: &DataverseEngine,
    sqlite_path: &Path,
    table: &SqliteTable,
    preserve_id: bool,
) -> Result<i64> {
    // ── 1. Read all rows from SQLite into memory FIRST and close the
    //       connection. rusqlite's `Connection` is `!Send` so we can't
    //       hold it across the async PG inserts below — buffer up
    //       front, then drop. For typical Dataverse-managed tables
    //       (thousands of rows, modest column counts) this is fine.
    let cols_to_copy_owned: Vec<SqliteColumn> = table
        .columns
        .iter()
        .filter(|c| {
            !matches!(c.name.as_str(), "id" | "created_at" | "updated_at")
                && !matches!(
                    parse_field_type(&c.field_type).ok(),
                    Some(PgFieldType::Formula)
                )
        })
        .cloned()
        .collect();

    let (rows, has_created_at, has_updated_at, has_id) =
        read_sqlite_rows(sqlite_path, &table.name, &cols_to_copy_owned, preserve_id)?;

    // ── 2. Insert into PG, batched (async, no SQLite borrow held) ───
    // Single-row INSERTs cost a network round-trip per row. For tables
    // with > 100k rows (e.g. trader's market_data_cache) that would
    // take minutes and risk client timeouts during MCP calls. Batch up
    // to BATCH_SIZE rows per INSERT — Postgres handles this well and
    // it cuts wall-time roughly 50-100×.
    const BATCH_SIZE: usize = 500;
    let cols_to_copy: Vec<&SqliteColumn> = cols_to_copy_owned.iter().collect();
    let mut total: i64 = 0;
    for chunk in rows.chunks(BATCH_SIZE) {
        insert_rows_batch(
            engine,
            &table.name,
            &cols_to_copy,
            chunk,
            has_created_at,
            has_updated_at,
            has_id,
        )
        .await?;
        total += chunk.len() as i64;
    }
    Ok(total)
}

/// Synchronous helper: opens the SQLite DB, reads all rows of `table`
/// projected on the given columns + optional implicit timestamps,
/// returns them as JSON values along with which timestamps exist.
///
/// When `preserve_id` is true, the `id` column is also selected (so the
/// caller can preserve its value during INSERT — used for UUID-PK
/// tables where the original UUIDs must survive migration).
fn read_sqlite_rows(
    sqlite_path: &Path,
    table: &str,
    cols_to_copy: &[SqliteColumn],
    preserve_id: bool,
) -> Result<(Vec<HashMap<String, JsonValue>>, bool, bool, bool)> {
    let conn = Connection::open_with_flags(sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_created_at = sqlite_has_column(&conn, table, "created_at")?;
    let has_updated_at = sqlite_has_column(&conn, table, "updated_at")?;
    let has_id = preserve_id && sqlite_has_column(&conn, table, "id")?;

    let mut select_cols: Vec<String> = Vec::new();
    if has_id { select_cols.push("id".to_string()); }
    if has_created_at { select_cols.push("created_at".to_string()); }
    if has_updated_at { select_cols.push("updated_at".to_string()); }
    for c in cols_to_copy {
        select_cols.push(c.name.clone());
    }
    let select_sql = format!(
        "SELECT {} FROM \"{}\"",
        select_cols
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", "),
        table
    );

    let mut stmt = conn.prepare(&select_sql)?;
    let n_cols = stmt.column_count();
    let mut iter = stmt.query([])?;
    let mut rows: Vec<HashMap<String, JsonValue>> = Vec::new();
    while let Some(row) = iter.next()? {
        let mut values: HashMap<String, JsonValue> = HashMap::with_capacity(n_cols);
        for (i, col_name) in select_cols.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i)?;
            values.insert(col_name.clone(), sqlite_to_json(v));
        }
        rows.push(values);
    }
    Ok((rows, has_created_at, has_updated_at, has_id))
}

fn sqlite_has_column(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{}\")", table))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let name: String = r.get(1)?;
        if name == col {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Multi-row INSERT (`INSERT … VALUES (row1), (row2), …`) for the
/// supplied batch. Cuts down round-trips dramatically vs single-row
/// inserts. Each placeholder gets the same column-type cast as the
/// single-row path, so the Decimal/Numeric handling stays identical.
async fn insert_rows_batch(
    engine: &DataverseEngine,
    table: &str,
    cols: &[&SqliteColumn],
    batch: &[HashMap<String, JsonValue>],
    has_created_at: bool,
    has_updated_at: bool,
    has_id: bool,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    // Build the column list once.
    let mut col_names: Vec<String> = Vec::with_capacity(cols.len() + 3);
    let mut col_casts: Vec<&'static str> = Vec::with_capacity(cols.len() + 3);
    // `id` first so its placeholder casts cleanly to `::UUID`.
    if has_id { col_names.push("id".into()); col_casts.push("::UUID"); }
    if has_created_at { col_names.push("created_at".into()); col_casts.push(""); }
    if has_updated_at { col_names.push("updated_at".into()); col_casts.push(""); }
    for c in cols {
        col_names.push(c.name.clone());
        let ft = parse_field_type(&c.field_type)?;
        col_casts.push(match ft {
            PgFieldType::Decimal | PgFieldType::Currency | PgFieldType::Percent => "::NUMERIC",
            _ => "",
        });
    }
    let cols_per_row = col_names.len();

    // Each row contributes `cols_per_row` placeholders. They are
    // numbered globally so all binds line up: `($1, $2), ($3, $4), …`.
    let mut row_groups: Vec<String> = Vec::with_capacity(batch.len());
    for row_ix in 0..batch.len() {
        let mut placeholders: Vec<String> = Vec::with_capacity(cols_per_row);
        for col_ix in 0..cols_per_row {
            let n = row_ix * cols_per_row + col_ix + 1;
            placeholders.push(format!("${}{}", n, col_casts[col_ix]));
        }
        row_groups.push(format!("({})", placeholders.join(", ")));
    }

    let sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES {}",
        table,
        col_names
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", "),
        row_groups.join(", "),
    );

    let mut args = PgArguments::default();
    for values in batch {
        if has_id {
            // `id` arrives as a TEXT-shaped UUID. Bind as String — the
            // `::UUID` cast in the placeholder converts it server-side.
            // A NULL id forces Postgres to fall back to the default
            // (gen_random_uuid()) which is fine.
            let v = values.get("id").cloned().unwrap_or(JsonValue::Null);
            match v {
                JsonValue::Null => args
                    .add(Option::<String>::None)
                    .map_err(map_bind)?,
                JsonValue::String(s) => args.add(s).map_err(map_bind)?,
                other => {
                    return Err(anyhow!(
                        "non-string id value while preserving UUID PK: {:?}",
                        other
                    ));
                }
            }
        }
        if has_created_at {
            bind_optional_dt(&mut args, "created_at", values)?;
        }
        if has_updated_at {
            bind_optional_dt(&mut args, "updated_at", values)?;
        }
        for c in cols {
            let ft = parse_field_type(&c.field_type)?;
            let v = values.get(&c.name).cloned().unwrap_or(JsonValue::Null);
            bind_for_field(&mut args, &v, ft)?;
        }
    }

    sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .execute(engine.pool())
        .await
        .with_context(|| format!("INSERT batch into {} ({} rows)", table, batch.len()))?;
    Ok(())
}

#[allow(dead_code)]
async fn insert_row(
    engine: &DataverseEngine,
    table: &str,
    cols: &[&SqliteColumn],
    values: &HashMap<String, JsonValue>,
    has_created_at: bool,
    has_updated_at: bool,
) -> Result<()> {
    let mut col_names: Vec<String> = Vec::with_capacity(cols.len() + 2);
    let mut col_casts: Vec<&'static str> = Vec::with_capacity(cols.len() + 2);
    if has_created_at { col_names.push("created_at".into()); col_casts.push(""); }
    if has_updated_at { col_names.push("updated_at".into()); col_casts.push(""); }
    for c in cols {
        col_names.push(c.name.clone());
        let ft = parse_field_type(&c.field_type)?;
        col_casts.push(match ft {
            PgFieldType::Decimal | PgFieldType::Currency | PgFieldType::Percent => "::NUMERIC",
            _ => "",
        });
    }

    let placeholders: Vec<String> = col_casts
        .iter()
        .enumerate()
        .map(|(i, cast)| format!("${}{}", i + 1, cast))
        .collect();
    let sql = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({})",
        table,
        col_names
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", "),
        placeholders.join(", "),
    );

    let mut args = PgArguments::default();
    if has_created_at {
        bind_optional_dt(&mut args, "created_at", values)?;
    }
    if has_updated_at {
        bind_optional_dt(&mut args, "updated_at", values)?;
    }
    for c in cols {
        let ft = parse_field_type(&c.field_type)?;
        let v = values.get(&c.name).cloned().unwrap_or(JsonValue::Null);
        bind_for_field(&mut args, &v, ft)?;
    }

    sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .execute(engine.pool())
        .await
        .with_context(|| format!("INSERT into {}", table))?;
    Ok(())
}

fn bind_optional_dt(
    args: &mut PgArguments,
    key: &str,
    values: &HashMap<String, JsonValue>,
) -> Result<()> {
    match values.get(key).and_then(|v| v.as_str()) {
        Some(s) => {
            let dt = parse_dt(s)?;
            args.add(dt).map_err(|e| anyhow!("bind {}: {}", key, e))?;
        }
        None => {
            args.add(Option::<chrono::DateTime<Utc>>::None)
                .map_err(|e| anyhow!("bind {}: {}", key, e))?;
        }
    }
    Ok(())
}

fn bind_for_field(args: &mut PgArguments, v: &JsonValue, ft: PgFieldType) -> Result<()> {
    use PgFieldType::*;
    match (ft, v) {
        (_, JsonValue::Null) => match ft {
            Boolean => args.add(Option::<bool>::None).map_err(map_bind)?,
            Number | AutoIncrement | Lookup => args.add(Option::<i64>::None).map_err(map_bind)?,
            Decimal | Currency | Percent => args.add(Option::<String>::None).map_err(map_bind)?,
            DateTime => args
                .add(Option::<chrono::DateTime<Utc>>::None)
                .map_err(map_bind)?,
            Date => args
                .add(Option::<chrono::NaiveDate>::None)
                .map_err(map_bind)?,
            Time => args
                .add(Option::<chrono::NaiveTime>::None)
                .map_err(map_bind)?,
            Json => args
                .add(Option::<SqlxJson<JsonValue>>::None)
                .map_err(map_bind)?,
            Uuid => args.add(Option::<uuid::Uuid>::None).map_err(map_bind)?,
            MultiChoice => args.add(Option::<Vec<String>>::None).map_err(map_bind)?,
            _ => args.add(Option::<String>::None).map_err(map_bind)?,
        },
        (Boolean, JsonValue::Bool(b)) => args.add(*b).map_err(map_bind)?,
        (Boolean, JsonValue::Number(n)) => {
            let b = n.as_i64().map(|i| i != 0).ok_or_else(|| anyhow!("bool: bad int"))?;
            args.add(b).map_err(map_bind)?
        }
        (Number | AutoIncrement | Lookup, JsonValue::Number(n)) => {
            let i = n.as_i64().ok_or_else(|| anyhow!("int: not i64"))?;
            args.add(i).map_err(map_bind)?
        }
        (Decimal | Currency | Percent, JsonValue::Number(n)) => {
            args.add(n.to_string()).map_err(map_bind)?
        }
        (Decimal | Currency | Percent, JsonValue::String(s)) => {
            args.add(s.clone()).map_err(map_bind)?
        }
        (DateTime, JsonValue::String(s)) => {
            // SQLite metadata sometimes mis-classifies columns
            // (a Date column actually carrying "market_closed" etc.).
            // Tolerate by binding NULL on parse failure rather than
            // failing the whole migration. The agent can fix this
            // post-cleanup.
            match parse_dt(s) {
                Ok(dt) => args.add(dt).map_err(map_bind)?,
                Err(_) => {
                    warn!("dropping unparseable DateTime value: {:?}", s);
                    args.add(Option::<chrono::DateTime<Utc>>::None).map_err(map_bind)?
                }
            }
        }
        (Date, JsonValue::String(s)) => {
            match chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                Ok(d) => args.add(d).map_err(map_bind)?,
                Err(_) => {
                    warn!("dropping unparseable Date value: {:?}", s);
                    args.add(Option::<chrono::NaiveDate>::None).map_err(map_bind)?
                }
            }
        }
        (Time, JsonValue::String(s)) => {
            let parsed = chrono::NaiveTime::parse_from_str(s, "%H:%M:%S")
                .or_else(|_| chrono::NaiveTime::parse_from_str(s, "%H:%M:%S%.f"));
            match parsed {
                Ok(t) => args.add(t).map_err(map_bind)?,
                Err(_) => {
                    warn!("dropping unparseable Time value: {:?}", s);
                    args.add(Option::<chrono::NaiveTime>::None).map_err(map_bind)?
                }
            }
        }
        (Uuid, JsonValue::String(s)) => {
            match uuid::Uuid::parse_str(s) {
                Ok(u) => args.add(u).map_err(map_bind)?,
                Err(_) => {
                    warn!("dropping unparseable UUID value: {:?}", s);
                    args.add(Option::<uuid::Uuid>::None).map_err(map_bind)?
                }
            }
        }
        (Json, v) => args.add(SqlxJson(v.clone())).map_err(map_bind)?,
        (MultiChoice, JsonValue::String(s)) => {
            let arr: Vec<String> = serde_json::from_str(s)
                .with_context(|| format!("MultiChoice: invalid JSON {}", s))?;
            args.add(arr).map_err(map_bind)?
        }
        (MultiChoice, JsonValue::Array(items)) => {
            let mut v: Vec<String> = Vec::with_capacity(items.len());
            for it in items {
                if let JsonValue::String(s) = it {
                    v.push(s.clone());
                } else {
                    bail!("MultiChoice element is not a string");
                }
            }
            args.add(v).map_err(map_bind)?
        }
        (_, JsonValue::String(s)) => args.add(s.clone()).map_err(map_bind)?,
        (_, JsonValue::Number(n)) => args.add(n.to_string()).map_err(map_bind)?,
        (_, JsonValue::Bool(b)) => args.add(b.to_string()).map_err(map_bind)?,
        (ft, other) => bail!("type mismatch: column type {:?} can't bind {:?}", ft, other),
    }
    Ok(())
}

fn map_bind(e: sqlx_core::error::BoxDynError) -> anyhow::Error {
    anyhow!("bind error: {}", e)
}

fn parse_dt(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|n| n.and_utc())
        })
        .with_context(|| format!("invalid datetime: {}", s))
}

fn sqlite_to_json(v: rusqlite::types::Value) -> JsonValue {
    use rusqlite::types::Value as V;
    match v {
        V::Null => JsonValue::Null,
        V::Integer(i) => JsonValue::Number(i.into()),
        V::Real(f) => serde_json::Number::from_f64(f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        V::Text(s) => JsonValue::String(s),
        V::Blob(b) => {
            let mut out = String::with_capacity(b.len() * 2);
            for byte in b {
                use std::fmt::Write;
                let _ = write!(&mut out, "{:02x}", byte);
            }
            JsonValue::String(out)
        }
    }
}
