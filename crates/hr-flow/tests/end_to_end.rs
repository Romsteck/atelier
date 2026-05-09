//! End-to-end tests covering the executor + persistence wiring.

use std::sync::Arc;

use async_trait::async_trait;
use hr_flow::{
    parse_flow_toml, Connector, FlowEngineBuilder, FlowResult, JsonRunStore, RunStatus,
};
use serde_json::{json, Value};

/// In-memory connector that echoes its params back; useful for asserting
/// substitution + dispatch without a real network call.
struct EchoConnector;

#[async_trait]
impl Connector for EchoConnector {
    fn name(&self) -> &str { "echo" }
    async fn call(&self, op: &str, params: Value) -> FlowResult<Value> {
        Ok(json!({ "op": op, "params": params }))
    }
}

#[tokio::test]
async fn runs_simple_compose_and_persists() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "hello"

        [[steps]]
        id = "msg"
        kind = "compose"
        value = "hi {{ input.name }}"
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow).with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let result = engine.run("hello", json!({ "name": "world" })).await.unwrap();
    assert_eq!(result.status, RunStatus::Success);
    assert_eq!(result.output, Some(Value::String("hi world".into())));

    // Run was persisted
    let run = engine.store().load(&result.run_id).await.unwrap();
    assert_eq!(run.flow_name, "hello");
    assert_eq!(run.steps.len(), 1);
    assert_eq!(run.steps[0].step_id, "msg");
    assert_eq!(run.steps[0].kind, "compose");
}

#[tokio::test]
async fn runs_for_each_with_if_and_set_var() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "count_big"

        [[steps]]
        id = "init"
        kind = "set_var"
        name = "count"
        value = 0

        [[steps]]
        id = "loop"
        kind = "for_each"
        over = "input.numbers"
        as = "n"
        needs = ["init"]

        [[steps]]
        id = "branch"
        parent = "loop"
        kind = "if"
        cond = "{{ @n }} > 10"

        [[steps]]
        id = "bump"
        parent = "branch"
        parent_branch = "then"
        kind = "increment_var"
        name = "count"
        by = 1

        [[steps]]
        id = "report"
        kind = "compose"
        value = "{{ vars.count }}"
        needs = ["loop"]
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow).register_connector("echo", Arc::new(EchoConnector));
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("count_big", json!({ "numbers": [1, 12, 5, 20, 3] })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    assert_eq!(r.output, Some(json!(2.0)));
}

#[tokio::test]
async fn errors_propagate_with_step_id() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "boom"

        [[steps]]
        id = "explode"
        kind = "connector"
        connector = "echo"
        op = "request"
        url = "{{ input.does_not_exist }}"
    "#).unwrap();

    // Use a connector that *does* fail on op != "echo"
    struct Fail;
    #[async_trait]
    impl Connector for Fail {
        fn name(&self) -> &str { "echo" }
        async fn call(&self, op: &str, _params: Value) -> FlowResult<Value> {
            Err(hr_flow::FlowError::Connector(format!("nope: {op}")))
        }
    }

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow).register_connector("echo", Arc::new(Fail));
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("boom", json!({})).await.unwrap();
    assert_eq!(r.status, RunStatus::Failed);
    let err = r.error.unwrap();
    assert_eq!(err.step_id, "explode");
    assert!(err.message.contains("nope"), "got: {}", err.message);

    // The trace was still saved with the failed step
    let run = engine.store().load(&r.run_id).await.unwrap();
    assert_eq!(run.status, "failed");
    assert!(run.steps.iter().any(|s| s.step_id == "explode" && s.status == "failed"));
}

fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("hr-flow-tests-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[tokio::test]
async fn data_ops_filter_sort_take_select() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "data_ops"

        [[steps]]
        id = "uncat"
        kind = "filter"
        from = "input.txs"
        cond = "{{ @iter.cat }} == null"

        [[steps]]
        id = "by_amount"
        kind = "sort"
        from = "steps.uncat.output"
        by = "@iter.amount"
        desc = true
        needs = ["uncat"]

        [[steps]]
        id = "top2"
        kind = "take"
        from = "steps.by_amount.output"
        count = 2
        needs = ["by_amount"]

        [[steps]]
        id = "labels"
        kind = "select"
        from = "steps.top2.output"
        project = "tx-{{ @iter.id }}"
        needs = ["top2"]
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("data_ops", json!({
        "txs": [
            { "id": 1, "amount": 50, "cat": "Food" },
            { "id": 2, "amount": 200, "cat": null },
            { "id": 3, "amount": 800, "cat": null },
            { "id": 4, "amount": 30, "cat": null },
        ]
    })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    // Filter keeps the 3 uncategorised, sort desc by amount, take 2 → [800, 200]
    assert_eq!(r.output, Some(json!(["tx-3", "tx-2"])));
}

#[tokio::test]
async fn data_ops_group_dedupe_join_partition() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "more_data_ops"

        [[steps]]
        id = "by_cat"
        kind = "group_by"
        from = "input.items"
        by = "@iter.cat"

        [[steps]]
        id = "uniq"
        kind = "dedupe"
        from = "input.tags"

        [[steps]]
        id = "joined"
        kind = "join"
        from = "steps.uniq.output"
        separator = ","
        needs = ["uniq"]

        [[steps]]
        id = "split"
        kind = "partition"
        from = "input.items"
        cond = "{{ @iter.amount }} > 100"

        [[steps]]
        id = "out"
        kind = "compose"
        needs = ["by_cat", "joined", "split"]
        value = { groups = "{{ steps.by_cat.output }}", uniq_csv = "{{ steps.joined.output }}", big_count = "{{ steps.split.output }}" }
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("more_data_ops", json!({
        "items": [
            { "id": 1, "amount": 50,  "cat": "A" },
            { "id": 2, "amount": 150, "cat": "B" },
            { "id": 3, "amount": 200, "cat": "A" },
        ],
        "tags": ["x", "y", "x", "z", "y"]
    })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    let out = r.output.unwrap();
    assert_eq!(out["uniq_csv"], json!("x,y,z"));
    let groups = &out["groups"];
    assert_eq!(groups["A"].as_array().unwrap().len(), 2);
    assert_eq!(groups["B"].as_array().unwrap().len(), 1);
    let split = &out["big_count"];
    assert_eq!(split["matching"].as_array().unwrap().len(), 2);
    assert_eq!(split["non_matching"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn data_ops_parse_json() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "parse"

        [[steps]]
        id = "parsed"
        kind = "parse_json"
        from = "input.raw"

        [[steps]]
        id = "out"
        kind = "compose"
        needs = ["parsed"]
        value = "{{ steps.parsed.output.name }}"
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("parse", json!({ "raw": "{\"name\":\"alice\",\"age\":30}" })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    assert_eq!(r.output, Some(json!("alice")));
}

#[tokio::test]
async fn p0_lex_string_compare() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "lex"

        [[steps]]
        id = "in_window"
        kind = "filter"
        from = "input.txs"
        cond = '{{ @iter.date }} >= "2026-04-01" && {{ @iter.date }} < "2026-05-01"'
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("lex", json!({
        "txs": [
            { "id": 1, "date": "2026-03-15" },
            { "id": 2, "date": "2026-04-10" },
            { "id": 3, "date": "2026-04-30" },
            { "id": 4, "date": "2026-05-01" },
        ]
    })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    let arr = r.output.unwrap();
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], json!(2));
    assert_eq!(arr[1]["id"], json!(3));
}

#[tokio::test]
async fn p0_if_truthy_on_array_object() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "truthy"

        [[steps]]
        id = "list"
        kind = "compose"
        value = []

        [[steps]]
        id = "branch_empty"
        kind = "if"
        needs = ["list"]
        cond = "{{ steps.list.output }}"

        [[steps]]
        id = "should_not_run"
        parent = "branch_empty"
        parent_branch = "then"
        kind = "compose"
        value = "ran"

        [[steps]]
        id = "list2"
        kind = "compose"
        value = [1, 2]

        [[steps]]
        id = "branch_full"
        kind = "if"
        needs = ["list2"]
        cond = "{{ steps.list2.output }}"

        [[steps]]
        id = "did_run"
        parent = "branch_full"
        parent_branch = "then"
        kind = "compose"
        value = "ran"
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("truthy", json!({})).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
}

#[tokio::test]
async fn p1_iter_index_in_for_each() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "indexed"

        [[steps]]
        id = "init"
        kind = "set_var"
        name = "rows"
        value = []

        [[steps]]
        id = "loop"
        kind = "for_each"
        over = "input.items"
        as = "tx"
        needs = ["init"]

        [[steps]]
        id = "row"
        parent = "loop"
        kind = "append_to_var"
        name = "rows"
        value = "{{ @tx_index }}-{{ @tx.label }}"

        [[steps]]
        id = "out"
        kind = "compose"
        needs = ["loop"]
        value = "{{ vars.rows }}"
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("indexed", json!({
        "items": [{ "label": "a" }, { "label": "b" }, { "label": "c" }]
    })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    assert_eq!(r.output, Some(json!(["0-a", "1-b", "2-c"])));
}

#[tokio::test]
async fn p1_length_and_starts_with() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "fns"

        [[steps]]
        id = "n"
        kind = "length"
        from = "input.txs"

        [[steps]]
        id = "matching"
        kind = "filter"
        from = "input.txs"
        cond = 'starts_with({{ @iter.cat }}, "A categoriser")'

        [[steps]]
        id = "out"
        kind = "compose"
        needs = ["n", "matching"]
        value = { count = "{{ steps.n.output }}", matches = "{{ steps.matching.output }}" }
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("fns", json!({
        "txs": [
            { "id": 1, "cat": "Restaurants" },
            { "id": 2, "cat": "A categoriser - rentree d'argent" },
            { "id": 3, "cat": "A categoriser" },
        ]
    })).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    let out = r.output.unwrap();
    assert_eq!(out["count"], json!(3));
    assert_eq!(out["matches"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn p1_null_literal_in_compose() {
    let dir = tempdir();
    let store = JsonRunStore::new(&dir).unwrap();

    let flow = parse_flow_toml(r#"
        name = "null_lit"

        [[steps]]
        id = "out"
        kind = "compose"
        value = { reset_field = "{{ null }}", flag = "{{ true }}" }
    "#).unwrap();

    let mut b = FlowEngineBuilder::new();
    b.register_flow(flow);
    b.with_store(Arc::new(store));
    let engine = b.build().unwrap();

    let r = engine.run("null_lit", json!({})).await.unwrap();
    assert_eq!(r.status, RunStatus::Success);
    let out = r.output.unwrap();
    assert!(out["reset_field"].is_null());
    assert_eq!(out["flag"], json!(true));
}
