//! Helpers shared by query and mutation resolvers:
//! - [`SqlBuilder`] for building parameterised SQL with positional `$N` binds
//! - [`BindValue`] enum that erases the static type of a value while still
//!   binding the right Postgres type at execution time
//! - [`row_to_json`] which turns a `PgRow` into a `serde_json::Map` whose
//!   field types match the GraphQL scalar mapping
//!
//! V1 supports the column types the [`crate::schema::FieldType`] enum
//! defines, with the exception of `Lookup` expansion (the FK is returned
//! as an integer; expansion will arrive with the `DataLoader` in a future
//! milestone).

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde_json::{Map, Number, Value};
use sqlx_core::arguments::Arguments;
use sqlx_core::row::Row;
use sqlx_postgres::{PgArguments, PgRow};
use uuid::Uuid;

use crate::error::{DataverseError, Result};
use crate::schema::{ColumnDefinition, FieldType, IdStrategy};

/// Type-erased value of an implicit `id` (or any FK) — picks the right
/// Postgres bind based on the target table's [`IdStrategy`].
#[derive(Debug, Clone)]
pub enum IdValue {
    Int(i64),
    Uuid(Uuid),
}

impl IdValue {
    /// Convert to a [`BindValue`] for sqlx parameter binding.
    pub fn into_bind(self) -> BindValue {
        match self {
            IdValue::Int(i) => BindValue::Int(i),
            IdValue::Uuid(u) => BindValue::Uuid(u),
        }
    }

    /// Parse an `id` argument from a GraphQL Value, picking the variant
    /// from the target table's strategy.
    pub fn from_gql(v: &async_graphql::Value, strategy: IdStrategy) -> Result<Self> {
        use async_graphql::Value as V;
        match (strategy, v) {
            (IdStrategy::Bigserial, V::Number(n)) => {
                let i = n.as_i64().ok_or_else(|| {
                    DataverseError::internal(format!("expected Int id, got {:?}", n))
                })?;
                Ok(IdValue::Int(i))
            }
            (IdStrategy::Bigserial, V::String(s)) => {
                // Tolerant: integer-shaped strings are accepted (the
                // SPA may stringify large ids before sending).
                let i = s.parse::<i64>().map_err(|e| {
                    DataverseError::internal(format!("invalid Int id '{}': {}", s, e))
                })?;
                Ok(IdValue::Int(i))
            }
            (IdStrategy::Uuid, V::String(s)) => {
                let u = Uuid::parse_str(s).map_err(|e| {
                    DataverseError::internal(format!("invalid UUID id '{}': {}", s, e))
                })?;
                Ok(IdValue::Uuid(u))
            }
            (strategy, other) => Err(DataverseError::internal(format!(
                "id value {:?} doesn't match table id_strategy {:?}",
                other, strategy
            ))),
        }
    }
}

/// A typed value that can be bound to a `$N` placeholder. We keep the
/// type information here because Postgres rejects untyped NULLs and we
/// want to bind the right encoder per scalar.
#[derive(Debug, Clone)]
pub enum BindValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Json(Value),
    DateTime(DateTime<Utc>),
    Uuid(Uuid),
    /// Array of strings — used for `MultiChoice` columns (`TEXT[]`).
    TextArray(Vec<String>),
    /// `NULL` typed as the column's pg type, e.g. `$1::TEXT`.
    /// The cast string is whatever you'd put after `::`.
    NullAs(&'static str),
}

impl BindValue {
    /// Apply this bind to a `PgArguments` accumulator.
    pub fn add_to(self, args: &mut PgArguments) -> Result<()> {
        let result = match self {
            BindValue::Bool(b) => args.add(b),
            BindValue::Int(i) => args.add(i),
            BindValue::Float(f) => args.add(f),
            BindValue::Text(s) => args.add(s),
            BindValue::Json(v) => args.add(sqlx_core::types::Json(v)),
            BindValue::DateTime(dt) => args.add(dt),
            BindValue::Uuid(u) => args.add(u),
            BindValue::TextArray(v) => args.add(v),
            BindValue::NullAs(_) => args.add(Option::<i64>::None),
        };
        result.map_err(|e| DataverseError::internal(format!("bind error: {}", e)))
    }

    /// When emitting the placeholder, NULL casts need an explicit type.
    pub fn placeholder(&self, n: usize) -> String {
        match self {
            BindValue::NullAs(ty) => format!("${}::{}", n, ty),
            _ => format!("${}", n),
        }
    }
}

/// Accumulates parameterised SQL fragments and their bound values.
#[derive(Debug, Default)]
pub struct SqlBuilder {
    pub binds: Vec<BindValue>,
}

impl SqlBuilder {
    pub fn new() -> Self { Self::default() }

    /// Push a value and return its placeholder string (e.g. `$3` or
    /// `$3::TEXT` for typed nulls).
    pub fn push(&mut self, v: BindValue) -> String {
        self.binds.push(v);
        let n = self.binds.len();
        self.binds[n - 1].placeholder(n)
    }

    /// Convert into [`PgArguments`] for `sqlx::query_with`.
    pub fn into_arguments(self) -> Result<PgArguments> {
        let mut args = PgArguments::default();
        for bind in self.binds {
            bind.add_to(&mut args)?;
        }
        Ok(args)
    }
}

/// Convert an `async_graphql::Value` (received as a query argument or
/// mutation input) into a [`BindValue`] of the column's expected type.
///
/// Returns `Ok(None)` if the value is `null` and the column is not a
/// nullable type (caller decides whether that's an error).
///
/// `lookup_target_strategy` matters only when `field_type` is
/// [`FieldType::Lookup`] — it picks between Int (target=Bigserial) and
/// Uuid (target=Uuid) binding. For non-Lookup columns the value is
/// ignored.
pub fn graphql_value_to_bind(
    val: &async_graphql::Value,
    field_type: FieldType,
    lookup_target_strategy: IdStrategy,
) -> Result<Option<BindValue>> {
    use async_graphql::Value as V;

    if matches!(val, V::Null) {
        return Ok(None);
    }

    let bind = match (field_type, val) {
        (FieldType::Boolean, V::Boolean(b)) => BindValue::Bool(*b),
        (FieldType::Number | FieldType::AutoIncrement, V::Number(n)) => {
            let i = n.as_i64().ok_or_else(|| {
                DataverseError::internal(format!("expected integer, got {:?}", n))
            })?;
            BindValue::Int(i)
        }
        (FieldType::Lookup, V::Number(n)) => {
            // Bigserial-target Lookup arrives as Int.
            let i = n.as_i64().ok_or_else(|| {
                DataverseError::internal(format!("expected integer FK, got {:?}", n))
            })?;
            BindValue::Int(i)
        }
        (FieldType::Lookup, V::String(s)) => {
            // UUID-target Lookup arrives as String. Accept bigserial-style
            // numeric strings too, defensively, when target is Bigserial
            // (some clients stringify large ints).
            match lookup_target_strategy {
                IdStrategy::Uuid => {
                    let u = Uuid::parse_str(s).map_err(|e| {
                        DataverseError::internal(format!("invalid UUID FK '{}': {}", s, e))
                    })?;
                    BindValue::Uuid(u)
                }
                IdStrategy::Bigserial => {
                    let i = s.parse::<i64>().map_err(|e| {
                        DataverseError::internal(format!("invalid Int FK '{}': {}", s, e))
                    })?;
                    BindValue::Int(i)
                }
            }
        }
        (FieldType::Decimal | FieldType::Currency | FieldType::Percent, V::Number(n)) => {
            // Numeric arrives as either an int or a float. We bind as text
            // to preserve precision; PG casts it via NUMERIC.
            BindValue::Text(n.to_string())
        }
        (FieldType::Decimal | FieldType::Currency | FieldType::Percent, V::String(s)) => {
            BindValue::Text(s.clone())
        }
        (FieldType::Text | FieldType::Email | FieldType::Url | FieldType::Phone
         | FieldType::Choice | FieldType::Time | FieldType::Duration | FieldType::Formula,
         V::String(s)) => BindValue::Text(s.clone()),
        (FieldType::DateTime, V::String(s)) => {
            let dt = DateTime::parse_from_rfc3339(s)
                .map_err(|e| DataverseError::internal(format!("invalid DateTime '{}': {}", s, e)))?
                .with_timezone(&Utc);
            BindValue::DateTime(dt)
        }
        (FieldType::Date, V::String(s)) => BindValue::Text(s.clone()),
        (FieldType::Uuid, V::String(s)) => {
            let u = Uuid::parse_str(s)
                .map_err(|e| DataverseError::internal(format!("invalid UUID '{}': {}", s, e)))?;
            BindValue::Uuid(u)
        }
        (FieldType::Json, v) => BindValue::Json(graphql_value_to_json(v)),
        (FieldType::MultiChoice, V::List(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                if let V::String(s) = it {
                    out.push(s.clone());
                } else {
                    return Err(DataverseError::internal(format!(
                        "MultiChoice expects strings, got {:?}", it
                    )));
                }
            }
            BindValue::TextArray(out)
        }
        (ft, v) => {
            return Err(DataverseError::internal(format!(
                "type mismatch: column type {:?} cannot accept value {:?}", ft, v
            )));
        }
    };
    Ok(Some(bind))
}

/// Convert any `async_graphql::Value` into a `serde_json::Value` (for
/// JSON columns, where we don't care about the schema).
pub fn graphql_value_to_json(val: &async_graphql::Value) -> Value {
    use async_graphql::Value as V;
    match val {
        V::Null => Value::Null,
        V::Number(n) => Value::Number(n.clone()),
        V::String(s) => Value::String(s.clone()),
        V::Boolean(b) => Value::Bool(*b),
        V::Binary(b) => Value::String(format!("0x{}", hex_encode(b))),
        V::Enum(name) => Value::String(name.to_string()),
        V::List(items) => Value::Array(items.iter().map(graphql_value_to_json).collect()),
        V::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map.iter() {
                out.insert(k.to_string(), graphql_value_to_json(v));
            }
            Value::Object(out)
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

/// Read context for [`row_to_json`] / [`read_column_value`]: holds the
/// table's id_strategy (governs the `id` column read) and a per-column
/// lookup-target strategy lookup (governs each Lookup FK read).
pub struct RowReadCtx<'a> {
    pub id_strategy: IdStrategy,
    /// Strategy of each Lookup column's target table, indexed by column
    /// name. Non-Lookup columns may be absent.
    pub lookup_targets: std::collections::HashMap<&'a str, IdStrategy>,
}

impl<'a> RowReadCtx<'a> {
    pub fn from_table_and_schema(
        table: &'a crate::schema::TableDefinition,
        dataverse: &'a crate::schema::DatabaseSchema,
    ) -> Self {
        let mut lookup_targets = std::collections::HashMap::new();
        for col in &table.columns {
            if col.field_type == FieldType::Lookup {
                if let Some(target) = col.lookup_target.as_deref() {
                    let s = dataverse
                        .tables
                        .iter()
                        .find(|t| t.name == target)
                        .map(|t| t.id_strategy)
                        .unwrap_or_default();
                    lookup_targets.insert(col.name.as_str(), s);
                }
            }
        }
        Self {
            id_strategy: table.id_strategy,
            lookup_targets,
        }
    }
}

/// Decode a `PgRow` into a `serde_json::Map` whose values respect the
/// GraphQL scalar mapping (cf. [`FieldType::graphql_type_name`]).
///
/// Implicit columns (`id`, `created_at`, `updated_at`) are always read.
/// User columns are read by `column_def` order; the column must exist in
/// the row by name (driven by the SELECT list the resolver issues).
pub fn row_to_json(
    row: &PgRow,
    columns: &[ColumnDefinition],
    ctx: &RowReadCtx<'_>,
) -> Result<Map<String, Value>> {
    let mut out = Map::new();

    // id — type depends on the table's id_strategy.
    match ctx.id_strategy {
        IdStrategy::Bigserial => {
            let id: i64 = row.try_get("id").map_err(map_err)?;
            out.insert("id".into(), Value::Number(Number::from(id)));
        }
        IdStrategy::Uuid => {
            let id: Uuid = row.try_get("id").map_err(map_err)?;
            out.insert("id".into(), Value::String(id.to_string()));
        }
    }

    // created_at / updated_at
    let created: DateTime<Utc> = row.try_get("created_at").map_err(map_err)?;
    out.insert("created_at".into(), Value::String(created.to_rfc3339()));
    let updated: DateTime<Utc> = row.try_get("updated_at").map_err(map_err)?;
    out.insert("updated_at".into(), Value::String(updated.to_rfc3339()));

    for col in columns {
        let key = col.name.as_str();
        let lookup_target = ctx.lookup_targets.get(key).copied().unwrap_or_default();
        let v = read_column_value(row, key, col.field_type, lookup_target)?;
        out.insert(key.to_string(), v);
    }

    Ok(out)
}

fn read_column_value(
    row: &PgRow,
    name: &str,
    ft: FieldType,
    lookup_target_strategy: IdStrategy,
) -> Result<Value> {
    Ok(match ft {
        FieldType::Boolean => match row.try_get::<Option<bool>, _>(name).map_err(map_err)? {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        },
        FieldType::Number | FieldType::AutoIncrement => {
            match row.try_get::<Option<i64>, _>(name).map_err(map_err)? {
                Some(i) => Value::Number(Number::from(i)),
                None => Value::Null,
            }
        }
        FieldType::Lookup => match lookup_target_strategy {
            IdStrategy::Bigserial => {
                match row.try_get::<Option<i64>, _>(name).map_err(map_err)? {
                    Some(i) => Value::Number(Number::from(i)),
                    None => Value::Null,
                }
            }
            IdStrategy::Uuid => {
                match row.try_get::<Option<Uuid>, _>(name).map_err(map_err)? {
                    Some(u) => Value::String(u.to_string()),
                    None => Value::Null,
                }
            }
        }
        FieldType::Decimal | FieldType::Currency | FieldType::Percent => {
            // Cast NUMERIC to TEXT in the SELECT list to preserve precision
            // server-side. The resolver issues `<col>::TEXT AS "<col>"`.
            match row.try_get::<Option<String>, _>(name).map_err(map_err)? {
                Some(s) => Value::String(s),
                None => Value::Null,
            }
        }
        FieldType::Text | FieldType::Email | FieldType::Url | FieldType::Phone
        | FieldType::Choice | FieldType::Formula => {
            match row.try_get::<Option<String>, _>(name).map_err(map_err)? {
                Some(s) => Value::String(s),
                None => Value::Null,
            }
        }
        FieldType::Date => {
            // PG `DATE` decodes to `chrono::NaiveDate` natively. We
            // serialise as `YYYY-MM-DD` for JSON.
            match row.try_get::<Option<NaiveDate>, _>(name).map_err(map_err)? {
                Some(d) => Value::String(d.format("%Y-%m-%d").to_string()),
                None => Value::Null,
            }
        }
        FieldType::Time => {
            match row.try_get::<Option<NaiveTime>, _>(name).map_err(map_err)? {
                Some(t) => Value::String(t.format("%H:%M:%S%.f").to_string()),
                None => Value::Null,
            }
        }
        FieldType::Duration => {
            // `INTERVAL` decoding is messy in sqlx; fallback to text
            // representation when present.
            match row.try_get::<Option<sqlx_postgres::types::PgInterval>, _>(name).map_err(map_err)? {
                Some(i) => Value::String(format!("{}mo {}d {}us", i.months, i.days, i.microseconds)),
                None => Value::Null,
            }
        }
        FieldType::DateTime => {
            match row.try_get::<Option<DateTime<Utc>>, _>(name).map_err(map_err)? {
                Some(dt) => Value::String(dt.to_rfc3339()),
                None => Value::Null,
            }
        }
        FieldType::Uuid => {
            match row.try_get::<Option<Uuid>, _>(name).map_err(map_err)? {
                Some(u) => Value::String(u.to_string()),
                None => Value::Null,
            }
        }
        FieldType::Json => {
            match row.try_get::<Option<sqlx_core::types::Json<Value>>, _>(name).map_err(map_err)? {
                Some(v) => v.0,
                None => Value::Null,
            }
        }
        FieldType::MultiChoice => {
            match row.try_get::<Option<Vec<String>>, _>(name).map_err(map_err)? {
                Some(arr) => Value::Array(arr.into_iter().map(Value::String).collect()),
                None => Value::Null,
            }
        }
    })
}

fn map_err(e: sqlx_core::Error) -> DataverseError {
    DataverseError::Sqlx(e)
}

/// A `SELECT` column-list fragment for a given column. Returns the
/// fragment to splice into the SELECT clause; for NUMERIC columns this
/// adds a `::TEXT` cast so the value is preserved as a string.
pub fn select_fragment_for(col: &ColumnDefinition) -> String {
    use crate::migration::quote_ident;
    match col.field_type {
        FieldType::Decimal | FieldType::Currency | FieldType::Percent => {
            format!("{}::TEXT AS {}", quote_ident(&col.name), quote_ident(&col.name))
        }
        _ => quote_ident(&col.name),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_basics() {
        let mut b = SqlBuilder::new();
        let p1 = b.push(BindValue::Int(42));
        let p2 = b.push(BindValue::Text("x".into()));
        assert_eq!(p1, "$1");
        assert_eq!(p2, "$2");
    }

    #[test]
    fn null_placeholder_carries_cast() {
        let mut b = SqlBuilder::new();
        let p = b.push(BindValue::NullAs("TEXT"));
        assert_eq!(p, "$1::TEXT");
    }

    #[test]
    fn graphql_to_bind_int() {
        use async_graphql::Value;
        let v = Value::Number(serde_json::Number::from(42));
        let b = graphql_value_to_bind(&v, FieldType::Number, IdStrategy::default()).unwrap();
        match b.unwrap() {
            BindValue::Int(i) => assert_eq!(i, 42),
            _ => panic!("expected Int"),
        }
    }

    #[test]
    fn graphql_to_bind_string() {
        use async_graphql::Value;
        let v = Value::String("hi".into());
        let b = graphql_value_to_bind(&v, FieldType::Text, IdStrategy::default()).unwrap();
        match b.unwrap() {
            BindValue::Text(s) => assert_eq!(s, "hi"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn null_returns_none() {
        use async_graphql::Value;
        let b = graphql_value_to_bind(&Value::Null, FieldType::Text, IdStrategy::default()).unwrap();
        assert!(b.is_none());
    }

    #[test]
    fn graphql_to_bind_lookup_uuid_target() {
        use async_graphql::Value;
        let v = Value::String("550e8400-e29b-41d4-a716-446655440000".into());
        let b = graphql_value_to_bind(&v, FieldType::Lookup, IdStrategy::Uuid).unwrap();
        match b.unwrap() {
            BindValue::Uuid(_) => {}
            other => panic!("expected Uuid bind, got {:?}", other),
        }
    }

    #[test]
    fn graphql_to_bind_lookup_bigserial_target() {
        use async_graphql::Value;
        let v = Value::Number(serde_json::Number::from(7));
        let b = graphql_value_to_bind(&v, FieldType::Lookup, IdStrategy::Bigserial).unwrap();
        match b.unwrap() {
            BindValue::Int(i) => assert_eq!(i, 7),
            _ => panic!("expected Int"),
        }
    }

    #[test]
    fn id_value_uuid_round_trip() {
        use async_graphql::Value;
        let s = "550e8400-e29b-41d4-a716-446655440000";
        let v = Value::String(s.into());
        let id = IdValue::from_gql(&v, IdStrategy::Uuid).unwrap();
        match id {
            IdValue::Uuid(u) => assert_eq!(u.to_string(), s),
            _ => panic!("expected Uuid id"),
        }
    }

    #[test]
    fn id_value_int_round_trip() {
        use async_graphql::Value;
        let v = Value::Number(serde_json::Number::from(42));
        let id = IdValue::from_gql(&v, IdStrategy::Bigserial).unwrap();
        match id {
            IdValue::Int(i) => assert_eq!(i, 42),
            _ => panic!("expected Int id"),
        }
    }
}
