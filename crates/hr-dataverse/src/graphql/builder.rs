//! Build a per-app `async_graphql::dynamic::Schema` from the Dataverse
//! metadata. The output is cached by [`crate::graphql::cache::SchemaCache`]
//! and rebuilt whenever `_dv_meta.schema_version` bumps.
//!
//! Layout of the generated schema (per user table `T`):
//! - Object type `<T>` with implicit `id`, `createdAt`, `updatedAt` and
//!   one field per `_dv_columns` row
//! - Inputs `<T>Where`, `<T>OrderBy`, `<T>Insert`, `<T>Update`
//! - Query: `<t>(where, orderBy, limit, offset)`, `<t>ById(id)`, `<t>Count(where)`
//! - Mutation: `insert<T>(values)`, `update<T>(id, values)`, `delete<T>(id)`
//!
//! Lookup expansion is currently NOT generated — Lookup columns expose
//! the integer FK only. The DataLoader-backed object-link field will land
//! in a follow-up milestone.

use std::sync::Arc;

use async_graphql::dynamic::{
    Enum as DynEnum, Field, FieldFuture, FieldValue, InputObject, InputValue, Object, ResolverContext,
    Scalar, Schema, SchemaError, TypeRef,
};
use async_graphql::Value as GqlValue;
use serde_json::Value as JsonValue;

use crate::engine::DataverseEngine;
use crate::error::{DataverseError, Result};
use crate::graphql::filters::where_fragment;
use crate::graphql::naming;
use crate::graphql::sql::{
    BindValue, IdValue, RowReadCtx, SqlBuilder, graphql_value_to_bind, row_to_json,
    select_fragment_for,
};
use crate::migration::quote_ident;
use crate::schema::{ColumnDefinition, DatabaseSchema, FieldType, IdStrategy, TableDefinition};

/// Types of the scalar filter inputs (one set, shared across tables).
const INT_FILTER: &str = "IntFilter";
const STRING_FILTER: &str = "StringFilter";
const BOOL_FILTER: &str = "BooleanFilter";

/// Names of the scalars we register on top of GraphQL built-ins.
const DATETIME_SCALAR: &str = "DateTime";
const JSON_SCALAR: &str = "JSON";
const UUID_SCALAR: &str = "UUID";

/// Translate a column's [`FieldType`] into the GraphQL output type ref.
///
/// `lookup_target_strategy` is consulted only for [`FieldType::Lookup`]:
/// the FK column surfaces as `Int` when the target table uses Bigserial,
/// or `String` when the target uses UUID PKs.
fn output_type_ref(ft: FieldType, lookup_target_strategy: IdStrategy) -> TypeRef {
    match ft {
        FieldType::Boolean => TypeRef::named(TypeRef::BOOLEAN),
        FieldType::Number | FieldType::AutoIncrement => TypeRef::named(TypeRef::INT),
        FieldType::Lookup => match lookup_target_strategy {
            IdStrategy::Bigserial => TypeRef::named(TypeRef::INT),
            IdStrategy::Uuid => TypeRef::named(TypeRef::STRING),
        },
        FieldType::DateTime => TypeRef::named(DATETIME_SCALAR),
        FieldType::Json => TypeRef::named(JSON_SCALAR),
        FieldType::Uuid => TypeRef::named(UUID_SCALAR),
        FieldType::MultiChoice => TypeRef::named_list(TypeRef::STRING),
        // Decimal/Currency/Percent serialised as String for precision;
        // String-flavoured types as String.
        _ => TypeRef::named(TypeRef::STRING),
    }
}

/// Type ref of the scalar filter input matching this column's type.
///
/// Lookup columns with a UUID target use the string filter (UUIDs are
/// exchanged as strings at the GraphQL boundary).
fn filter_type_ref(ft: FieldType, lookup_target_strategy: IdStrategy) -> TypeRef {
    match ft {
        FieldType::Boolean => TypeRef::named(BOOL_FILTER),
        FieldType::Number | FieldType::AutoIncrement => TypeRef::named(INT_FILTER),
        FieldType::Lookup => match lookup_target_strategy {
            IdStrategy::Bigserial => TypeRef::named(INT_FILTER),
            IdStrategy::Uuid => TypeRef::named(STRING_FILTER),
        },
        // V1: every other column uses the string filter (DateTime/Uuid/Date
        // values are exchanged as strings at the GraphQL boundary).
        _ => TypeRef::named(STRING_FILTER),
    }
}

/// GraphQL type ref of the implicit `id` field, given a table's strategy.
fn id_output_ref(strategy: IdStrategy) -> TypeRef {
    match strategy {
        IdStrategy::Bigserial => TypeRef::named_nn(TypeRef::INT),
        IdStrategy::Uuid => TypeRef::named_nn(TypeRef::STRING),
    }
}

/// GraphQL type ref of the `id` argument on `*ById/update*/delete*`
/// mutations. Same as [`id_output_ref`] but built from the input shape
/// (still `Int!`/`String!`).
fn id_arg_ref(strategy: IdStrategy) -> TypeRef {
    match strategy {
        IdStrategy::Bigserial => TypeRef::named_nn(TypeRef::INT),
        IdStrategy::Uuid => TypeRef::named_nn(TypeRef::STRING),
    }
}

/// Filter input ref for `id` filtering, given a table's strategy.
fn id_filter_ref(strategy: IdStrategy) -> TypeRef {
    match strategy {
        IdStrategy::Bigserial => TypeRef::named(INT_FILTER),
        IdStrategy::Uuid => TypeRef::named(STRING_FILTER),
    }
}

/// Lookup the [`IdStrategy`] of a Lookup column's target table.
fn lookup_target_strategy(
    col: &ColumnDefinition,
    dataverse: &DatabaseSchema,
) -> IdStrategy {
    if col.field_type != FieldType::Lookup {
        return IdStrategy::default();
    }
    let Some(target) = col.lookup_target.as_deref() else {
        return IdStrategy::default();
    };
    dataverse
        .tables
        .iter()
        .find(|t| t.name == target)
        .map(|t| t.id_strategy)
        .unwrap_or_default()
}

/// Build the schema. Caller passes the engine (for resolver access to its
/// pool) and the current Dataverse snapshot.
pub fn build_schema(
    engine: Arc<DataverseEngine>,
    dataverse: Arc<DatabaseSchema>,
) -> Result<Schema> {
    let mut sb = Schema::build("Query", Some("Mutation"), None);

    // Custom scalars
    sb = sb
        .register(Scalar::new(DATETIME_SCALAR))
        .register(Scalar::new(JSON_SCALAR))
        .register(Scalar::new(UUID_SCALAR));

    // SortOrder enum
    sb = sb.register(DynEnum::new("SortOrder").item("ASC").item("DESC"));

    // Scalar filter inputs (shared)
    sb = sb
        .register(int_filter_input())
        .register(string_filter_input())
        .register(bool_filter_input());

    // Per-table types
    let mut query = Object::new("Query");
    let mut mutation = Object::new("Mutation");

    for (idx, table) in dataverse.tables.iter().enumerate() {
        // Type registrations
        sb = sb
            .register(build_object_type(table, &dataverse))
            .register(build_where_input(table, &dataverse))
            .register(build_order_by_input(table))
            .register(build_insert_input(table, &dataverse))
            .register(build_update_input(table, &dataverse));

        // Query fields
        query = query
            .field(field_list(idx, dataverse.clone()))
            .field(field_by_id(idx, dataverse.clone()))
            .field(field_count(idx, dataverse.clone()));

        // Mutation fields
        mutation = mutation
            .field(field_insert(idx, dataverse.clone()))
            .field(field_update(idx, dataverse.clone()))
            .field(field_delete(idx, dataverse.clone()));
    }

    // If the database is empty, both Query and Mutation must still have at
    // least one field per the GraphQL spec. Inject a tautological `_health`
    // field so the schema validates either way.
    let query = query.field(Field::new(
        "_health",
        TypeRef::named_nn(TypeRef::BOOLEAN),
        |_ctx| FieldFuture::new(async move { Ok(Some(FieldValue::value(true))) }),
    ));
    let mutation = mutation.field(Field::new(
        "_health",
        TypeRef::named_nn(TypeRef::BOOLEAN),
        |_ctx| FieldFuture::new(async move { Ok(Some(FieldValue::value(true))) }),
    ));

    sb = sb.register(query).register(mutation);
    sb = sb.data(engine).data(dataverse);

    sb.finish().map_err(map_schema_err)
}

fn map_schema_err(e: SchemaError) -> DataverseError {
    DataverseError::internal(format!("graphql schema build failed: {}", e))
}

// ------------------------------------------------------------------
// Type registration
// ------------------------------------------------------------------

fn int_filter_input() -> InputObject {
    InputObject::new(INT_FILTER)
        .field(InputValue::new("_eq", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("_ne", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("_gt", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("_gte", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("_lt", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("_lte", TypeRef::named(TypeRef::INT)))
        .field(InputValue::new("_in", TypeRef::named_nn_list(TypeRef::INT)))
        .field(InputValue::new("_nin", TypeRef::named_nn_list(TypeRef::INT)))
        .field(InputValue::new("_isNull", TypeRef::named(TypeRef::BOOLEAN)))
}

fn string_filter_input() -> InputObject {
    InputObject::new(STRING_FILTER)
        .field(InputValue::new("_eq", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_ne", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_gt", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_gte", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_lt", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_lte", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_like", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_ilike", TypeRef::named(TypeRef::STRING)))
        .field(InputValue::new("_in", TypeRef::named_nn_list(TypeRef::STRING)))
        .field(InputValue::new("_nin", TypeRef::named_nn_list(TypeRef::STRING)))
        .field(InputValue::new("_isNull", TypeRef::named(TypeRef::BOOLEAN)))
}

fn bool_filter_input() -> InputObject {
    InputObject::new(BOOL_FILTER)
        .field(InputValue::new("_eq", TypeRef::named(TypeRef::BOOLEAN)))
        .field(InputValue::new("_ne", TypeRef::named(TypeRef::BOOLEAN)))
        .field(InputValue::new("_isNull", TypeRef::named(TypeRef::BOOLEAN)))
}

fn build_object_type(table: &TableDefinition, dataverse: &DatabaseSchema) -> Object {
    let type_name = naming::type_name(&table.name);

    let mut obj = Object::new(type_name)
        .field(scalar_field("id", id_output_ref(table.id_strategy), "id"))
        .field(scalar_field(
            "createdAt",
            TypeRef::named_nn(DATETIME_SCALAR),
            "created_at",
        ))
        .field(scalar_field(
            "updatedAt",
            TypeRef::named_nn(DATETIME_SCALAR),
            "updated_at",
        ));

    for col in &table.columns {
        let nullable = !col.required;
        let target_strategy = lookup_target_strategy(col, dataverse);
        let base = output_type_ref(col.field_type, target_strategy);
        let ty = if nullable {
            base
        } else {
            // Convert named to named_nn when required.
            // Lists already carry their non-null markers.
            match col.field_type {
                FieldType::MultiChoice => TypeRef::named_nn_list_nn(TypeRef::STRING),
                _ => TypeRef::named_nn(named_inner_of(&base)),
            }
        };
        // GraphQL field uses camelCase, Postgres column keeps snake_case.
        let gql_name = naming::camel_case(&col.name);
        let pg_name = col.name.clone();
        obj = obj.field(scalar_field(&gql_name, ty, &pg_name));
    }

    obj
}

/// Helper: pull the inner name of a `TypeRef::named(...)`. Used only when
/// we know the input was constructed via `named` (not a list).
fn named_inner_of(t: &TypeRef) -> String {
    // The dynamic API only exposes Display/Debug; we rely on the textual
    // form which for `named(name)` is exactly `name`. Lists become
    // `[name]` etc. — we never wrap a list back into `named_nn`.
    let s = format!("{}", t);
    s
}

/// Build a field that reads its scalar value from the parent JSON map
/// using the given Postgres column name as the key.
fn scalar_field(gql_name: &str, ty: TypeRef, pg_key: &str) -> Field {
    let pg_key = pg_key.to_string();
    Field::new(gql_name, ty, move |ctx: ResolverContext| {
        let key = pg_key.clone();
        FieldFuture::new(async move {
            let parent = ctx
                .parent_value
                .try_downcast_ref::<serde_json::Map<String, JsonValue>>()
                .map_err(|_| async_graphql::Error::new("invalid parent for scalar field"))?;
            let v = parent.get(&key).cloned().unwrap_or(JsonValue::Null);
            Ok(Some(FieldValue::value(json_to_gql(v))))
        })
    })
}

fn build_where_input(table: &TableDefinition, dataverse: &DatabaseSchema) -> InputObject {
    let name = naming::input_where(&table.name);
    let mut io = InputObject::new(&name);

    // _and / _or / _not (recursive on this same input)
    io = io
        .field(InputValue::new("_and", TypeRef::named_nn_list(&name)))
        .field(InputValue::new("_or", TypeRef::named_nn_list(&name)))
        .field(InputValue::new("_not", TypeRef::named(&name)));

    // Implicit columns are first-class filterable. Every row has them
    // and every UI eventually wants to filter on `id` or sort by date.
    io = io
        .field(InputValue::new("id", id_filter_ref(table.id_strategy)))
        .field(InputValue::new("createdAt", TypeRef::named(STRING_FILTER)))
        .field(InputValue::new("updatedAt", TypeRef::named(STRING_FILTER)));

    for col in &table.columns {
        // V1: skip MultiChoice/Json from filtering surface.
        if matches!(col.field_type, FieldType::MultiChoice | FieldType::Json) {
            continue;
        }
        let f = naming::camel_case(&col.name);
        let target_strategy = lookup_target_strategy(col, dataverse);
        io = io.field(InputValue::new(
            &f,
            filter_type_ref(col.field_type, target_strategy),
        ));
    }

    io
}

fn build_order_by_input(table: &TableDefinition) -> InputObject {
    let name = naming::input_order_by(&table.name);
    let sort_order = TypeRef::named("SortOrder");
    let mut io = InputObject::new(&name)
        .field(InputValue::new("id", sort_order.clone()))
        .field(InputValue::new("createdAt", sort_order.clone()))
        .field(InputValue::new("updatedAt", sort_order.clone()));

    for col in &table.columns {
        if matches!(col.field_type, FieldType::MultiChoice | FieldType::Json) {
            continue;
        }
        let f = naming::camel_case(&col.name);
        io = io.field(InputValue::new(&f, sort_order.clone()));
    }
    io
}

/// Pick the input scalar type for a column at insert/update time.
fn input_scalar_for(col: &ColumnDefinition, dataverse: &DatabaseSchema) -> TypeRef {
    match col.field_type {
        FieldType::Boolean => TypeRef::named(TypeRef::BOOLEAN),
        FieldType::Number | FieldType::AutoIncrement => TypeRef::named(TypeRef::INT),
        FieldType::Lookup => match lookup_target_strategy(col, dataverse) {
            IdStrategy::Bigserial => TypeRef::named(TypeRef::INT),
            IdStrategy::Uuid => TypeRef::named(TypeRef::STRING),
        },
        FieldType::Json => TypeRef::named(JSON_SCALAR),
        FieldType::DateTime => TypeRef::named(DATETIME_SCALAR),
        FieldType::Uuid => TypeRef::named(UUID_SCALAR),
        FieldType::MultiChoice => TypeRef::named_nn_list(TypeRef::STRING),
        _ => TypeRef::named(TypeRef::STRING),
    }
}

fn build_insert_input(table: &TableDefinition, dataverse: &DatabaseSchema) -> InputObject {
    let name = naming::input_insert(&table.name);
    let mut io = InputObject::new(&name);

    for col in &table.columns {
        if col.field_type == FieldType::Formula {
            continue; // generated columns can't be inserted into
        }
        let base = input_scalar_for(col, dataverse);
        let ty = if col.required && col.default_value.is_none() {
            match col.field_type {
                FieldType::MultiChoice => TypeRef::named_nn_list_nn(TypeRef::STRING),
                _ => TypeRef::named_nn(named_inner_of(&base)),
            }
        } else {
            base
        };
        let f = naming::camel_case(&col.name);
        io = io.field(InputValue::new(&f, ty));
    }
    io
}

fn build_update_input(table: &TableDefinition, dataverse: &DatabaseSchema) -> InputObject {
    // Update inputs always make every column optional.
    let name = naming::input_update(&table.name);
    let mut io = InputObject::new(&name);
    for col in &table.columns {
        if col.field_type == FieldType::Formula {
            continue;
        }
        let f = naming::camel_case(&col.name);
        let ty = input_scalar_for(col, dataverse);
        io = io.field(InputValue::new(&f, ty));
    }
    io
}

// ------------------------------------------------------------------
// Query field constructors
// ------------------------------------------------------------------

fn field_list(table_idx: usize, dataverse: Arc<DatabaseSchema>) -> Field {
    let table = &dataverse.tables[table_idx];
    let gql_name = naming::field_list(&table.name);
    let row_type = naming::type_name(&table.name);
    let where_ty = naming::input_where(&table.name);
    let order_by_ty = naming::input_order_by(&table.name);
    let dv = dataverse.clone();

    Field::new(
        gql_name,
        TypeRef::named_nn_list_nn(row_type),
        move |ctx: ResolverContext| {
            let dv = dv.clone();
            FieldFuture::new(async move {
                let table = &dv.tables[table_idx];
                let engine = ctx
                    .data::<Arc<DataverseEngine>>()
                    .map_err(|e| async_graphql::Error::new(format!("missing engine: {}", e.message)))?;

                let rows = run_list(engine.clone(), table, &dv, &ctx).await?;
                let values = rows
                    .into_iter()
                    .map(|r| FieldValue::owned_any(r))
                    .collect::<Vec<_>>();
                Ok(Some(FieldValue::list(values)))
            })
        },
    )
    .argument(InputValue::new("where", TypeRef::named(where_ty)))
    .argument(InputValue::new(
        "orderBy",
        TypeRef::named_nn_list(order_by_ty),
    ))
    .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
    .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)))
}

fn field_by_id(table_idx: usize, dataverse: Arc<DatabaseSchema>) -> Field {
    let table = &dataverse.tables[table_idx];
    let gql_name = naming::field_by_id(&table.name);
    let row_type = naming::type_name(&table.name);
    let id_arg = id_arg_ref(table.id_strategy);
    let dv = dataverse.clone();

    Field::new(gql_name, TypeRef::named(row_type), move |ctx: ResolverContext| {
        let dv = dv.clone();
        FieldFuture::new(async move {
            let table = &dv.tables[table_idx];
            let engine = ctx
                .data::<Arc<DataverseEngine>>()
                .map_err(|e| async_graphql::Error::new(format!("missing engine: {}", e.message)))?;
            let id_arg = ctx.args.try_get("id")?.deserialize::<GqlValue>()?;
            let id = IdValue::from_gql(&id_arg, table.id_strategy).map_err(to_gql_err)?;
            let row = run_by_id(engine.clone(), table, &dv, id).await?;
            Ok(row.map(FieldValue::owned_any))
        })
    })
    .argument(InputValue::new("id", id_arg))
}

fn field_count(table_idx: usize, dataverse: Arc<DatabaseSchema>) -> Field {
    let table = &dataverse.tables[table_idx];
    let gql_name = naming::field_count(&table.name);
    let where_ty = naming::input_where(&table.name);
    let dv = dataverse.clone();

    Field::new(
        gql_name,
        TypeRef::named_nn(TypeRef::INT),
        move |ctx: ResolverContext| {
            let dv = dv.clone();
            FieldFuture::new(async move {
                let table = &dv.tables[table_idx];
                let engine = ctx
                    .data::<Arc<DataverseEngine>>()
                    .map_err(|e| async_graphql::Error::new(format!("missing engine: {}", e.message)))?;
                let n = run_count(engine.clone(), table, &dv, &ctx).await?;
                Ok(Some(FieldValue::value(n)))
            })
        },
    )
    .argument(InputValue::new("where", TypeRef::named(where_ty)))
}

// ------------------------------------------------------------------
// Mutation field constructors
// ------------------------------------------------------------------

fn field_insert(table_idx: usize, dataverse: Arc<DatabaseSchema>) -> Field {
    let table = &dataverse.tables[table_idx];
    let gql_name = naming::mutation_insert(&table.name);
    let row_type = naming::type_name(&table.name);
    let insert_ty = naming::input_insert(&table.name);
    let dv = dataverse.clone();

    Field::new(
        gql_name,
        TypeRef::named_nn(row_type),
        move |ctx: ResolverContext| {
            let dv = dv.clone();
            FieldFuture::new(async move {
                let table = &dv.tables[table_idx];
                let engine = ctx
                    .data::<Arc<DataverseEngine>>()
                    .map_err(|e| async_graphql::Error::new(format!("missing engine: {}", e.message)))?;
                let values = ctx.args.try_get("values")?;
                let row = run_insert(
                    engine.clone(),
                    table,
                    &dv,
                    values.deserialize::<GqlValue>()?,
                )
                .await?;
                Ok(Some(FieldValue::owned_any(row)))
            })
        },
    )
    .argument(InputValue::new(
        "values",
        TypeRef::named_nn(insert_ty),
    ))
}

fn field_update(table_idx: usize, dataverse: Arc<DatabaseSchema>) -> Field {
    let table = &dataverse.tables[table_idx];
    let gql_name = naming::mutation_update(&table.name);
    let row_type = naming::type_name(&table.name);
    let update_ty = naming::input_update(&table.name);
    let id_arg = id_arg_ref(table.id_strategy);
    let dv = dataverse.clone();

    Field::new(
        gql_name,
        TypeRef::named(row_type),
        move |ctx: ResolverContext| {
            let dv = dv.clone();
            FieldFuture::new(async move {
                let table = &dv.tables[table_idx];
                let engine = ctx
                    .data::<Arc<DataverseEngine>>()
                    .map_err(|e| async_graphql::Error::new(format!("missing engine: {}", e.message)))?;
                let id_arg = ctx.args.try_get("id")?.deserialize::<GqlValue>()?;
                let id = IdValue::from_gql(&id_arg, table.id_strategy).map_err(to_gql_err)?;
                let values = ctx.args.try_get("values")?.deserialize::<GqlValue>()?;
                let row = run_update(engine.clone(), table, &dv, id, values).await?;
                Ok(row.map(FieldValue::owned_any))
            })
        },
    )
    .argument(InputValue::new("id", id_arg))
    .argument(InputValue::new("values", TypeRef::named_nn(update_ty)))
}

fn field_delete(table_idx: usize, dataverse: Arc<DatabaseSchema>) -> Field {
    let table = &dataverse.tables[table_idx];
    let gql_name = naming::mutation_delete(&table.name);
    let id_arg = id_arg_ref(table.id_strategy);
    let dv = dataverse.clone();

    Field::new(
        gql_name,
        TypeRef::named_nn(TypeRef::BOOLEAN),
        move |ctx: ResolverContext| {
            let dv = dv.clone();
            FieldFuture::new(async move {
                let table = &dv.tables[table_idx];
                let engine = ctx
                    .data::<Arc<DataverseEngine>>()
                    .map_err(|e| async_graphql::Error::new(format!("missing engine: {}", e.message)))?;
                let id_arg = ctx.args.try_get("id")?.deserialize::<GqlValue>()?;
                let id = IdValue::from_gql(&id_arg, table.id_strategy).map_err(to_gql_err)?;
                let removed = run_delete(engine.clone(), table, id).await?;
                Ok(Some(FieldValue::value(removed)))
            })
        },
    )
    .argument(InputValue::new("id", id_arg))
}

// ------------------------------------------------------------------
// Resolver implementations
// ------------------------------------------------------------------

async fn run_list(
    engine: Arc<DataverseEngine>,
    table: &TableDefinition,
    dataverse: &DatabaseSchema,
    ctx: &ResolverContext<'_>,
) -> std::result::Result<Vec<serde_json::Map<String, JsonValue>>, async_graphql::Error> {
    let mut builder = SqlBuilder::new();
    let where_arg: Option<GqlValue> = arg_to_value(ctx, "where")?;
    let where_clause = where_fragment(table, dataverse, where_arg.as_ref(), &mut builder)
        .map_err(to_gql_err)?;

    // SELECT list
    let select_cols = build_select_list(table);
    let mut sql = format!(
        "SELECT {sel} FROM {tbl}",
        sel = select_cols,
        tbl = quote_ident(&table.name),
    );
    if let Some(w) = where_clause {
        sql.push_str(&format!(" WHERE {}", w));
    }

    // ORDER BY
    if let Some(order_value) = arg_to_value::<GqlValue>(ctx, "orderBy")? {
        if let GqlValue::List(items) = order_value {
            let parts = build_order_by(table, &items)?;
            if !parts.is_empty() {
                sql.push_str(" ORDER BY ");
                sql.push_str(&parts.join(", "));
            }
        }
    }

    // LIMIT / OFFSET (default limit = 100, max = 1000)
    let limit = ctx
        .args
        .get("limit")
        .map(|v| v.i64())
        .transpose()?
        .unwrap_or(100)
        .clamp(0, 1000);
    sql.push_str(&format!(" LIMIT {}", limit));

    let offset = ctx
        .args
        .get("offset")
        .map(|v| v.i64())
        .transpose()?
        .unwrap_or(0)
        .max(0);
    if offset > 0 {
        sql.push_str(&format!(" OFFSET {}", offset));
    }

    let args = builder.into_arguments().map_err(to_gql_err)?;
    let pool = engine.pool().clone();
    let rows = sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .fetch_all(&pool)
        .await
        .map_err(|e| to_gql_err(DataverseError::from(e)))?;

    let read_ctx = RowReadCtx::from_table_and_schema(table, dataverse);
    rows.iter()
        .map(|r| row_to_json(r, &table.columns, &read_ctx).map_err(to_gql_err))
        .collect()
}

async fn run_by_id(
    engine: Arc<DataverseEngine>,
    table: &TableDefinition,
    dataverse: &DatabaseSchema,
    id: IdValue,
) -> std::result::Result<Option<serde_json::Map<String, JsonValue>>, async_graphql::Error> {
    let select_cols = build_select_list(table);
    // Cast the placeholder type so PG accepts the bind regardless of
    // strategy. ::BIGINT for Bigserial id, ::UUID for Uuid id.
    let id_cast = match table.id_strategy {
        crate::schema::IdStrategy::Bigserial => "BIGINT",
        crate::schema::IdStrategy::Uuid => "UUID",
    };
    let sql = format!(
        "SELECT {} FROM {} WHERE id = $1::{}",
        select_cols,
        quote_ident(&table.name),
        id_cast,
    );
    let pool = engine.pool().clone();
    let mut builder = SqlBuilder::new();
    let _ = builder.push(id.into_bind());
    let args = builder.into_arguments().map_err(to_gql_err)?;
    let row = sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .fetch_optional(&pool)
        .await
        .map_err(|e| to_gql_err(DataverseError::from(e)))?;

    let read_ctx = RowReadCtx::from_table_and_schema(table, dataverse);
    row.map(|r| row_to_json(&r, &table.columns, &read_ctx).map_err(to_gql_err))
        .transpose()
}

async fn run_count(
    engine: Arc<DataverseEngine>,
    table: &TableDefinition,
    dataverse: &DatabaseSchema,
    ctx: &ResolverContext<'_>,
) -> std::result::Result<i64, async_graphql::Error> {
    let mut builder = SqlBuilder::new();
    let where_arg: Option<GqlValue> = arg_to_value(ctx, "where")?;
    let where_clause = where_fragment(table, dataverse, where_arg.as_ref(), &mut builder)
        .map_err(to_gql_err)?;

    let mut sql = format!("SELECT COUNT(*) FROM {}", quote_ident(&table.name));
    if let Some(w) = where_clause {
        sql.push_str(&format!(" WHERE {}", w));
    }

    let args = builder.into_arguments().map_err(to_gql_err)?;
    let pool = engine.pool().clone();
    let (count,): (i64,) = sqlx_core::query_as::query_as_with::<sqlx_postgres::Postgres, (i64,), _>(&sql, args)
        .fetch_one(&pool)
        .await
        .map_err(|e| to_gql_err(DataverseError::from(e)))?;
    Ok(count)
}

async fn run_insert(
    engine: Arc<DataverseEngine>,
    table: &TableDefinition,
    dataverse: &DatabaseSchema,
    values: GqlValue,
) -> std::result::Result<serde_json::Map<String, JsonValue>, async_graphql::Error> {
    let GqlValue::Object(map) = values else {
        return Err(async_graphql::Error::new("insert values must be an object"));
    };

    // Collect the (column, BindValue) pairs in column order.
    let mut col_idents: Vec<String> = Vec::new();
    let mut placeholders: Vec<String> = Vec::new();
    let mut builder = SqlBuilder::new();
    for col in &table.columns {
        if col.field_type == FieldType::Formula { continue; }
        let camel = naming::camel_case(&col.name);
        let Some(v) = map.get(camel.as_str()) else { continue };
        let target_strategy = lookup_target_strategy(col, dataverse);
        let bind = graphql_value_to_bind(v, col.field_type, target_strategy)
            .map_err(to_gql_err)?;
        match bind {
            Some(b) => {
                col_idents.push(quote_ident(&col.name));
                placeholders.push(builder.push(b));
            }
            None => {
                // Explicit null — pick the right pg type cast: Lookup
                // columns inherit the FK type from the target table.
                let null_pg_type = if col.field_type == FieldType::Lookup {
                    target_strategy.pg_fk_type()
                } else {
                    col.field_type.pg_type()
                };
                col_idents.push(quote_ident(&col.name));
                placeholders.push(builder.push(BindValue::NullAs(null_pg_type)));
            }
        }
    }

    let select_cols = build_select_list(table);
    let sql = if col_idents.is_empty() {
        format!(
            "INSERT INTO {} DEFAULT VALUES RETURNING {}",
            quote_ident(&table.name),
            select_cols,
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({}) RETURNING {}",
            quote_ident(&table.name),
            col_idents.join(", "),
            placeholders.join(", "),
            select_cols,
        )
    };

    let args = builder.into_arguments().map_err(to_gql_err)?;
    let pool = engine.pool().clone();
    let row = sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .fetch_one(&pool)
        .await
        .map_err(|e| to_gql_err(DataverseError::from(e)))?;

    let read_ctx = RowReadCtx::from_table_and_schema(table, dataverse);
    row_to_json(&row, &table.columns, &read_ctx).map_err(to_gql_err)
}

async fn run_update(
    engine: Arc<DataverseEngine>,
    table: &TableDefinition,
    dataverse: &DatabaseSchema,
    id: IdValue,
    values: GqlValue,
) -> std::result::Result<Option<serde_json::Map<String, JsonValue>>, async_graphql::Error> {
    let GqlValue::Object(map) = values else {
        return Err(async_graphql::Error::new("update values must be an object"));
    };

    let mut sets: Vec<String> = Vec::new();
    let mut builder = SqlBuilder::new();
    for col in &table.columns {
        if col.field_type == FieldType::Formula { continue; }
        let camel = naming::camel_case(&col.name);
        let Some(v) = map.get(camel.as_str()) else { continue };
        let target_strategy = lookup_target_strategy(col, dataverse);
        let bind = graphql_value_to_bind(v, col.field_type, target_strategy).map_err(to_gql_err)?;
        match bind {
            Some(b) => {
                let p = builder.push(b);
                sets.push(format!("{} = {}", quote_ident(&col.name), p));
            }
            None => {
                sets.push(format!("{} = NULL", quote_ident(&col.name)));
            }
        }
    }

    if sets.is_empty() {
        // Nothing to update — just return the current row.
        return run_by_id(engine, table, dataverse, id).await;
    }

    let id_p = builder.push(id.into_bind());
    let select_cols = build_select_list(table);
    let sql = format!(
        "UPDATE {} SET {} WHERE id = {} RETURNING {}",
        quote_ident(&table.name),
        sets.join(", "),
        id_p,
        select_cols,
    );

    let args = builder.into_arguments().map_err(to_gql_err)?;
    let pool = engine.pool().clone();
    let row = sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .fetch_optional(&pool)
        .await
        .map_err(|e| to_gql_err(DataverseError::from(e)))?;

    let read_ctx = RowReadCtx::from_table_and_schema(table, dataverse);
    row.map(|r| row_to_json(&r, &table.columns, &read_ctx).map_err(to_gql_err))
        .transpose()
}

async fn run_delete(
    engine: Arc<DataverseEngine>,
    table: &TableDefinition,
    id: IdValue,
) -> std::result::Result<bool, async_graphql::Error> {
    let id_cast = match table.id_strategy {
        crate::schema::IdStrategy::Bigserial => "BIGINT",
        crate::schema::IdStrategy::Uuid => "UUID",
    };
    let sql = format!(
        "DELETE FROM {} WHERE id = $1::{}",
        quote_ident(&table.name),
        id_cast,
    );
    let pool = engine.pool().clone();
    let mut builder = SqlBuilder::new();
    let _ = builder.push(id.into_bind());
    let args = builder.into_arguments().map_err(to_gql_err)?;
    let result = sqlx_core::query::query_with::<sqlx_postgres::Postgres, _>(&sql, args)
        .execute(&pool)
        .await
        .map_err(|e| to_gql_err(DataverseError::from(e)))?;
    Ok(result.rows_affected() > 0)
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn build_select_list(table: &TableDefinition) -> String {
    let mut parts = vec![
        "id".to_string(),
        "created_at".to_string(),
        "updated_at".to_string(),
    ];
    for col in &table.columns {
        parts.push(select_fragment_for(col));
    }
    parts.join(", ")
}

fn build_order_by(
    table: &TableDefinition,
    items: &[GqlValue],
) -> std::result::Result<Vec<String>, async_graphql::Error> {
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        let GqlValue::Object(m) = item else {
            return Err(async_graphql::Error::new("orderBy items must be objects"));
        };
        for (key, v) in m.iter() {
            // Accept both the GraphQL enum form (`ASC`/`DESC`) and a
            // String fallback — JSON variables encode enums as strings,
            // and the dynamic schema otherwise refuses them.
            let dir_raw: &str = match v {
                GqlValue::Enum(s) => s.as_str(),
                GqlValue::String(s) => s.as_str(),
                _ => return Err(async_graphql::Error::new(
                    "orderBy direction must be ASC or DESC",
                )),
            };
            let dir = if dir_raw.eq_ignore_ascii_case("ASC") {
                "ASC"
            } else if dir_raw.eq_ignore_ascii_case("DESC") {
                "DESC"
            } else {
                return Err(async_graphql::Error::new(format!(
                    "orderBy direction must be ASC or DESC, got '{}'", dir_raw
                )));
            };
            // Resolve the key (camelCase) back to a Postgres column name.
            let key_str = key.as_str();
            let pg_col = if key_str == "id" || key_str == "createdAt" || key_str == "updatedAt" {
                match key_str {
                    "createdAt" => "created_at".to_string(),
                    "updatedAt" => "updated_at".to_string(),
                    _ => "id".to_string(),
                }
            } else {
                let mut found: Option<&ColumnDefinition> = None;
                for c in &table.columns {
                    if naming::camel_case(&c.name) == key_str {
                        found = Some(c);
                        break;
                    }
                }
                let Some(col) = found else {
                    return Err(async_graphql::Error::new(format!(
                        "orderBy: unknown column '{}'", key_str
                    )));
                };
                col.name.clone()
            };
            parts.push(format!("{} {}", quote_ident(&pg_col), dir));
        }
    }
    Ok(parts)
}

fn json_to_gql(v: JsonValue) -> GqlValue {
    GqlValue::from_json(v).unwrap_or(GqlValue::Null)
}

fn to_gql_err(e: DataverseError) -> async_graphql::Error {
    async_graphql::Error::new(e.to_string())
}

/// Pull an argument out of `ResolverContext.args` as an owned typed value.
/// Returns `Ok(None)` when the argument is absent or explicitly null.
fn arg_to_value<T: serde::de::DeserializeOwned>(
    ctx: &ResolverContext<'_>,
    name: &str,
) -> std::result::Result<Option<T>, async_graphql::Error> {
    match ctx.args.get(name) {
        None => Ok(None),
        Some(va) => {
            let v: GqlValue = va.deserialize()?;
            if matches!(v, GqlValue::Null) {
                return Ok(None);
            }
            // Round-trip through JSON to satisfy any T.
            let j = v.into_json()?;
            Ok(Some(serde_json::from_value(j).map_err(|e| {
                async_graphql::Error::new(format!("arg '{}' deserialize: {}", name, e))
            })?))
        }
    }
}
