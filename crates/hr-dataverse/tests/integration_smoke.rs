//! End-to-end smoke test against a real Postgres instance.
//!
//! Gated behind `#[ignore]` because it needs `HR_DATAVERSE_TEST_ADMIN_URL`
//! to be set to a Postgres DSN with `CREATEDB` + `CREATEROLE`. Run with:
//!
//! ```text
//! HR_DATAVERSE_TEST_ADMIN_URL=postgres://… \
//!   cargo test -p hr-dataverse --test integration_smoke -- --ignored --nocapture
//! ```
//!
//! The test provisions an app named `smoke_<rand>`, exercises schema-ops +
//! REST/SQL CRUD against the live engine, and tears the database down.

use std::collections::BTreeMap;

use chrono::Utc;
use hr_common::Identity;
use hr_dataverse::{
    ColumnDefinition, DataverseManager, FieldType, IdStrategy, ProvisioningConfig,
    TableDefinition,
    crud::{build_get, build_insert, build_soft_delete},
    dv_io::{MutationOutcome, run_get, run_list, run_mutation},
    query::{ListQuery, build_list_sql},
};
use serde_json::Value;

fn test_admin_url() -> Option<String> {
    std::env::var("HR_DATAVERSE_TEST_ADMIN_URL").ok()
}

fn test_host() -> String {
    std::env::var("HR_DATAVERSE_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into())
}

fn random_slug() -> String {
    use rand::RngCore;
    let mut b = [0u8; 4];
    rand::rng().fill_bytes(&mut b);
    format!("smoke_{:08x}", u32::from_be_bytes(b))
}

fn col(name: &str, ty: FieldType, required: bool) -> ColumnDefinition {
    ColumnDefinition {
        name: name.to_string(),
        field_type: ty,
        required,
        unique: false,
        default_value: None,
        description: None,
        choices: vec![],
        formula_expression: None,
        lookup_target: None,
    }
}

#[tokio::test]
#[ignore]
async fn full_dataverse_lifecycle() {
    let admin_url = test_admin_url()
        .expect("HR_DATAVERSE_TEST_ADMIN_URL not set");

    let cfg = ProvisioningConfig { host: test_host(), port: 5432 };
    let manager = DataverseManager::connect_admin(admin_url, cfg, None)
        .await
        .expect("connect admin");

    let slug = random_slug();
    println!(">>> provisioning app '{}'", slug);

    let secret = manager.provision(&slug).await.expect("provision");
    println!(">>> provisioned: db={} role={}", secret.db_name, secret.role_name);

    // Inject the DSN so engine_for finds it (no on-disk secrets).
    manager.set_dsn_override(&slug, secret.dsn.clone()).await;

    // Always tear down, even if assertions panic.
    let result = std::panic::AssertUnwindSafe(async {
        run_assertions(&manager, &slug).await
    });
    let outcome = futures_util::FutureExt::catch_unwind(result).await;

    println!(">>> cleaning up '{}'", slug);
    if let Err(e) = manager.drop_app(&slug).await {
        eprintln!("drop_app failed: {}", e);
    }

    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn run_assertions(manager: &DataverseManager, slug: &str) {
    let engine = manager.engine_for(slug).await.expect("engine_for");

    // ── schema-ops ────────────────────────────────────────────────
    let now = Utc::now();
    let contacts = TableDefinition {
        name: "contacts".into(),
        slug: "contacts".into(),
        columns: vec![
            col("email", FieldType::Email, true),
            col("age", FieldType::Number, false),
            col("active", FieldType::Boolean, false),
        ],
        description: Some("smoke-test table".into()),
        id_strategy: IdStrategy::Bigserial,
        created_at: now,
        updated_at: now,
    };
    let v1 = engine.create_table(&contacts).await.expect("create_table");
    assert!(v1 > 1, "schema_version should bump beyond 1, got {}", v1);

    let tables = engine.list_tables().await.expect("list_tables");
    assert_eq!(tables, vec!["contacts".to_string()]);

    let count_before = engine.count_rows("contacts").await.expect("count_rows");
    assert_eq!(count_before, 0);

    // ── REST/SQL : insert + list + soft_delete via the dv_io helpers ──
    let snapshot = engine.get_schema().await.expect("get_schema");
    let table_def = snapshot
        .tables
        .iter()
        .find(|t| t.name == "contacts")
        .expect("contacts table in snapshot");
    let identity = Identity::system();

    // Insert
    let mut payload: BTreeMap<String, Value> = BTreeMap::new();
    payload.insert("email".into(), Value::String("a@b.c".into()));
    payload.insert("age".into(), Value::from(42));
    payload.insert("active".into(), Value::Bool(true));
    let insert_mut = build_insert(table_def, &payload, &identity).expect("build_insert");
    let inserted = match run_mutation(
        engine.pool(), table_def,
        &insert_mut.sql, &insert_mut.params,
        None, &[], &Value::Null,
    )
    .await
    .expect("run_mutation insert")
    {
        MutationOutcome::Applied(row) => row,
        other => panic!("unexpected insert outcome: {other:?}"),
    };
    let row_id = inserted.get("id").cloned().expect("inserted id");
    let row_version = inserted
        .get("version")
        .and_then(|v| v.as_i64())
        .expect("inserted version") as i32;

    let count_after = engine.count_rows("contacts").await.expect("count_rows post-insert");
    assert_eq!(count_after, 1);

    // List with $filter via dvexpr
    let lq = ListQuery {
        filter: Some("age > 18".into()),
        select: Vec::new(),
        orderby: Vec::new(),
        top: Some(10),
        skip: None,
        count: false,
        include_deleted: false,
    };
    let compiled = build_list_sql(table_def, &lq, &identity).expect("build_list_sql");
    let rows = run_list(engine.pool(), table_def, &compiled).await.expect("run_list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("email").and_then(|v| v.as_str()), Some("a@b.c"));

    // Read-by-id round trip
    let get = build_get(table_def, &row_id, false);
    let got = run_get(engine.pool(), table_def, &get.sql, &get.params)
        .await
        .expect("run_get");
    assert!(got.is_some(), "row should be readable after insert");

    // Soft-delete
    let del = build_soft_delete(table_def, &row_id, row_version, &identity)
        .expect("build_soft_delete");
    match run_mutation(
        engine.pool(), table_def,
        &del.sql, &del.params,
        None, &[], &row_id,
    )
    .await
    .expect("run_mutation delete")
    {
        MutationOutcome::Applied(_) => {}
        other => panic!("unexpected delete outcome: {other:?}"),
    }

    // After soft-delete, the row is filtered out of default lists.
    let lq2 = ListQuery {
        filter: None,
        select: Vec::new(),
        orderby: Vec::new(),
        top: Some(10),
        skip: None,
        count: false,
        include_deleted: false,
    };
    let compiled2 = build_list_sql(table_def, &lq2, &identity).expect("build_list_sql 2");
    let rows2 = run_list(engine.pool(), table_def, &compiled2).await.expect("run_list 2");
    assert!(rows2.is_empty(), "soft-deleted row should be hidden");

    println!("    schema_version after create_table: {}", v1);
}
