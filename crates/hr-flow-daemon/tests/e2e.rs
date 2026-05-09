//! End-to-end test: spawn the daemon binary as a subprocess, exercise the
//! HTTP surface against a synthetic `apps.json` + a single-step `compose`
//! flow that needs no callback. Validates Phase 1 critical path :
//!
//! - boot + bind
//! - bearer auth (401 / 200)
//! - hot-reload via /v1/_admin/reload
//! - dispatch + run persistence on disk
//! - GET /v1/runs reads the same files

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_atelier-flowd");

struct DaemonHandle {
    child: Child,
    port: u16,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pick_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn write_json(path: &std::path::Path, v: &Value) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(serde_json::to_vec_pretty(v).unwrap().as_slice())
        .unwrap();
}

fn write_str(path: &std::path::Path, s: &str) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(s.as_bytes()).unwrap();
}

async fn wait_healthy(client: &reqwest::Client, url: &str) {
    for _ in 0..50 {
        if let Ok(r) = client.get(url).send().await {
            if r.status() == 200 {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon did not become healthy at {url}");
}

fn spawn_daemon(token: &str, env: &[(&str, String)]) -> DaemonHandle {
    let port = pick_port();
    let mut cmd = Command::new(BIN);
    cmd.env("ATELIER_FLOW_TOKEN", token)
        .env("HR_FLOWD_BIND", format!("127.0.0.1:{port}"))
        .env("HR_FLOWD_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().expect("spawn atelier-flowd");
    DaemonHandle { child, port }
}

#[tokio::test]
async fn boots_and_serves_health() {
    let tmp = tempfile::tempdir().unwrap();
    let apps_json = tmp.path().join("apps.json");
    write_json(&apps_json, &json!([]));
    let token = "smoke-token-must-be-long-enough";

    let handle = spawn_daemon(
        token,
        &[
            ("ATELIER_APPS_JSON", apps_json.display().to_string()),
            ("ATELIER_APPS_RUNTIME_ROOT", tmp.path().display().to_string()),
            ("ATELIER_APPS_SRC_ROOT", tmp.path().display().to_string()),
        ],
    );
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", handle.port);
    wait_healthy(&client, &format!("{base}/v1/health")).await;

    // 401 without bearer
    let r = client
        .get(format!("{base}/v1/definitions?slug=x"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "no bearer → 401");

    // 200 + empty list with bearer
    let r = client
        .get(format!("{base}/v1/definitions?slug=x"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["flows"], json!([]));
}

#[tokio::test]
async fn dispatches_compose_flow_end_to_end() {
    // Layout :
    //   tmp/apps.json
    //   tmp/test-slug/src/flows/hello.toml
    //   tmp/test-slug/runs/                 (created by JsonRunStore)
    let tmp = tempfile::tempdir().unwrap();
    let slug = "test-slug";

    let apps_json = tmp.path().join("apps.json");
    write_json(
        &apps_json,
        &json!([
            { "slug": slug }
        ]),
    );

    let flows_dir = tmp.path().join(slug).join("src").join("flows");
    std::fs::create_dir_all(&flows_dir).unwrap();
    write_str(
        &flows_dir.join("hello.toml"),
        r#"
            name = "hello"
            description = "compose-only smoke"

            [[steps]]
            id = "greet"
            kind = "compose"
            value = "Hello {{ input.name }}"
        "#,
    );

    let token = "test-token-must-be-long-enough";
    let handle = spawn_daemon(
        token,
        &[
            ("ATELIER_APPS_JSON", apps_json.display().to_string()),
            ("ATELIER_APPS_RUNTIME_ROOT", tmp.path().display().to_string()),
            ("ATELIER_APPS_SRC_ROOT", tmp.path().display().to_string()),
        ],
    );

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", handle.port);
    wait_healthy(&client, &format!("{base}/v1/health")).await;

    // Definitions reflect the registry from boot
    let r = client
        .get(format!("{base}/v1/definitions?slug={slug}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let defs: Value = r.json().await.unwrap();
    assert_eq!(defs["flows"][0]["name"], "hello");

    // Trigger a run
    let r = client
        .post(format!("{base}/v1/runs"))
        .bearer_auth(token)
        .json(&json!({
            "slug": slug,
            "flow_name": "hello",
            "input": { "name": "Romain" },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "POST /v1/runs status");
    let result: Value = r.json().await.unwrap();
    assert_eq!(result["status"], "success");
    assert!(result["run_id"].as_str().unwrap().len() > 8);

    // Persisted on disk where the Atelier API viewer reads from
    let runs_dir = tmp.path().join(slug).join("runs");
    let entries: Vec<_> = std::fs::read_dir(&runs_dir).unwrap().flatten().collect();
    assert!(!entries.is_empty(), "run JSON should exist on disk");

    // GET /v1/runs lists it
    let r = client
        .get(format!("{base}/v1/runs?slug={slug}&limit=10"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let listing: Value = r.json().await.unwrap();
    assert_eq!(listing["runs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn admin_reload_picks_up_new_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let slug = "reload-slug";
    let apps_json = tmp.path().join("apps.json");
    write_json(&apps_json, &json!([{ "slug": slug }]));
    let flows_dir = tmp.path().join(slug).join("src").join("flows");
    std::fs::create_dir_all(&flows_dir).unwrap();

    let token = "reload-token-must-be-long-enough";
    let handle = spawn_daemon(
        token,
        &[
            ("ATELIER_APPS_JSON", apps_json.display().to_string()),
            ("ATELIER_APPS_RUNTIME_ROOT", tmp.path().display().to_string()),
            ("ATELIER_APPS_SRC_ROOT", tmp.path().display().to_string()),
        ],
    );
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", handle.port);
    wait_healthy(&client, &format!("{base}/v1/health")).await;

    // Initially zero flows
    let r = client
        .get(format!("{base}/v1/definitions?slug={slug}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let defs: Value = r.json().await.unwrap();
    assert_eq!(defs["flows"].as_array().unwrap().len(), 0);

    // Add a flow file then reload
    write_str(
        &flows_dir.join("late.toml"),
        r#"
            name = "late"
            [[steps]]
            id = "x"
            kind = "compose"
            value = "later"
        "#,
    );
    let r = client
        .post(format!("{base}/v1/_admin/reload"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["flows_loaded"], 1);

    // Now visible
    let r = client
        .get(format!("{base}/v1/definitions?slug={slug}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let defs: Value = r.json().await.unwrap();
    assert_eq!(defs["flows"][0]["name"], "late");
}
