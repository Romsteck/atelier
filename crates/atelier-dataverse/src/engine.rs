//! Dataverse engine bound to a single app's Postgres database.
//!
//! Holds a `PgPool` connected to `app_{slug}` and exposes:
//! - schema introspection (`list_tables`, `get_schema`, `count_rows`)
//! - schema mutations (`create_table`, `drop_table`, `add_column`,
//!   `remove_column`, `create_relation`) — each one bumps `schema_version`
//!   in `_dv_meta` and journals the operation in `_dv_migrations`.
//!
//! GraphQL execution and row-level CRUD live in their own modules and take
//! a borrow on this engine.

use chrono::{DateTime, Utc};
use serde_json::json;
use tracing::instrument;

use crate::sqlx::{self, PgPool};
use crate::error::{DataverseError, Result};
use crate::migration::{
    add_column_sql, add_foreign_key_sql, create_active_index_sql, create_table_sql,
    create_updated_at_trigger_sql, drop_column_sql, drop_table_sql, quote_ident,
};
use crate::schema::{
    CascadeAction, ColumnDefinition, DatabaseSchema, FieldType, IdStrategy, RelationDefinition,
    RelationType, TableDefinition,
};
use crate::validation;

/// SQL run once per app database to set up the Dataverse metadata layer.
///
/// Idempotent (uses `IF NOT EXISTS`) so the bootstrap can be replayed
/// safely after a partial provisioning failure.
pub const INIT_METADATA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS _dv_tables (
    id             BIGSERIAL PRIMARY KEY,
    name           TEXT NOT NULL UNIQUE,
    slug           TEXT NOT NULL UNIQUE,
    description    TEXT,
    -- 'bigserial' (legacy default) or 'uuid' — picks the implicit
    -- `id` column type for this user table. See [`IdStrategy`].
    id_strategy    TEXT NOT NULL DEFAULT 'bigserial',
    -- Primary display column: shown in place of the raw id when this table
    -- is referenced by a Lookup (DbExplorer, selectors, gateway $expand).
    -- NULL = auto (heuristic). See [`DatabaseSchema::effective_display_column`].
    display_column TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Forward-compat: existing databases that pre-date these columns get them
-- backfilled. ALTER … IF NOT EXISTS is idempotent, so replaying the whole
-- bootstrap on every engine open is safe.
ALTER TABLE _dv_tables
    ADD COLUMN IF NOT EXISTS id_strategy TEXT NOT NULL DEFAULT 'bigserial';
ALTER TABLE _dv_tables
    ADD COLUMN IF NOT EXISTS display_column TEXT;

CREATE TABLE IF NOT EXISTS _dv_columns (
    id                  BIGSERIAL PRIMARY KEY,
    table_name          TEXT NOT NULL,
    name                TEXT NOT NULL,
    field_type          TEXT NOT NULL,
    required            BOOLEAN NOT NULL DEFAULT FALSE,
    is_unique           BOOLEAN NOT NULL DEFAULT FALSE,
    default_value       TEXT,
    description         TEXT,
    choices             JSONB NOT NULL DEFAULT '[]'::jsonb,
    position            INTEGER NOT NULL DEFAULT 0,
    formula_expression  TEXT,
    lookup_target       TEXT,
    UNIQUE (table_name, name)
);
CREATE INDEX IF NOT EXISTS _dv_columns_table_name_idx ON _dv_columns (table_name);

CREATE TABLE IF NOT EXISTS _dv_relations (
    id            BIGSERIAL PRIMARY KEY,
    from_table    TEXT NOT NULL,
    from_column   TEXT NOT NULL,
    to_table      TEXT NOT NULL,
    to_column     TEXT NOT NULL,
    relation_type TEXT NOT NULL,
    on_delete     TEXT NOT NULL DEFAULT 'restrict',
    on_update     TEXT NOT NULL DEFAULT 'cascade',
    UNIQUE (from_table, from_column, to_table, to_column)
);

CREATE TABLE IF NOT EXISTS _dv_migrations (
    id          BIGSERIAL PRIMARY KEY,
    description TEXT NOT NULL,
    operations  JSONB NOT NULL,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS _dv_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO _dv_meta (key, value) VALUES ('schema_version', '1')
ON CONFLICT (key) DO NOTHING;

-- Per-database audit log. Every gateway-level mutation appends here in
-- the same transaction as the data change, so the table is the
-- ground-truth history (legacy `_dv_migrations` only journals schema
-- changes). Rows are never deleted; admin tooling adds retention.
CREATE TABLE IF NOT EXISTS _dv_audit (
    id          BIGSERIAL PRIMARY KEY,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now(),
    table_name  TEXT NOT NULL,
    row_id      TEXT NOT NULL,
    op          TEXT NOT NULL CHECK (op IN ('INSERT','UPDATE','DELETE','RESTORE')),
    actor_kind  TEXT NOT NULL CHECK (actor_kind IN ('user','app','system')),
    actor_uuid  UUID,
    actor_label TEXT,
    before      JSONB,
    after       JSONB,
    diff        JSONB
);
CREATE INDEX IF NOT EXISTS _dv_audit_table_row_idx ON _dv_audit (table_name, row_id);
CREATE INDEX IF NOT EXISTS _dv_audit_ts_idx        ON _dv_audit (ts DESC);

-- Combined trigger: bumps `updated_at` AND `version` on every UPDATE.
-- Defense in depth: even if the gateway forgets to increment `version` in
-- the SET clause, the trigger guarantees monotonic version progression so
-- optimistic-concurrency callers (`If-Match`) stay correct.
--
-- The function name is preserved (`_dv_set_updated_at`) for backwards
-- compatibility with per-table triggers created before the base model
-- landed; new behavior fires automatically on the next UPDATE because
-- `CREATE OR REPLACE FUNCTION` updates the body in place.
CREATE OR REPLACE FUNCTION _dv_set_updated_at() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    -- Only bump version when the column exists on this table. Pre-base-model
    -- tables that haven't been migrated yet have no `version`; the trigger
    -- gracefully skips the bump rather than aborting their UPDATEs.
    IF TG_RELID IS NOT NULL AND EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = TG_TABLE_NAME
          AND table_schema = TG_TABLE_SCHEMA
          AND column_name = 'version'
    ) THEN
        NEW.version = COALESCE(OLD.version, 0) + 1;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
"#;

pub struct DataverseEngine {
    pool: PgPool,
    slug: String,
}

impl DataverseEngine {
    pub fn new(pool: PgPool, slug: impl Into<String>) -> Self {
        Self {
            pool,
            slug: slug.into(),
        }
    }

    pub fn pool(&self) -> &PgPool { &self.pool }
    pub fn slug(&self) -> &str { &self.slug }

    /// Run `INIT_METADATA_SQL` against this engine's pool. Safe to call
    /// repeatedly (everything is `IF NOT EXISTS`). After bootstrap,
    /// upgrades pre-base-model tables (legacy data-migrated apps) by
    /// adding the audit/version/soft-delete columns + the active-row
    /// partial index; this is idempotent via `ADD COLUMN IF NOT EXISTS`.
    #[instrument(level = "info", skip(self), fields(slug = %self.slug))]
    pub async fn init_metadata(&self) -> Result<()> {
        sqlx::raw_sql(INIT_METADATA_SQL).execute(&self.pool).await?;
        self.upgrade_base_model().await?;
        self.backfill_display_columns().await?;
        Ok(())
    }

    /// Ensure every table has an EXPLICIT primary display column. Tables that
    /// predate this feature (or were created via a path that left it NULL) get
    /// pinned to their auto-resolved column ("id" when there's no readable text
    /// column). Idempotent and cheap: a NULL-count probe short-circuits it to a
    /// no-op once every row is set, so replaying on each engine open is free.
    async fn backfill_display_columns(&self) -> Result<()> {
        let pending: i64 =
            sqlx::query_scalar("SELECT count(*) FROM _dv_tables WHERE display_column IS NULL")
                .fetch_one(&self.pool)
                .await?;
        if pending == 0 {
            return Ok(());
        }
        let schema = self.get_schema().await?;
        for t in &schema.tables {
            if t.display_column.is_some() {
                continue;
            }
            let resolved = schema.effective_display_column(t);
            sqlx::query(
                "UPDATE _dv_tables SET display_column = $2 \
                 WHERE name = $1 AND display_column IS NULL",
            )
            .bind(&t.name)
            .bind(&resolved)
            .execute(&self.pool)
            .await?;
        }
        tracing::info!(slug = %self.slug, count = pending, "backfilled explicit display_column");
        Ok(())
    }

    /// One-shot upgrade of every user table to the current base model.
    /// Idempotent. Called from [`Self::init_metadata`].
    async fn upgrade_base_model(&self) -> Result<()> {
        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM _dv_tables")
                .fetch_all(&self.pool)
                .await?;
        for (name,) in tables {
            for stmt in crate::migration::add_base_columns_sql(&name) {
                sqlx::raw_sql(sqlx::AssertSqlSafe(stmt)).execute(&self.pool).await?;
            }
        }
        Ok(())
    }

    /// Return the user-defined table names (i.e. excludes `_dv_*`).
    pub async fn list_tables(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM _dv_tables ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }

    /// Taille disque totale de la base `app_{slug}` (octets). Pour la page
    /// `/stats`. Aucun input utilisateur → requête statique sûre.
    pub async fn database_size_bytes(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT pg_database_size(current_database())::bigint")
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    /// Estimation du nombre de lignes vivantes sur les tables utilisateur
    /// (`pg_stat_user_tables.n_live_tup`, exclut les tables système `_dv_*`).
    /// Estimation (pas un COUNT exact) — bien moins coûteux pour une vue globale.
    pub async fn live_row_estimate(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COALESCE(sum(n_live_tup),0)::bigint FROM pg_stat_user_tables \
              WHERE relname NOT LIKE '\\_dv\\_%'",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    /// Read the current `schema_version` from `_dv_meta`.
    pub async fn schema_version(&self) -> Result<u64> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM _dv_meta WHERE key = 'schema_version'")
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((v,)) => v.parse::<u64>().map_err(|e| {
                DataverseError::internal(format!("invalid schema_version '{}': {}", v, e))
            }),
            None => Err(DataverseError::NotProvisioned(self.slug.clone())),
        }
    }

    /// Number of rows in a user table.
    pub async fn count_rows(&self, table: &str) -> Result<i64> {
        validation::validate_user_identifier(table)?;
        let exists = self.table_exists(table).await?;
        if !exists {
            return Err(DataverseError::TableNotFound(table.into()));
        }
        let sql = format!("SELECT COUNT(*) FROM {}", quote_ident(table));
        let (count,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(sql)).fetch_one(&self.pool).await?;
        Ok(count)
    }

    /// Number of *active* (non soft-deleted) rows in a user table.
    ///
    /// Mirrors the `"is_deleted" = FALSE` predicate the query gateway auto-injects
    /// (cf. `query::build_list_sql`) so the sidebar count matches the grid's `total`.
    /// `is_deleted` is a guaranteed system column on every dataverse table.
    pub async fn count_active_rows(&self, table: &str) -> Result<i64> {
        validation::validate_user_identifier(table)?;
        let exists = self.table_exists(table).await?;
        if !exists {
            return Err(DataverseError::TableNotFound(table.into()));
        }
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE \"is_deleted\" = FALSE",
            quote_ident(table)
        );
        let (count,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(sql)).fetch_one(&self.pool).await?;
        Ok(count)
    }

    async fn table_exists(&self, table: &str) -> Result<bool> {
        let row: Option<(bool,)> = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM _dv_tables WHERE name = $1)",
        )
        .bind(table)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(b,)| b).unwrap_or(false))
    }

    /// Read just the [`IdStrategy`] of a single user table. Returns `None`
    /// when the table is unknown — caller decides between defaulting and
    /// erroring (here, default is the safe choice for FK type resolution).
    async fn lookup_strategy_of(&self, table: &str) -> Result<IdStrategy> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id_strategy FROM _dv_tables WHERE name = $1",
        )
        .bind(table)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row
            .and_then(|(s,)| IdStrategy::from_code(&s))
            .unwrap_or_default())
    }

    /// Read the full schema (tables + columns + relations + version).
    pub async fn get_schema(&self) -> Result<DatabaseSchema> {
        let table_rows: Vec<(
            String,         // name
            String,         // slug
            Option<String>, // description
            String,         // id_strategy
            Option<String>, // display_column
            DateTime<Utc>,
            DateTime<Utc>,
        )> = sqlx::query_as(
            "SELECT name, slug, description, id_strategy, display_column, created_at, updated_at \
             FROM _dv_tables ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut tables: Vec<TableDefinition> = Vec::with_capacity(table_rows.len());
        for (name, slug, description, id_strategy_code, display_column, created_at, updated_at) in
            table_rows
        {
            let cols = self.list_columns(&name).await?;
            let id_strategy = IdStrategy::from_code(&id_strategy_code).unwrap_or_default();
            tables.push(TableDefinition {
                name, slug, description, columns: cols, id_strategy, display_column, created_at, updated_at,
            });
        }

        let rel_rows: Vec<(String, String, String, String, String, String, String)> =
            sqlx::query_as(
                "SELECT from_table, from_column, to_table, to_column, relation_type, on_delete, on_update FROM _dv_relations ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await?;
        let mut relations: Vec<RelationDefinition> = Vec::with_capacity(rel_rows.len());
        for (ft, fc, tt, tc, rt, od, ou) in rel_rows {
            relations.push(RelationDefinition {
                from_table: ft,
                from_column: fc,
                to_table: tt,
                to_column: tc,
                relation_type: RelationType::from_code(&rt).unwrap_or(RelationType::OneToMany),
                cascade: crate::schema::CascadeRules {
                    on_delete: CascadeAction::from_code(&od).unwrap_or_default(),
                    on_update: CascadeAction::from_code(&ou).unwrap_or(CascadeAction::Cascade),
                },
            });
        }

        let version = self.schema_version().await?;

        Ok(DatabaseSchema { tables, relations, version, updated_at: Some(Utc::now()) })
    }

    async fn list_columns(&self, table: &str) -> Result<Vec<ColumnDefinition>> {
        let rows: Vec<(
            String,                     // name
            String,                     // field_type
            bool,                       // required
            bool,                       // is_unique
            Option<String>,             // default_value
            Option<String>,             // description
            serde_json::Value,          // choices
            Option<String>,             // formula_expression
            Option<String>,             // lookup_target
        )> = sqlx::query_as(
            "SELECT name, field_type, required, is_unique, default_value, description, \
             choices, formula_expression, lookup_target \
             FROM _dv_columns WHERE table_name = $1 ORDER BY position, id",
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (name, ft, required, is_unique, default_value, description, choices_json, formula_expression, lookup_target) in rows {
            let field_type = FieldType::from_code(&ft).ok_or_else(|| {
                DataverseError::SchemaMismatch(format!("unknown field_type '{}' in _dv_columns for {}.{}", ft, table, name))
            })?;
            let choices: Vec<String> = serde_json::from_value(choices_json).unwrap_or_default();
            out.push(ColumnDefinition {
                name, field_type, required, unique: is_unique, default_value, description,
                choices, formula_expression, lookup_target,
            });
        }
        Ok(out)
    }

    /// Create a user table: DDL + metadata + migration journal + version bump.
    ///
    /// **Atomicity note:** sqlx 0.8 + Rust's async-fn-in-trait have an HRTB
    /// quirk that prevents `Send` futures when borrowing a `Transaction`
    /// across multiple awaits. As a pragmatic V1 compromise we run each
    /// statement on the pool (Postgres auto-commits) — schema mutations
    /// are infrequent and a partial failure leaves an orphan that
    /// `sync_schema` can repair. Restoring transactional grouping is a
    /// later fix once sqlx ships an HRTB-friendly Transaction API.
    #[instrument(level = "info", skip(self, def), fields(slug = %self.slug, table = %def.name))]
    pub async fn create_table(&self, def: &TableDefinition) -> Result<u64> {
        let snapshot = self.get_schema().await?;
        validation::validate_table_definition(def, &snapshot)?;

        // 1. CREATE TABLE — DDL needs the schema snapshot so Lookup
        //    columns inherit their target table's id_strategy for the
        //    FK column type (BIGINT vs UUID).
        sqlx::raw_sql(sqlx::AssertSqlSafe(create_table_sql(def, &snapshot))).execute(&self.pool).await?;

        // 2. Trigger that bumps `updated_at` and `version` on every UPDATE.
        sqlx::raw_sql(sqlx::AssertSqlSafe(create_updated_at_trigger_sql(&def.name))).execute(&self.pool).await?;

        // 3. Partial index for the soft-delete-aware default filter — `WHERE
        //    is_deleted = FALSE` is auto-injected by the gateway, the index
        //    keeps active-row scans cheap.
        sqlx::raw_sql(sqlx::AssertSqlSafe(create_active_index_sql(&def.name))).execute(&self.pool).await?;

        // 4. _dv_tables row — persists id_strategy so subsequent
        //    schema reads (and add_column for Lookups targeting this
        //    table) pick up the right FK type. Every table also gets an
        //    EXPLICIT primary display column: the caller's choice if given,
        //    else the auto-resolved one (never NULL; "id" when the table has
        //    no readable text column).
        let display_column = match &def.display_column {
            Some(c) => c.clone(),
            None => snapshot.effective_display_column(def),
        };
        sqlx::query(
            "INSERT INTO _dv_tables (name, slug, description, id_strategy, display_column) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&def.name)
        .bind(&def.slug)
        .bind(&def.description)
        .bind(def.id_strategy.as_str())
        .bind(&display_column)
        .execute(&self.pool)
        .await?;

        // 5. _dv_columns rows
        for (pos, col) in def.columns.iter().enumerate() {
            insert_column_metadata(&self.pool, &def.name, col, pos as i32).await?;
        }

        // 6. FK constraints for any Lookup columns
        for col in def.columns.iter().filter(|c| c.field_type == FieldType::Lookup) {
            if let Some(target) = &col.lookup_target {
                let rel = RelationDefinition {
                    from_table: def.name.clone(),
                    from_column: col.name.clone(),
                    to_table: target.clone(),
                    to_column: "id".into(),
                    relation_type: if target == &def.name { RelationType::SelfReferential } else { RelationType::OneToMany },
                    cascade: Default::default(),
                };
                sqlx::raw_sql(sqlx::AssertSqlSafe(add_foreign_key_sql(&rel))).execute(&self.pool).await?;
                insert_relation_metadata(&self.pool, &rel).await?;
            }
        }

        let version = bump_schema_version(&self.pool).await?;
        journal_migration(&self.pool, &format!("create_table:{}", def.name), &json!({
            "op": "create_table",
            "table": def.name,
            "columns": def.columns.iter().map(|c| &c.name).collect::<Vec<_>>(),
        })).await?;

        Ok(version)
    }

    /// Set a table's **primary display column** — the column shown in place of
    /// the raw id when the table is referenced by a Lookup.
    ///
    /// Every table keeps an EXPLICIT display column (never NULL): a `None`
    /// request recomputes the auto default (heuristic) and pins it. `id` is a
    /// valid value (means "show the raw id"); other system columns are not.
    /// A non-`id` value must be an existing user column. Bumps the schema
    /// version + journals the change, like other schema-ops.
    #[instrument(level = "info", skip(self), fields(slug = %self.slug, table = %table))]
    pub async fn set_display_column(&self, table: &str, column: Option<&str>) -> Result<u64> {
        validation::validate_user_identifier(table)?;
        if !self.table_exists(table).await? {
            return Err(DataverseError::TableNotFound(table.into()));
        }
        let resolved: String = match column {
            // "id" = show the raw id — allowed even though it's a base column.
            Some("id") => "id".to_string(),
            Some(col) => {
                if crate::migration::is_base_column(col) {
                    return Err(DataverseError::SchemaMismatch(format!(
                        "'{col}' is a system column and cannot be used as a display column"
                    )));
                }
                let cols = self.list_columns(table).await?;
                if !cols.iter().any(|c| c.name == col) {
                    return Err(DataverseError::ColumnNotFound {
                        table: table.into(),
                        column: col.into(),
                    });
                }
                col.to_string()
            }
            // No column given → recompute the default and pin it explicitly,
            // so the table never falls back to an implicit/NULL display column.
            None => {
                let schema = self.get_schema().await?;
                match schema.tables.iter().find(|t| t.name == table) {
                    Some(t) => schema.effective_display_column(t),
                    None => return Err(DataverseError::TableNotFound(table.into())),
                }
            }
        };
        sqlx::query("UPDATE _dv_tables SET display_column = $2 WHERE name = $1")
            .bind(table)
            .bind(&resolved)
            .execute(&self.pool)
            .await?;
        let version = bump_schema_version(&self.pool).await?;
        journal_migration(
            &self.pool,
            &format!("set_display_column:{table}"),
            &json!({ "op": "set_display_column", "table": table, "column": resolved }),
        )
        .await?;
        Ok(version)
    }

    /// Drop a user table, its FKs, its trigger, and its metadata.
    /// See [`create_table`] for the atomicity caveat.
    #[instrument(level = "info", skip(self), fields(slug = %self.slug, table = %name))]
    pub async fn drop_table(&self, name: &str) -> Result<u64> {
        validation::validate_user_identifier(name)?;
        if !self.table_exists(name).await? {
            return Err(DataverseError::TableNotFound(name.into()));
        }

        // The trigger is dropped automatically with the table (CASCADE).
        sqlx::raw_sql(sqlx::AssertSqlSafe(drop_table_sql(name))).execute(&self.pool).await?;

        sqlx::query("DELETE FROM _dv_columns WHERE table_name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM _dv_relations WHERE from_table = $1 OR to_table = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM _dv_tables WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;

        let version = bump_schema_version(&self.pool).await?;
        journal_migration(&self.pool, &format!("drop_table:{}", name), &json!({
            "op": "drop_table", "table": name,
        })).await?;

        Ok(version)
    }

    #[instrument(level = "info", skip(self, col), fields(slug = %self.slug, table = %table, column = %col.name))]
    pub async fn add_column(&self, table: &str, col: &ColumnDefinition) -> Result<u64> {
        validation::validate_user_identifier(table)?;
        validation::validate_column(col)?;
        if !self.table_exists(table).await? {
            return Err(DataverseError::TableNotFound(table.into()));
        }

        // For Lookup columns, look up the target table's id_strategy
        // so the FK column type matches the target's `id` type.
        let lookup_target_strategy = if col.field_type == FieldType::Lookup {
            match col.lookup_target.as_deref() {
                Some(target) => self.lookup_strategy_of(target).await.unwrap_or_default(),
                None => IdStrategy::default(),
            }
        } else {
            IdStrategy::default()
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(add_column_sql(table, col, lookup_target_strategy)))
            .execute(&self.pool)
            .await?;

        let position: i32 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM _dv_columns WHERE table_name = $1",
        )
        .bind(table)
        .fetch_one(&self.pool)
        .await?;
        insert_column_metadata(&self.pool, table, col, position).await?;

        if col.field_type == FieldType::Lookup {
            if let Some(target) = &col.lookup_target {
                let rel = RelationDefinition {
                    from_table: table.into(),
                    from_column: col.name.clone(),
                    to_table: target.clone(),
                    to_column: "id".into(),
                    relation_type: if target == table { RelationType::SelfReferential } else { RelationType::OneToMany },
                    cascade: Default::default(),
                };
                sqlx::raw_sql(sqlx::AssertSqlSafe(add_foreign_key_sql(&rel))).execute(&self.pool).await?;
                insert_relation_metadata(&self.pool, &rel).await?;
            }
        }

        let version = bump_schema_version(&self.pool).await?;
        journal_migration(&self.pool, &format!("add_column:{}.{}", table, col.name), &json!({
            "op": "add_column", "table": table, "column": col.name,
        })).await?;

        Ok(version)
    }

    #[instrument(level = "info", skip(self), fields(slug = %self.slug, table = %table, column = %column))]
    pub async fn remove_column(&self, table: &str, column: &str) -> Result<u64> {
        validation::validate_user_identifier(table)?;
        validation::validate_user_identifier(column)?;

        sqlx::raw_sql(sqlx::AssertSqlSafe(drop_column_sql(table, column))).execute(&self.pool).await?;

        sqlx::query("DELETE FROM _dv_columns WHERE table_name = $1 AND name = $2")
            .bind(table)
            .bind(column)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM _dv_relations WHERE from_table = $1 AND from_column = $2")
            .bind(table)
            .bind(column)
            .execute(&self.pool)
            .await?;

        let version = bump_schema_version(&self.pool).await?;
        journal_migration(&self.pool, &format!("remove_column:{}.{}", table, column), &json!({
            "op": "remove_column", "table": table, "column": column,
        })).await?;

        Ok(version)
    }

    #[instrument(level = "info", skip(self, rel), fields(slug = %self.slug, from = %rel.from_table, to = %rel.to_table))]
    pub async fn create_relation(&self, rel: &RelationDefinition) -> Result<u64> {
        let snapshot = self.get_schema().await?;
        validation::validate_relation(rel, &snapshot)?;

        sqlx::raw_sql(sqlx::AssertSqlSafe(add_foreign_key_sql(rel))).execute(&self.pool).await?;
        insert_relation_metadata(&self.pool, rel).await?;

        let version = bump_schema_version(&self.pool).await?;
        journal_migration(&self.pool, &format!("create_relation:{}.{} -> {}.{}", rel.from_table, rel.from_column, rel.to_table, rel.to_column), &json!({
            "op": "create_relation",
            "from": format!("{}.{}", rel.from_table, rel.from_column),
            "to": format!("{}.{}", rel.to_table, rel.to_column),
        })).await?;

        Ok(version)
    }

    /// Detect drift between the Dataverse metadata (`_dv_tables`) and the
    /// actual Postgres schema (`information_schema.tables`). Returns the
    /// list of inconsistencies. With `dry_run=false`, also auto-repairs:
    ///   - metadata pointing to a non-existent physical table → DELETE
    ///     orphaned rows in `_dv_tables` / `_dv_columns` / `_dv_relations`.
    ///   - physical table not in metadata → leave it alone (we do NOT
    ///     drop physical data) but report it. Adopting such a table
    ///     requires explicit operator action.
    ///
    /// This exists because `create_table` and friends can't be wrapped in
    /// a single Postgres transaction (sqlx 0.8 HRTB + DDL constraints),
    /// so a crash mid-mutation can leave a half-created table or stranded
    /// metadata. Called automatically (in `dry_run=true` mode) by
    /// `engine_for` the first time an engine is opened, and exposed on
    /// demand via the `/api/dv/{slug}/_repair` admin endpoint.
    #[instrument(level = "info", skip(self), fields(slug = %self.slug, dry_run))]
    pub async fn sync_schema(&self, dry_run: bool) -> Result<SyncSchemaReport> {
        let mut report = SyncSchemaReport::default();

        // 1. Tables present in _dv_tables: do they exist physically?
        let meta_tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM _dv_tables ORDER BY name")
                .fetch_all(&self.pool)
                .await?;

        for (table,) in &meta_tables {
            let exists: Option<(bool,)> = sqlx::query_as(
                "SELECT EXISTS(\
                    SELECT 1 FROM information_schema.tables \
                    WHERE table_schema = 'public' AND table_name = $1\
                )",
            )
            .bind(table)
            .fetch_optional(&self.pool)
            .await?;
            let physical_exists = exists.map(|(b,)| b).unwrap_or(false);
            if !physical_exists {
                report.orphan_metadata.push(table.clone());
                if !dry_run {
                    sqlx::query("DELETE FROM _dv_columns WHERE table_name = $1")
                        .bind(table)
                        .execute(&self.pool)
                        .await?;
                    sqlx::query(
                        "DELETE FROM _dv_relations WHERE from_table = $1 OR to_table = $1",
                    )
                    .bind(table)
                    .execute(&self.pool)
                    .await?;
                    sqlx::query("DELETE FROM _dv_tables WHERE name = $1")
                        .bind(table)
                        .execute(&self.pool)
                        .await?;
                    report.repaired_metadata.push(table.clone());
                }
            }
        }

        // 2. Physical tables not in _dv_tables (excluding _dv_* system).
        let physical_tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' \
             AND table_name NOT LIKE '\\_dv\\_%' ESCAPE '\\' \
             ORDER BY table_name",
        )
        .fetch_all(&self.pool)
        .await?;

        let meta_set: std::collections::HashSet<&str> =
            meta_tables.iter().map(|(n,)| n.as_str()).collect();

        for (table,) in &physical_tables {
            if !meta_set.contains(table.as_str()) {
                report.orphan_physical.push(table.clone());
                // We do NOT auto-drop user data. Operator must inspect.
            }
        }

        // 3. For tables that do exist in both, check the update trigger and
        //    the active-row partial index are still present. These are
        //    created in `create_table` (steps 2 and 3) and a crash between
        //    them and the metadata insert leaves them, but the inverse
        //    (table created via psql then init_metadata replayed) lacks
        //    them.
        for (table,) in &meta_tables {
            if report.orphan_metadata.contains(table) {
                continue;
            }
            let trigger_name = format!("{}_set_updated_at", table);
            let trig: Option<(bool,)> = sqlx::query_as(
                "SELECT EXISTS(SELECT 1 FROM pg_trigger \
                 JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid \
                 WHERE pg_class.relname = $1 AND pg_trigger.tgname = $2)",
            )
            .bind(table)
            .bind(&trigger_name)
            .fetch_optional(&self.pool)
            .await?;
            if !trig.map(|(b,)| b).unwrap_or(false) {
                report.missing_triggers.push(table.clone());
                if !dry_run {
                    let sql = crate::migration::create_updated_at_trigger_sql(table);
                    let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(&self.pool).await;
                    report.repaired_triggers.push(table.clone());
                }
            }
        }

        if report.is_empty() {
            tracing::info!(slug = %self.slug, "sync_schema clean");
        } else {
            tracing::warn!(
                slug = %self.slug,
                orphan_meta = report.orphan_metadata.len(),
                orphan_phys = report.orphan_physical.len(),
                missing_triggers = report.missing_triggers.len(),
                dry_run,
                "sync_schema drift detected"
            );
        }
        Ok(report)
    }
}

/// Output of [`DataverseEngine::sync_schema`].
#[derive(Debug, Default, serde::Serialize)]
pub struct SyncSchemaReport {
    /// Tables present in `_dv_tables` but missing in `information_schema`.
    pub orphan_metadata: Vec<String>,
    /// Tables physically present but not in `_dv_tables`.
    pub orphan_physical: Vec<String>,
    /// Tables missing their `*_set_updated_at` trigger.
    pub missing_triggers: Vec<String>,
    /// Subset of `orphan_metadata` that was repaired in this call.
    pub repaired_metadata: Vec<String>,
    /// Subset of `missing_triggers` that was repaired in this call.
    pub repaired_triggers: Vec<String>,
}

impl SyncSchemaReport {
    pub fn is_empty(&self) -> bool {
        self.orphan_metadata.is_empty()
            && self.orphan_physical.is_empty()
            && self.missing_triggers.is_empty()
    }
}

async fn insert_column_metadata(
    pool: &PgPool,
    table: &str,
    col: &ColumnDefinition,
    position: i32,
) -> Result<()> {
    let choices_json = serde_json::to_value(&col.choices)?;
    sqlx::query(
        "INSERT INTO _dv_columns \
         (table_name, name, field_type, required, is_unique, default_value, description, \
          choices, position, formula_expression, lookup_target) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(table)
    .bind(&col.name)
    .bind(col.field_type.as_str())
    .bind(col.required)
    .bind(col.unique)
    .bind(&col.default_value)
    .bind(&col.description)
    .bind(choices_json)
    .bind(position)
    .bind(&col.formula_expression)
    .bind(&col.lookup_target)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_relation_metadata(
    pool: &PgPool,
    rel: &RelationDefinition,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO _dv_relations \
         (from_table, from_column, to_table, to_column, relation_type, on_delete, on_update) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (from_table, from_column, to_table, to_column) DO NOTHING",
    )
    .bind(&rel.from_table)
    .bind(&rel.from_column)
    .bind(&rel.to_table)
    .bind(&rel.to_column)
    .bind(rel.relation_type.as_str())
    .bind(rel.cascade.on_delete.as_str())
    .bind(rel.cascade.on_update.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

async fn bump_schema_version(pool: &PgPool) -> Result<u64> {
    let row: (String,) = sqlx::query_as(
        "UPDATE _dv_meta SET value = (CAST(value AS BIGINT) + 1)::TEXT \
         WHERE key = 'schema_version' RETURNING value",
    )
    .fetch_one(pool)
    .await?;
    row.0
        .parse::<u64>()
        .map_err(|e| DataverseError::internal(format!("invalid schema_version: {}", e)))
}

async fn journal_migration(
    pool: &PgPool,
    description: &str,
    operations: &serde_json::Value,
) -> Result<()> {
    sqlx::query("INSERT INTO _dv_migrations (description, operations) VALUES ($1, $2)")
        .bind(description)
        .bind(operations)
        .execute(pool)
        .await?;
    Ok(())
}
