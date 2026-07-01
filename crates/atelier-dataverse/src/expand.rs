//! Lookup display resolution shared by the admin DbExplorer path
//! (`apps_db::query_rows`) and the runtime gateway (`dv::list_rows`).
//!
//! Both callers decode their rows with [`crate::dv_io::decode_row`], which
//! keeps only columns declared in the schema — a SQL `JOIN` alias would be
//! dropped. So enrichment happens here as a *post-processing* pass: for each
//! requested lookup column we resolve its target's primary display column
//! (see [`DatabaseSchema::effective_display_column`]), fetch `id -> display`
//! in one indexed round-trip, and re-inject `{from_column}_display` next to
//! the untouched raw id. The raw id is never modified, so existing app
//! deserializers (which ignore unknown `_display` keys) don't break.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx_core::arguments::Arguments as _;
use sqlx_core::row::Row as _;
use sqlx_core::sql_str::AssertSqlSafe;
use sqlx_postgres::{PgArguments, PgPool};
use uuid::Uuid;

use crate::error::{DataverseError, Result};
use crate::migration::quote_ident;
use crate::schema::{DatabaseSchema, FieldType, IdStrategy, TableDefinition};

/// Postgres type family of a relation's target key column (`to_column`),
/// used to bind the `= ANY($1)` list and decode the join key back.
#[derive(Clone, Copy)]
enum KeyKind {
    Int,
    Uuid,
    Text,
}

/// Resolve the key kind of `to_column` on `target`. Lookups usually reference
/// the implicit `id` (whose type follows `id_strategy`), but a relation can
/// point at a natural key (e.g. a text `type_id`), so branch on the actual
/// column type. Returns `None` for shapes we can't key on (e.g. multi-choice).
fn key_kind_for(target: &TableDefinition, to_column: &str) -> Option<KeyKind> {
    if to_column == "id" {
        return Some(match target.id_strategy {
            IdStrategy::Bigserial => KeyKind::Int,
            IdStrategy::Uuid => KeyKind::Uuid,
        });
    }
    let col = target.columns.iter().find(|c| c.name == to_column)?;
    Some(match col.field_type {
        FieldType::Text | FieldType::Email | FieldType::Url | FieldType::Phone
        | FieldType::Choice => KeyKind::Text,
        FieldType::Number | FieldType::AutoIncrement | FieldType::Lookup => KeyKind::Int,
        FieldType::Uuid => KeyKind::Uuid,
        _ => return None,
    })
}

/// Enrich `rows` in place with `{from_column}_display` for every lookup column
/// of `table` listed in `expand`.
///
/// No-op when `expand` or `rows` is empty, when a requested column isn't a
/// declared relation, or when the target has no readable display column (the
/// resolver returns `"id"` → left as the raw id). Soft-deleted target rows are
/// excluded, so a reference to a deleted row keeps its raw id.
pub async fn expand_lookup_displays(
    pool: &PgPool,
    schema: &DatabaseSchema,
    table: &TableDefinition,
    rows: &mut [Value],
    expand: &[String],
) -> Result<()> {
    if expand.is_empty() || rows.is_empty() {
        return Ok(());
    }

    for rel in schema
        .relations
        .iter()
        .filter(|r| r.from_table == table.name && expand.iter().any(|e| e == &r.from_column))
    {
        let Some(target) = schema.tables.iter().find(|t| t.name == rel.to_table) else {
            continue;
        };
        let disp = schema.effective_display_column(target);
        if disp == "id" {
            continue; // nothing readable to show — leave the raw id
        }
        let Some(kind) = key_kind_for(target, &rel.to_column) else {
            continue; // can't key on this target column (e.g. multi-choice)
        };

        // Distinct non-null lookup keys present on this page.
        let keys: HashSet<String> = rows
            .iter()
            .filter_map(|row| row.get(&rel.from_column).and_then(id_key))
            .collect();
        if keys.is_empty() {
            continue;
        }

        // Join on the relation's target key column (usually `id`, but may be a
        // natural key like a text `type_id`), fetching its primary display col.
        let sql = format!(
            "SELECT {key} AS k, {disp} AS display FROM {tbl} \
             WHERE {key} = ANY($1) AND \"is_deleted\" = FALSE",
            key = quote_ident(&rel.to_column),
            disp = quote_ident(&disp),
            tbl = quote_ident(&rel.to_table),
        );
        let display_map = fetch_display_map(pool, &sql, &keys, kind).await?;
        if display_map.is_empty() {
            continue;
        }

        let display_key = format!("{}_display", rel.from_column);
        for row in rows.iter_mut() {
            let Some(obj) = row.as_object_mut() else { continue };
            let Some(key) = obj.get(&rel.from_column).and_then(id_key) else { continue };
            if let Some(disp_val) = display_map.get(&key) {
                obj.insert(display_key.clone(), Value::String(disp_val.clone()));
            }
        }
    }

    Ok(())
}

/// Stable string key for a lookup id value. Bigserial ids decode to JSON
/// numbers, UUID ids to strings; both collapse to a comparable string. `null`
/// / other shapes yield `None` (nothing to resolve).
fn id_key(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// One `to_column = ANY($1)` round-trip against the target table, keyed by the
/// same string form as [`id_key`]. `display` is textual (the resolver only ever
/// returns text columns), read as optional — null displays are skipped.
async fn fetch_display_map(
    pool: &PgPool,
    sql: &str,
    keys: &HashSet<String>,
    kind: KeyKind,
) -> Result<HashMap<String, String>> {
    let mut args = PgArguments::default();
    match kind {
        KeyKind::Int => {
            let ids: Vec<i64> = keys.iter().filter_map(|k| k.parse::<i64>().ok()).collect();
            if ids.is_empty() {
                return Ok(HashMap::new());
            }
            args.add(ids)
                .map_err(|e| DataverseError::internal(format!("expand bind keys: {e}")))?;
        }
        KeyKind::Uuid => {
            let ids: Vec<Uuid> = keys.iter().filter_map(|k| Uuid::parse_str(k).ok()).collect();
            if ids.is_empty() {
                return Ok(HashMap::new());
            }
            args.add(ids)
                .map_err(|e| DataverseError::internal(format!("expand bind keys: {e}")))?;
        }
        KeyKind::Text => {
            let ids: Vec<String> = keys.iter().cloned().collect();
            args.add(ids)
                .map_err(|e| DataverseError::internal(format!("expand bind keys: {e}")))?;
        }
    }

    let fetched = sqlx_core::query::query_with(AssertSqlSafe(sql), args)
        .fetch_all(pool)
        .await
        .map_err(|e| DataverseError::internal(format!("expand fetch: {e}")))?;

    let mut map = HashMap::with_capacity(fetched.len());
    for r in &fetched {
        let key = match kind {
            KeyKind::Int => r.try_get::<i64, _>("k").map(|i| i.to_string()),
            KeyKind::Uuid => r.try_get::<Uuid, _>("k").map(|u| u.to_string()),
            KeyKind::Text => r.try_get::<String, _>("k"),
        }
        .map_err(|e| DataverseError::internal(format!("expand key decode: {e}")))?;
        if let Ok(Some(disp)) = r.try_get::<Option<String>, _>("display") {
            map.insert(key, disp);
        }
    }
    Ok(map)
}
