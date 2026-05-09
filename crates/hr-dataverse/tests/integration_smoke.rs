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
//! GraphQL CRUD against the live engine, and tears the database down.

use std::sync::Arc;

use chrono::Utc;
use hr_dataverse::{
    ColumnDefinition, DataverseEngine, DataverseManager, FieldType, ProvisioningConfig,
    TableDefinition,
};

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
        created_at: now,
        updated_at: now,
    };
    let v1 = engine.create_table(&contacts).await.expect("create_table");
    assert!(v1 > 1, "schema_version should bump beyond 1, got {}", v1);

    let tables = engine.list_tables().await.expect("list_tables");
    assert_eq!(tables, vec!["contacts".to_string()]);

    let count_before = engine.count_rows("contacts").await.expect("count_rows");
    assert_eq!(count_before, 0);

    // ── GraphQL : introspection + insert + query + delete ─────────
    let engine_arc: Arc<DataverseEngine> = engine.clone();
    let schema = engine_arc.graphql_schema().await.expect("graphql_schema");

    // Insert via GraphQL mutation
    let insert_resp = schema
        .execute(
            r#"mutation {
                 insertContacts(values: { email: "a@b.c", age: 42, active: true }) {
                   id email age active
                 }
               }"#,
        )
        .await;
    assert!(insert_resp.errors.is_empty(), "insert errors: {:?}", insert_resp.errors);
    let data_str = format!("{:?}", insert_resp.data);
    assert!(data_str.contains("a@b.c"), "insert data should contain email: {}", data_str);

    let count_after = engine.count_rows("contacts").await.expect("count_rows post-insert");
    assert_eq!(count_after, 1);

    // Query via GraphQL with a where filter
    let query_resp = schema
        .execute(
            r#"{ contacts(where: { age: { _gt: 18 } }) { id email age } contactsCount }"#,
        )
        .await;
    assert!(query_resp.errors.is_empty(), "query errors: {:?}", query_resp.errors);
    let qstr = format!("{:?}", query_resp.data);
    assert!(qstr.contains("a@b.c"), "query should return our row: {}", qstr);

    // Introspect: SDL should describe the Contacts type
    let sdl = schema.sdl();
    assert!(sdl.contains("Contacts"), "SDL should declare Contacts type");
    assert!(sdl.contains("insertContacts"), "SDL should declare mutation");

    // Delete via GraphQL — read the id from the previous data
    // (we know there's only one row, get_schema's count is enough)
    let delete_resp = schema
        .execute(
            r#"mutation {
                 # Use contactsById(id: 1) — first auto-incremented row
                 a: deleteContacts(id: 1)
               }"#,
        )
        .await;
    assert!(delete_resp.errors.is_empty(), "delete errors: {:?}", delete_resp.errors);

    let final_count = engine.count_rows("contacts").await.expect("final count");
    assert_eq!(final_count, 0, "row should be deleted");

    println!("    schema_version after create_table: {}", v1);
    println!("    SDL preview:\n{}", &sdl[..sdl.len().min(400)]);
}
