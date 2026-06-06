//! Agent SDK chat — pilote le runner Node (`/opt/atelier/runner`) via le même
//! pattern de sous-process que la surveillance ([`atelier_watcher::codex`]) :
//! on spawn le runner, on lui écrit un JSON d'init sur stdin, on lit son NDJSON
//! ligne à ligne et on republie chaque ligne (normalisée + taggée `run_id`) sur
//! l'EventBus → WebSocket. Le runner tourne en `hr-studio` (OAuth abonnement),
//! jamais en root, et les secrets passent par l'env (jamais par l'argv).
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, instrument, warn};

use atelier_common::events::AgentEvent;

use crate::state::ApiState;

/// Registre des runs en vol : `run_id` → cancel sender. Permet d'arrêter un run
/// depuis un autre handler (POST .../cancel) sans porter l'état dans `ApiState`.
static AGENT_RUNS: LazyLock<Mutex<HashMap<String, oneshot::Sender<()>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Canal de réponse aux questions interactives (AskUserQuestion → `onUserDialog`) :
/// `run_id` → sender de lignes NDJSON à écrire sur le stdin du runner. Alimenté par
/// l'endpoint `/answer` ; consommé par la tâche d'écriture stdin de [`run_agent`].
static AGENT_ANSWERS: LazyLock<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Routes app-scoped, montées sous `/api/apps` (comme la surveillance) :
///   POST /api/apps/{slug}/agent/query
///   POST /api/apps/{slug}/agent/runs/{run_id}/cancel
pub fn app_router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/agent/query", post(query))
        .route("/{slug}/agent/runs/{run_id}/cancel", post(cancel))
        .route("/{slug}/agent/runs/{run_id}/answer", post(answer))
}

/// Routes globales, montées sous `/api/agent` :
///   GET  /api/agent/sdk/version
///   POST /api/agent/sdk/update
pub fn global_router() -> Router<ApiState> {
    Router::new()
        .route("/sdk/version", get(sdk_version))
        .route("/sdk/update", post(sdk_update))
}

// --- Config (env, avec défauts prod) ---

fn node_bin() -> String {
    std::env::var("ATELIER_AGENT_NODE_BIN").unwrap_or_else(|_| "/usr/bin/node".into())
}
fn runner_script() -> String {
    std::env::var("ATELIER_AGENT_RUNNER").unwrap_or_else(|_| "/opt/atelier/runner/src/runner.js".into())
}
fn run_as_user() -> String {
    std::env::var("ATELIER_AGENT_USER").unwrap_or_else(|_| "hr-studio".into())
}
fn claude_config_dir() -> String {
    std::env::var("ATELIER_AGENT_CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|_| "/var/lib/hr-studio/.claude".into())
}
fn mcp_endpoint_base() -> String {
    std::env::var("ATELIER_MCP_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:4100/mcp".into())
}
fn run_timeout() -> Duration {
    let secs = std::env::var("ATELIER_AGENT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600u64);
    Duration::from_secs(secs)
}

#[derive(Debug, Deserialize)]
struct QueryBody {
    prompt: String,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    resume: Option<String>,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    model: Option<String>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

#[instrument(skip(state, body), fields(slug = %slug))]
async fn query(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<QueryBody>,
) -> impl IntoResponse {
    if body.prompt.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "prompt vide");
    }
    // cwd = la source de l'app (même dossier que code-server édite). Doit exister.
    let cwd: PathBuf = state.apps_src_root.join(&slug).join("src");
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, format!("app source introuvable: {}", cwd.display()));
    }

    let run_id = uuid::Uuid::new_v4().to_string();

    // Le runner ne connaît que deux modes : `plan` (défaut, lecture seule) et
    // `bypassPermissions` (pleine capacité). `allowed_tools` n'est qu'un complément
    // d'allowlist côté Plan (Read/Glob/Grep sont déjà inclus par le runner). On
    // n'injecte PAS `mcp__studio__*` ici : ce wildcard ouvrirait `mcp__studio__exec`
    // (exécution EN ROOT via la passerelle) même en Plan. En Bypass, le runner
    // lève toutes les gardes ; le MCP devient disponible.
    let allowed_tools = body.allowed_tools.unwrap_or_else(|| {
        vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()]
    });

    // L'init est consommé par runner.js (clés camelCase). Le token MCP passe ICI,
    // par stdin (pipe) — que ni Atelier ni sudo ne journalisent. (Le passer en
    // env via sudo --preserve-env le ferait apparaître en clair dans journald.)
    let init = json!({
        "prompt": body.prompt,
        "effort": body.effort, // None → null → runner omet (Haiku ne supporte pas effort)
        "permissionMode": body.permission_mode.unwrap_or_else(|| "plan".into()),
        "allowedTools": allowed_tools,
        "cwd": cwd.to_string_lossy(),
        "mcpEndpoint": format!("{}?project={}", mcp_endpoint_base(), slug),
        "mcpToken": std::env::var("MCP_TOKEN").ok(),
        "resume": body.resume,
        "model": body.model,
    });

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    AGENT_RUNS.lock().unwrap().insert(run_id.clone(), cancel_tx);

    info!(run_id = %run_id, "agent run started");
    let events = state.events.clone();
    let run_id_task = run_id.clone();
    let slug_task = slug.clone();
    tokio::spawn(async move {
        run_agent(events, slug_task, run_id_task.clone(), cwd, init.to_string(), cancel_rx).await;
        AGENT_RUNS.lock().unwrap().remove(&run_id_task);
        AGENT_ANSWERS.lock().unwrap().remove(&run_id_task);
    });

    (StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))).into_response()
}

#[instrument(skip(state), fields(slug = %slug, run_id = %run_id))]
async fn cancel(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = &state; // état non requis ; signature homogène avec les autres handlers
    match AGENT_RUNS.lock().unwrap().remove(&run_id) {
        Some(tx) => {
            let _ = tx.send(());
            (StatusCode::OK, Json(json!({"cancelled": true}))).into_response()
        }
        None => err(StatusCode::NOT_FOUND, "run inconnu ou déjà terminé"),
    }
}

/// Réponse à une question interactive (AskUserQuestion). Sérialise une ligne de
/// contrôle `{type:"answer",...}` et l'écrit sur le stdin du runner via le canal
/// du run. `answers` = { texte_question -> réponse } (multi-select joint par virgule).
#[derive(Deserialize)]
struct AnswerBody {
    request_id: String,
    #[serde(default)]
    answers: HashMap<String, String>,
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    cancelled: bool,
}

#[instrument(skip(state, body), fields(slug = %slug, run_id = %run_id))]
async fn answer(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
    Json(body): Json<AnswerBody>,
) -> impl IntoResponse {
    let _ = &state; // signature homogène ; état non requis
    let line = json!({
        "type": "answer",
        "request_id": body.request_id,
        "answers": body.answers,
        "response": body.response,
        "cancelled": body.cancelled,
    })
    .to_string();
    let sent = AGENT_ANSWERS
        .lock()
        .unwrap()
        .get(&run_id)
        .map(|tx| tx.send(line).is_ok())
        .unwrap_or(false);
    if sent {
        info!(run_id = %run_id, "agent question answered");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou terminé")
    }
}

/// Publie un `AgentEvent` taggé sur l'EventBus. `seq` ordonne, le frontend filtre
/// par `run_id`.
fn publish(events: &atelier_common::events::EventBus, run_id: &str, slug: &str, seq: &mut u64, kind: &str, data: Value) {
    let ev = AgentEvent {
        run_id: run_id.to_string(),
        slug: slug.to_string(),
        seq: *seq,
        kind: kind.to_string(),
        data,
    };
    *seq += 1;
    let _ = events.agent.send(ev);
}

/// Spawn le runner Node, écrit l'init sur stdin, lit le NDJSON et republie.
/// Clone direct du pattern [`atelier_watcher::codex::CodexRunner::exec`] :
/// process group + SIGKILL au cancel/timeout pour reaper le binaire `claude`
/// petit-fils du SDK.
async fn run_agent(
    events: std::sync::Arc<atelier_common::events::EventBus>,
    slug: String,
    run_id: String,
    cwd: PathBuf,
    init_json: String,
    mut cancel: oneshot::Receiver<()>,
) {
    let mut seq: u64 = 0;
    publish(&events, &run_id, &slug, &mut seq, "started", json!({}));

    let mut cmd = Command::new("sudo");
    cmd.arg("-n")
        .arg("-H")
        .arg("-u")
        .arg(run_as_user())
        // Whitelist : seul CLAUDE_CONFIG_DIR (non secret) traverse l'env_reset de
        // sudo. Le MCP_TOKEN passe par l'init JSON (stdin), JAMAIS par l'env —
        // sinon sudo le journalise en clair dans son ENV=. ANTHROPIC_API_KEY et
        // les DSN root sont écartés par env_reset (pas dans la whitelist).
        .arg("--preserve-env=CLAUDE_CONFIG_DIR")
        .arg("--")
        .arg(node_bin())
        .arg(runner_script());
    cmd.current_dir(&cwd);
    // Non secret : posé sur l'env de sudo (pas l'argv), préservé par --preserve-env.
    cmd.env("CLAUDE_CONFIG_DIR", claude_config_dir());
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!(?e, "spawn runner failed");
            publish(&events, &run_id, &slug, &mut seq, "error", json!({"message": format!("spawn runner: {e}")}));
            publish(&events, &run_id, &slug, &mut seq, "done", json!({"exit_ok": false}));
            return;
        }
    };
    let child_pid = child.id();

    // stdin reste OUVERT (canal duplex) : on écrit l'init (1 ligne) puis une tâche
    // draine les réponses aux questions vers le stdin du runner. Sans ça, le hook
    // `onUserDialog` du runner n'aurait aucun moyen de recevoir la réponse de l'UI.
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(init_json.as_bytes()).await {
            warn!(?e, "write init to runner stdin failed");
        }
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.flush().await;
        let (ans_tx, mut ans_rx) = mpsc::unbounded_channel::<String>();
        AGENT_ANSWERS.lock().unwrap().insert(run_id.clone(), ans_tx);
        tokio::spawn(async move {
            while let Some(line) = ans_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
            // plus de sender (run terminé / nettoyé) → on ferme stdin (EOF runner).
            let _ = stdin.shutdown().await;
        });
    }

    // Draine stderr (diagnostics runner) pour ne pas bloquer le pipe.
    let stderr_task = child.stderr.take().map(|err| {
        tokio::spawn(async move {
            let mut buf = String::new();
            let _ = BufReader::new(err).read_to_string(&mut buf).await;
            buf
        })
    });

    // Coalescing des deltas : on agrège le texte consécutif d'un même kind et on
    // flush sur changement de kind, à ~200 car, ou à l'EOF. Borne le débit d'events
    // (le canal broadcast 2048 saturerait sous un flot token-par-token).
    let mut pending_text = String::new();
    let mut pending_kind: Option<String> = None;
    let flush = |events: &atelier_common::events::EventBus,
                 run_id: &str,
                 slug: &str,
                 seq: &mut u64,
                 pending_text: &mut String,
                 pending_kind: &mut Option<String>| {
        if let Some(kind) = pending_kind.take() {
            if !pending_text.is_empty() {
                publish(events, run_id, slug, seq, &kind, json!({"text": pending_text.clone()}));
            }
            pending_text.clear();
        }
    };

    let mut cancelled = false;
    let mut timed_out = false;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        let deadline = tokio::time::sleep(run_timeout());
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                _ = &mut cancel => { cancelled = true; break; }
                _ = &mut deadline => { timed_out = true; break; }
                next = lines.next_line() => match next {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        let obj: Value = match serde_json::from_str(trimmed) {
                            Ok(v) => v,
                            Err(_) => { continue; } // ligne non-JSON ignorée (robustesse)
                        };
                        let t = obj.get("t").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        if t == "assistant_delta" || t == "thinking_delta" {
                            let text = obj.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if pending_kind.as_deref() != Some(t.as_str()) {
                                flush(&events, &run_id, &slug, &mut seq, &mut pending_text, &mut pending_kind);
                                pending_kind = Some(t.clone());
                            }
                            pending_text.push_str(text);
                            if pending_text.len() >= 200 {
                                flush(&events, &run_id, &slug, &mut seq, &mut pending_text, &mut pending_kind);
                            }
                        } else {
                            flush(&events, &run_id, &slug, &mut seq, &mut pending_text, &mut pending_kind);
                            publish(&events, &run_id, &slug, &mut seq, &t, obj);
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => { warn!(?e, "runner stdout read error"); break; }
                },
            }
        }
    }
    flush(&events, &run_id, &slug, &mut seq, &mut pending_text, &mut pending_kind);

    if cancelled || timed_out {
        #[cfg(unix)]
        if let Some(pid) = child_pid {
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        let _ = child.start_kill();
    }
    let status = child.wait().await.ok();
    let stderr = match stderr_task {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    let exit_ok = !cancelled && !timed_out && status.map(|s| s.success()).unwrap_or(false);
    if !exit_ok && !cancelled {
        // tail stderr pour le diagnostic (le runner n'y met que des diags, pas de secret)
        let tail: String = stderr.chars().rev().take(800).collect::<String>().chars().rev().collect();
        warn!(run_id = %run_id, timed_out, stderr_tail = %tail, "agent run ended with failure");
    }
    info!(run_id = %run_id, exit_ok, cancelled, timed_out, "agent run done");
    publish(
        &events,
        &run_id,
        &slug,
        &mut seq,
        "done",
        json!({"exit_ok": exit_ok, "cancelled": cancelled, "timed_out": timed_out}),
    );
}

// --- SDK version check / update ---

async fn fetch_latest_sdk() -> Option<String> {
    let url = "https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/latest";
    let resp = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    let v: Value = resp.json().await.ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

fn installed_sdk_version() -> Option<String> {
    let runner = runner_script();
    // .../runner/src/runner.js → .../runner
    let runner_dir = FsPath::new(&runner).parent()?.parent()?;
    let pkg = runner_dir.join("node_modules/@anthropic-ai/claude-agent-sdk/package.json");
    let s = std::fs::read_to_string(pkg).ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

#[instrument]
async fn sdk_version() -> impl IntoResponse {
    let installed = installed_sdk_version();
    let latest = fetch_latest_sdk().await;
    let update_available = matches!((&installed, &latest), (Some(i), Some(l)) if i != l);
    Json(json!({
        "installed": installed,
        "latest": latest,
        "update_available": update_available,
    }))
}

/// MAJ auto du SDK : différée (Phase 1b). Le check de version est livré ; la
/// mise à jour orchestrée (npm install en `romain` + smoke-test + rollback +
/// rsync vers /opt) est encore à implémenter — on ne mute pas la prod à moitié.
#[instrument]
async fn sdk_update() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "MAJ SDK non encore implémentée (Phase 1b) — utiliser `make deploy` pour l'instant"})),
    )
}
