//! Agent SDK chat — pilote le runner Node (`/opt/atelier/runner`) via le même
//! pattern de sous-process que la surveillance ([`atelier_watcher::codex`]) :
//! on spawn le runner, on lui écrit un JSON d'init sur stdin, on lit son NDJSON
//! ligne à ligne et on republie chaque ligne (normalisée + taggée `run_id`) sur
//! l'EventBus → WebSocket. Le runner tourne en `hr-studio` (OAuth abonnement),
//! jamais en root, et les secrets passent par l'env (jamais par l'argv).
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// État d'un run en vol (= une session SDK vivante), indexé par `run_id` dans [`RUNS`]
/// + un index `session_id → run_id` ([`SID_RUN`], rempli dès le `system` du runner).
///
/// `items` est le transcript NORMALISÉ accumulé en mémoire. WHY : le SDK persiste la
/// session sur disque de façon incrémentale, mais (a) le runner ne ré-émet PAS les tours
/// utilisateur (on les ajoute ici) et (b) servir le snapshot depuis ce buffer évite de
/// spawn un runner `op:messages` à chaque requête tant que la session vit.
/// `cancel_tx`/`input_tx` pilotent l'arrêt et le stdin (tours/réponses).
struct RunState {
    slug: String,
    session_id: Option<String>,
    cancel_tx: Option<oneshot::Sender<()>>,
    input_tx: mpsc::UnboundedSender<String>,
    items: Vec<Value>,
    /// Mode courant côté UI ('plan' | 'bypass'). Mis à jour par les events `permission_mode`
    /// (approbation de plan, /set_mode) → exposé dans le snapshot pour survivre au reload.
    mode: String,
    /// Un tour est-il en vol ? `true` du dépôt d'un tour (init/`message`/`answer`/
    /// `plan_decision`) jusqu'au `turn_done`/`done`. Exposé dans le snapshot (`running`,
    /// pour restaurer l'indicateur de réflexion après un refresh) et utilisé par le drain.
    turn_active: bool,
}

static RUNS: LazyLock<Mutex<HashMap<String, RunState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SID_RUN: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Verrou single-flight de la MAJ SDK : `npm install` n'est pas transactionnel, deux MAJ
/// concurrentes corrompraient le snapshot/rollback. Posé/levé par [`sdk_update`] (garde RAII).
static SDK_UPDATING: AtomicBool = AtomicBool::new(false);

/// Écrit une ligne NDJSON sur le stdin du runner d'un run. `false` si inconnu/terminé.
fn send_input(run_id: &str, line: String) -> bool {
    RUNS.lock().unwrap().get(run_id).map(|r| r.input_tx.send(line).is_ok()).unwrap_or(false)
}

fn user_item(text: &str) -> Value {
    json!({ "type": "user", "text": text })
}

/// Replie un event (kind+data) dans un buffer transcript. Miroir EXACT d'`appendEvent`
/// côté frontend : deltas consécutifs coalescés dans le dernier item, autres kinds =
/// items discrets. started/system/turn_done/done ne sont pas rendus.
fn fold_item(items: &mut Vec<Value>, kind: &str, data: &Value) {
    let text = data.get("text").and_then(|x| x.as_str()).unwrap_or("");
    match kind {
        "assistant_delta" | "thinking_delta" => {
            let ty = if kind == "assistant_delta" { "assistant" } else { "thinking" };
            if let Some(last) = items.last_mut() {
                if last.get("type").and_then(|x| x.as_str()) == Some(ty) {
                    let prev = last.get("text").and_then(|x| x.as_str()).unwrap_or("");
                    last["text"] = json!(format!("{prev}{text}"));
                    return;
                }
            }
            items.push(json!({ "type": ty, "text": text }));
        }
        "tool_use" => items.push(json!({ "type": "tool_use", "name": data.get("name").cloned(), "input": data.get("input").cloned(), "id": data.get("id").cloned() })),
        "tool_result" => items.push(json!({
            "type": "tool_result",
            "text": data.get("text").and_then(|x| x.as_str()).unwrap_or(""),
            "isError": data.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
            "tool_use_id": data.get("tool_use_id").cloned(),
        })),
        "result" => items.push(json!({ "type": "result", "data": data.clone() })),
        "question" => items.push(json!({
            "type": "question",
            "request_id": data.get("request_id").cloned(),
            "questions": data.get("questions").cloned().unwrap_or_else(|| json!([])),
        })),
        "plan_review" => items.push(json!({
            "type": "plan_review",
            "request_id": data.get("request_id").cloned(),
            "plan": data.get("plan").and_then(|x| x.as_str()).unwrap_or(""),
        })),
        "error" => items.push(json!({ "type": "error", "message": data.get("message").and_then(|x| x.as_str()).unwrap_or("erreur") })),
        _ => {}
    }
}

/// Replie l'event dans le buffer du run (si encore présent au registre).
fn fold_into_run(run_id: &str, kind: &str, data: &Value) {
    if let Some(r) = RUNS.lock().unwrap().get_mut(run_id) {
        // `permission_mode` n'est pas un item de transcript : il met à jour le mode courant
        // (exposé dans le snapshot pour survivre au reload), pas le fil.
        if kind == "permission_mode" {
            if let Some(m) = data.get("mode").and_then(|x| x.as_str()) {
                r.mode = m.to_string();
            }
        }
        // Fin de tour → le tour n'est plus en vol (snapshot `running` repasse à false).
        if kind == "turn_done" || kind == "done" {
            r.turn_active = false;
        }
        fold_item(&mut r.items, kind, data);
    }
}

/// Routes app-scoped, montées sous `/api/apps` (comme la surveillance) :
///   POST /api/apps/{slug}/agent/query                    (démarre la session + 1er tour)
///   POST /api/apps/{slug}/agent/runs/{run_id}/message    (tour utilisateur suivant)
///   POST /api/apps/{slug}/agent/runs/{run_id}/answer     (réponse AskUserQuestion)
///   POST /api/apps/{slug}/agent/runs/{run_id}/cancel     (termine la session)
pub fn app_router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/agent/query", post(query))
        .route("/{slug}/agent/runs/{run_id}/message", post(message))
        .route("/{slug}/agent/runs/{run_id}/cancel", post(cancel))
        .route("/{slug}/agent/runs/{run_id}/interrupt", post(interrupt))
        .route("/{slug}/agent/runs/{run_id}/answer", post(answer))
        .route("/{slug}/agent/runs/{run_id}/plan_decision", post(plan_decision))
        .route("/{slug}/agent/runs/{run_id}/set_mode", post(set_mode))
        .route("/{slug}/agent/runs/{run_id}/set_model", post(set_model))
        // Conversations = sessions SDK persistées (CLAUDE_CONFIG_DIR), exposées via
        // le runner en mode introspection. La clé est le `session_id` SDK.
        .route("/{slug}/agent/conversations", get(list_conversations))
        .route(
            "/{slug}/agent/conversations/{sid}",
            get(get_conversation).patch(rename_conversation).delete(delete_conversation),
        )
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
fn npm_bin() -> String {
    std::env::var("ATELIER_NPM_BIN").unwrap_or_else(|_| "/usr/bin/npm".into())
}
/// Budget d'un `npm install` (MAJ SDK). Au-delà on tue + rollback.
fn npm_timeout() -> Duration {
    let secs = std::env::var("ATELIER_NPM_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180u64);
    Duration::from_secs(secs)
}
/// Racine du runner installé (= parent de `src/`), où vivent `node_modules` + manifests npm.
/// `.../runner/src/runner.js` → `.../runner`. Source unique partagée par la MAJ et le check de version.
fn runner_dir() -> Option<PathBuf> {
    FsPath::new(&runner_script()).parent()?.parent().map(|p| p.to_path_buf())
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
/// Timeout d'INACTIVITÉ d'une session : ré-armé à chaque ligne stdout du runner.
/// Une session vivante (qui pense ou stream) n'est jamais reapée ; une session
/// oubliée en `turn-idle` l'est après ce délai. (≠ ancien timeout wall-clock qui
/// tuait un run long.)
fn idle_timeout() -> Duration {
    let secs = std::env::var("ATELIER_AGENT_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1800u64);
    Duration::from_secs(secs)
}
/// Plafond DUR d'un tour EN VOL. WHY : l'idle ci-dessus est ré-armé sur le stdout du
/// runner, or un sous-agent (`Task`) n'émet AUCUN stdout côté parent — un tour long mais
/// légitime serait SIGKILL à 1800s, ce qui tronque la session (tour pendouillant). Tant
/// qu'un tour est actif on applique ce plafond large ; l'idle court ne vaut qu'ENTRE tours.
fn hard_cap() -> Duration {
    let secs = std::env::var("ATELIER_AGENT_HARD_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(14400u64); // 4 h
    Duration::from_secs(secs)
}

/// Construit la commande `sudo -u hr-studio node runner.js` partagée par le mode
/// session ([`run_agent`]) et le mode introspection ([`run_runner_op`]) : même user,
/// même whitelist d'env (seul `CLAUDE_CONFIG_DIR`, non secret), même cwd.
fn runner_command(cwd: &FsPath) -> Command {
    let mut cmd = Command::new("sudo");
    cmd.arg("-n")
        .arg("-H")
        .arg("-u")
        .arg(run_as_user())
        // Whitelist : seul CLAUDE_CONFIG_DIR (non secret) traverse l'env_reset de sudo.
        .arg("--preserve-env=CLAUDE_CONFIG_DIR")
        .arg("--")
        .arg(node_bin())
        .arg(runner_script());
    cmd.current_dir(cwd);
    cmd.env("CLAUDE_CONFIG_DIR", claude_config_dir());
    cmd
}

/// Lance le runner en mode introspection one-shot (op:list/messages/rename/delete) :
/// écrit l'init sur stdin, ferme stdin (EOF), lit le PREMIER objet NDJSON émis, reape
/// le process. Pas d'EventBus — la réponse part directe en HTTP. Timeout court.
async fn run_runner_op(cwd: &FsPath, init_json: String) -> Result<Value, String> {
    let mut cmd = runner_command(cwd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("spawn runner: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(init_json.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        let _ = stdin.flush().await;
        let _ = stdin.shutdown().await; // EOF : le runner introspection n'attend rien d'autre
    }
    let out = match child.stdout.take() {
        Some(o) => o,
        None => return Err("runner: pas de stdout".into()),
    };
    let mut lines = BufReader::new(out).lines();
    let read = tokio::time::timeout(Duration::from_secs(30), async {
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                return Some(v);
            }
        }
        None
    })
    .await;
    let _ = child.start_kill();
    let _ = child.wait().await;
    match read {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err("runner: aucune sortie".into()),
        Err(_) => Err("runner: timeout".into()),
    }
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
    // cwd = la source de l'app (le working tree édité par l'agent). Doit exister.
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

    // Seed du buffer transcript live. WHY : le runner ne ré-émet PAS les tours
    // utilisateur → on les ajoute nous-mêmes (ici le 1er), et servir le snapshot depuis
    // la mémoire évite de spawn un runner par requête. En reprise, on précharge le
    // transcript déjà persisté sur disque pour ne rien perdre au reload.
    let mut items: Vec<Value> = Vec::new();
    if let Some(sid) = &body.resume {
        let m = json!({ "op": "messages", "sessionId": sid, "cwd": cwd.to_string_lossy() }).to_string();
        if let Ok(v) = run_runner_op(&cwd, m).await {
            if let Some(arr) = v.get("items").and_then(|x| x.as_array()) {
                items = arr.clone();
            }
        }
    }
    items.push(user_item(&body.prompt));

    // Mode initial côté UI ('plan' | 'bypass'), dérivé du permissionMode SDK demandé.
    let permission_mode = body.permission_mode.clone().unwrap_or_else(|| "plan".into());
    let ui_mode = if permission_mode == "bypassPermissions" { "bypass" } else { "plan" };

    // L'init est consommé par runner.js (clés camelCase). Le token MCP passe ICI,
    // par stdin (pipe) — que ni Atelier ni sudo ne journalisent. (Le passer en
    // env via sudo --preserve-env le ferait apparaître en clair dans journald.)
    let init = json!({
        "prompt": body.prompt,
        "effort": body.effort, // None → null → runner omet (Haiku ne supporte pas effort)
        "permissionMode": permission_mode,
        "allowedTools": allowed_tools,
        "cwd": cwd.to_string_lossy(),
        "mcpEndpoint": format!("{}?project={}", mcp_endpoint_base(), slug),
        "mcpToken": std::env::var("MCP_TOKEN").ok(),
        "resume": body.resume,
        "model": body.model,
    });

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let (input_tx, input_rx) = mpsc::unbounded_channel::<String>();
    RUNS.lock().unwrap().insert(
        run_id.clone(),
        RunState {
            slug: slug.clone(),
            session_id: None,
            cancel_tx: Some(cancel_tx),
            input_tx,
            items,
            mode: ui_mode.to_string(),
            turn_active: true, // le prompt d'init est le tour #1
        },
    );

    info!(run_id = %run_id, "agent run started");
    let events = state.events.clone();
    let run_id_task = run_id.clone();
    let slug_task = slug.clone();
    tokio::spawn(async move {
        run_agent(events, slug_task, run_id_task, cwd, init.to_string(), cancel_rx, input_rx).await;
        // run_agent nettoie RUNS / SID_RUN en fin de run (graceful → session persistée).
    });

    (StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))).into_response()
}

#[instrument(skip(state), fields(slug = %slug, run_id = %run_id))]
async fn cancel(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = &state; // état non requis ; signature homogène avec les autres handlers
    // Demande l'arrêt PROPRE (interrupt + EOF côté run_agent → le SDK persiste la
    // session) sans retirer l'entrée : run_agent la nettoie une fois le flush terminé.
    let sent = RUNS
        .lock()
        .unwrap()
        .get_mut(&run_id)
        .and_then(|r| r.cancel_tx.take())
        .map(|tx| tx.send(()).is_ok())
        .unwrap_or(false);
    if sent {
        (StatusCode::OK, Json(json!({"cancelled": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou déjà terminé")
    }
}

/// `POST /runs/{run_id}/interrupt` — interrompt le TOUR courant (bouton Stop) sans fermer
/// la session : le runner appelle `query.interrupt()` (abort du tour ; la session survit
/// et accepte le tour suivant). À distinguer de `cancel` (EOF → fin de session).
#[instrument(skip(state), fields(slug = %slug, run_id = %run_id))]
async fn interrupt(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = (&state, &slug);
    let line = json!({ "type": "interrupt" }).to_string();
    if send_input(&run_id, line) {
        info!(run_id = %run_id, "agent turn interrupt requested");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou terminé")
    }
}

/// Tour utilisateur suivant dans une session existante (mémoire conservée).
#[derive(Deserialize)]
struct MessageBody {
    text: String,
}

#[instrument(skip(state, body), fields(slug = %slug, run_id = %run_id))]
async fn message(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
    Json(body): Json<MessageBody>,
) -> impl IntoResponse {
    let _ = &state; // signature homogène ; état non requis
    if body.text.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "message vide");
    }
    let line = json!({ "type": "user_message", "text": body.text }).to_string();
    if send_input(&run_id, line) {
        // Le runner ne ré-émet pas le tour user → on l'ajoute au buffer (reload).
        if let Some(r) = RUNS.lock().unwrap().get_mut(&run_id) {
            r.items.push(user_item(&body.text));
            r.turn_active = true; // nouveau tour soumis
        }
        info!(run_id = %run_id, "agent message sent");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "session inconnue ou terminée")
    }
}

/// Réponse à une question interactive (AskUserQuestion). Sérialise une ligne
/// `{type:"answer",...}` que le runner convertit en TOUR utilisateur suivant.
/// `answers` = { texte_question -> réponse } (multi-select joint par virgule).
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
    if send_input(&run_id, line) {
        // Marque la question comme répondue + stocke le texte de la réponse dans le buffer
        // (pour un reload pendant que la session vit : la réponse vraie est livrée hors-bande).
        let answer_text = if body.cancelled {
            "(non répondu)".to_string()
        } else {
            let mut parts: Vec<String> =
                body.answers.iter().map(|(q, a)| format!("- {q} → {a}")).collect();
            if let Some(resp) = body.response.as_ref().filter(|s| !s.trim().is_empty()) {
                parts.push(resp.trim().to_string());
            }
            parts.join("\n")
        };
        if let Some(r) = RUNS.lock().unwrap().get_mut(&run_id) {
            r.turn_active = true; // la réponse relance/poursuit le tour suspendu
            for it in r.items.iter_mut().rev() {
                if it.get("type").and_then(|x| x.as_str()) == Some("question")
                    && it.get("request_id").and_then(|x| x.as_str()) == Some(body.request_id.as_str())
                {
                    it["answered"] = json!(true);
                    it["answer"] = json!(answer_text);
                    break;
                }
            }
        }
        info!(run_id = %run_id, "agent question answered");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou terminé")
    }
}

/// Décision sur un plan proposé (ExitPlanMode). Sérialise `{type:"plan_decision",...}` que
/// le runner relaie à `canUseTool` : `approved=true` → le SDK enchaîne sur l'implémentation
/// (session basculée en édition) ; sinon le modèle ré-affine le plan en lecture seule.
#[derive(Deserialize)]
struct PlanDecisionBody {
    request_id: String,
    #[serde(default)]
    approved: bool,
    #[serde(default)]
    feedback: Option<String>,
}

#[instrument(skip(state, body), fields(slug = %slug, run_id = %run_id, approved = body.approved))]
async fn plan_decision(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
    Json(body): Json<PlanDecisionBody>,
) -> impl IntoResponse {
    let _ = (&state, &slug);
    let line = json!({
        "type": "plan_decision",
        "request_id": body.request_id,
        "approved": body.approved,
        "feedback": body.feedback,
    })
    .to_string();
    if send_input(&run_id, line) {
        // Marque le plan_review décidé dans le buffer (reload).
        if let Some(r) = RUNS.lock().unwrap().get_mut(&run_id) {
            r.turn_active = true; // approbation/renvoi poursuit le tour suspendu
            for it in r.items.iter_mut().rev() {
                if it.get("type").and_then(|x| x.as_str()) == Some("plan_review")
                    && it.get("request_id").and_then(|x| x.as_str()) == Some(body.request_id.as_str())
                {
                    it["decided"] = json!(true);
                    it["approved"] = json!(body.approved);
                    break;
                }
            }
        }
        info!(run_id = %run_id, "agent plan decision sent");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou terminé")
    }
}

/// Change le mode EN COURS de session (setPermissionMode côté SDK) : 'plan' (lecture seule)
/// ↔ 'bypass' (édition). Évite d'avoir à couper/relancer pour passer en implémentation.
#[derive(Deserialize)]
struct SetModeBody {
    mode: String,
}

#[instrument(skip(state, body), fields(slug = %slug, run_id = %run_id, mode = %body.mode))]
async fn set_mode(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
    Json(body): Json<SetModeBody>,
) -> impl IntoResponse {
    let _ = (&state, &slug);
    if body.mode != "plan" && body.mode != "bypass" {
        return err(StatusCode::BAD_REQUEST, "mode invalide (plan|bypass)");
    }
    let line = json!({ "type": "set_mode", "mode": body.mode }).to_string();
    if send_input(&run_id, line) {
        if let Some(r) = RUNS.lock().unwrap().get_mut(&run_id) {
            r.mode = body.mode.clone();
        }
        info!(run_id = %run_id, "agent mode changed");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou terminé")
    }
}

/// Change le modèle EN COURS de session (setModel côté SDK). `model` null → défaut abonnement.
#[derive(Deserialize)]
struct SetModelBody {
    #[serde(default)]
    model: Option<String>,
}

#[instrument(skip(state, body), fields(slug = %slug, run_id = %run_id))]
async fn set_model(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
    Json(body): Json<SetModelBody>,
) -> impl IntoResponse {
    let _ = (&state, &slug);
    let line = json!({ "type": "set_model", "model": body.model }).to_string();
    if send_input(&run_id, line) {
        info!(run_id = %run_id, "agent model changed");
        (StatusCode::OK, Json(json!({"ok": true}))).into_response()
    } else {
        err(StatusCode::NOT_FOUND, "run inconnu ou terminé")
    }
}

// --- Conversations (sessions SDK persistées sur disque, exposées via le runner) ---

fn runner_bad_gateway(v: &Value, fallback: &str) -> axum::response::Response {
    err(
        StatusCode::BAD_GATEWAY,
        v.get("message").and_then(|x| x.as_str()).unwrap_or(fallback).to_string(),
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Dernier dialogue interactif NON résolu du buffer (`question` sans `answered`,
/// `plan_review` sans `decided`) → `{kind, request_id}`, sinon `null`. Exposé dans le
/// snapshot pour que l'UI restaure l'état « en attente de ta réponse » après un refresh.
fn pending_dialog(items: &[Value]) -> Value {
    for it in items.iter().rev() {
        match it.get("type").and_then(|x| x.as_str()) {
            Some("question") if !it.get("answered").and_then(|x| x.as_bool()).unwrap_or(false) => {
                return json!({ "kind": "question", "request_id": it.get("request_id").cloned().unwrap_or(Value::Null) });
            }
            Some("plan_review") if !it.get("decided").and_then(|x| x.as_bool()).unwrap_or(false) => {
                return json!({ "kind": "plan_review", "request_id": it.get("request_id").cloned().unwrap_or(Value::Null) });
            }
            _ => {}
        }
    }
    Value::Null
}

/// Sessions vivantes de cet app : `(session_id, run_id, résumé)`. Sert à annoter la
/// liste `live` ET à y injecter les sessions pas encore flushées sur disque.
fn live_sessions_for(slug: &str) -> Vec<(String, String, String)> {
    let sid_runs: Vec<(String, String)> =
        SID_RUN.lock().unwrap().iter().map(|(s, r)| (s.clone(), r.clone())).collect();
    let runs = RUNS.lock().unwrap();
    sid_runs
        .into_iter()
        .filter_map(|(sid, rid)| {
            let r = runs.get(&rid)?;
            if r.slug != slug {
                return None;
            }
            let summary = r
                .items
                .iter()
                .find(|i| i.get("type").and_then(|x| x.as_str()) == Some("user"))
                .and_then(|i| i.get("text").and_then(|x| x.as_str()))
                .unwrap_or("")
                .chars()
                .take(80)
                .collect::<String>();
            Some((sid, rid, summary))
        })
        .collect()
}

/// `GET /api/apps/{slug}/agent/conversations` — liste les sessions SDK de l'app
/// (runner `op:list`, sur disque), annotées `live`/`run_id`, plus les sessions vivantes
/// pas encore persistées (injectées en tête).
#[instrument(skip(state), fields(slug = %slug))]
async fn list_conversations(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let cwd: PathBuf = state.apps_src_root.join(&slug).join("src");
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    let init = json!({ "op": "list", "cwd": cwd.to_string_lossy() }).to_string();
    match run_runner_op(&cwd, init).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("sessions") => {
            let live = live_sessions_for(&slug);
            let mut on_disk: Vec<String> = Vec::new();
            let mut conversations: Vec<Value> = v
                .get("sessions")
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|mut s| {
                    let sid = s.get("sessionId").and_then(|x| x.as_str()).map(String::from);
                    if let Some(sid) = sid {
                        on_disk.push(sid.clone());
                        match live.iter().find(|(lsid, _, _)| *lsid == sid) {
                            Some((_, rid, _)) => {
                                s["live"] = json!(true);
                                s["run_id"] = json!(rid);
                            }
                            None => s["live"] = json!(false),
                        }
                    }
                    s
                })
                .collect();
            for (sid, rid, summary) in live {
                if !on_disk.contains(&sid) {
                    conversations.insert(0, json!({
                        "sessionId": sid,
                        "live": true,
                        "run_id": rid,
                        "summary": summary,
                        "lastModified": now_ms(),
                    }));
                }
            }
            Json(json!({ "conversations": conversations })).into_response()
        }
        Ok(v) => runner_bad_gateway(&v, "runner: réponse inattendue"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// `GET /api/apps/{slug}/agent/conversations/{sid}` — snapshot : transcript (runner
/// `op:messages`) + `live`/`run_id` pour que le frontend se rebranche au WS.
#[instrument(skip(state), fields(slug = %slug, sid = %sid))]
async fn get_conversation(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
) -> impl IntoResponse {
    let cwd: PathBuf = state.apps_src_root.join(&slug).join("src");
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    // Session vivante → fil servi depuis le buffer mémoire (pas encore sur disque).
    let rid = SID_RUN.lock().unwrap().get(&sid).cloned();
    if let Some(rid) = rid {
        let snap = RUNS.lock().unwrap().get(&rid).map(|r| (r.items.clone(), r.mode.clone(), r.turn_active));
        if let Some((items, mode, turn_active)) = snap {
            // `running` (tour en vol) + `pending` (dialogue non résolu) permettent au
            // frontend de restaurer l'indicateur de réflexion / la carte d'attente après
            // un refresh (le WS broadcast ne rejoue pas `started`/`question`).
            let pending = pending_dialog(&items);
            return Json(json!({ "items": items, "live": true, "run_id": rid, "mode": mode, "running": turn_active, "pending": pending })).into_response();
        }
    }
    // Sinon → transcript persisté sur disque.
    let init = json!({ "op": "messages", "sessionId": sid, "cwd": cwd.to_string_lossy() }).to_string();
    match run_runner_op(&cwd, init).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("transcript") => {
            Json(json!({
                "items": v.get("items").cloned().unwrap_or_else(|| json!([])),
                "live": false,
                "run_id": Value::Null,
            }))
            .into_response()
        }
        Ok(v) => runner_bad_gateway(&v, "runner: réponse inattendue"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

/// `PATCH /api/apps/{slug}/agent/conversations/{sid}` — renomme la session (titre SDK).
#[instrument(skip(state, body), fields(slug = %slug, sid = %sid))]
async fn rename_conversation(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
    Json(body): Json<RenameBody>,
) -> impl IntoResponse {
    let cwd: PathBuf = state.apps_src_root.join(&slug).join("src");
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    let init = json!({ "op": "rename", "sessionId": sid, "title": body.title, "cwd": cwd.to_string_lossy() }).to_string();
    match run_runner_op(&cwd, init).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("ok") => {
            (StatusCode::OK, Json(json!({"ok": true}))).into_response()
        }
        Ok(v) => runner_bad_gateway(&v, "runner: échec rename"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// `DELETE /api/apps/{slug}/agent/conversations/{sid}` — coupe le run vivant éventuel
/// puis supprime la session du disque.
#[instrument(skip(state), fields(slug = %slug, sid = %sid))]
async fn delete_conversation(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
) -> impl IntoResponse {
    let cwd: PathBuf = state.apps_src_root.join(&slug).join("src");
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    // Conversation vivante sur cette session → on la coupe avant de supprimer le fichier.
    let rid = SID_RUN.lock().unwrap().get(&sid).cloned();
    if let Some(rid) = rid {
        if let Some(tx) = RUNS.lock().unwrap().get_mut(&rid).and_then(|r| r.cancel_tx.take()) {
            let _ = tx.send(());
        }
    }
    let init = json!({ "op": "delete", "sessionId": sid, "cwd": cwd.to_string_lossy() }).to_string();
    match run_runner_op(&cwd, init).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("ok") => {
            (StatusCode::OK, Json(json!({"deleted": true}))).into_response()
        }
        Ok(v) => runner_bad_gateway(&v, "runner: échec delete"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// Publie un `AgentEvent` taggé sur l'EventBus. `seq` ordonne ; le frontend route par
/// `session_id` (stable, présent dès le `system`) avec repli sur `run_id` pour la
/// fenêtre initiale (avant que le SDK n'ait annoncé la session).
fn publish(
    events: &atelier_common::events::EventBus,
    run_id: &str,
    session_id: Option<&str>,
    slug: &str,
    seq: &mut u64,
    kind: &str,
    data: Value,
) {
    // Tient le buffer transcript live à jour (sert le snapshot d'une session vivante).
    fold_into_run(run_id, kind, &data);
    let ev = AgentEvent {
        run_id: run_id.to_string(),
        session_id: session_id.map(String::from),
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
    mut input_rx: mpsc::UnboundedReceiver<String>,
) {
    let mut seq: u64 = 0;
    // Clé stable de la conversation : inconnue jusqu'à la 1re ligne `system` du runner.
    let mut session_id: Option<String> = None;
    publish(&events, &run_id, None, &slug, &mut seq, "started", json!({}));

    // Le MCP_TOKEN passe par l'init JSON (stdin), JAMAIS par l'env — sinon sudo le
    // journalise en clair dans son ENV=. ANTHROPIC_API_KEY et les DSN root sont
    // écartés par env_reset (hors whitelist `--preserve-env=CLAUDE_CONFIG_DIR`).
    let mut cmd = runner_command(&cwd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!(?e, "spawn runner failed");
            publish(&events, &run_id, None, &slug, &mut seq, "error", json!({"message": format!("spawn runner: {e}")}));
            publish(&events, &run_id, None, &slug, &mut seq, "done", json!({"exit_ok": false}));
            return;
        }
    };
    let child_pid = child.id();

    // stdin = canal d'entrée de la session. On le GARDE dans run_agent (pas de tâche
    // séparée) : l'init (1 ligne) puis les tours/réponses (`/message`/`/answer`) via
    // `input_rx` y sont écrits dans la boucle ci-dessous, et surtout on peut le FERMER
    // (shutdown = EOF) à l'arrêt → le runner termine proprement sa session et le SDK
    // flush le transcript sur disque (prérequis du resume/history).
    let mut stdin = child.stdin.take();
    if let Some(s) = stdin.as_mut() {
        if let Err(e) = s.write_all(init_json.as_bytes()).await {
            warn!(?e, "write init to runner stdin failed");
        }
        let _ = s.write_all(b"\n").await;
        let _ = s.flush().await;
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
                 session_id: Option<&str>,
                 slug: &str,
                 seq: &mut u64,
                 pending_text: &mut String,
                 pending_kind: &mut Option<String>| {
        if let Some(kind) = pending_kind.take() {
            if !pending_text.is_empty() {
                publish(events, run_id, session_id, slug, seq, &kind, json!({"text": pending_text.clone()}));
            }
            pending_text.clear();
        }
    };

    let mut cancelled = false;
    let mut timed_out = false;
    // Tour #1 (prompt d'init) actif au démarrage. Tant qu'un tour est en vol on applique le
    // plafond dur (`hard`) ; un sous-agent silencieux ne ré-arme pas l'idle, donc l'idle court
    // ne vaut qu'ENTRE tours (reape une session oubliée sans tuer un tour long légitime).
    let mut turn_active_local = true;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        let idle = idle_timeout();
        let hard = hard_cap();
        let deadline = tokio::time::sleep(hard);
        tokio::pin!(deadline);
        // `ending` : arrêt PROPRE demandé (cancel/idle) → on ferme stdin (EOF). Le runner
        // achève le tour courant puis termine la session ; le SDK flush sur disque. On lit
        // jusqu'à l'EOF stdout (grâce ré-armée sur activité : un tour qui stream n'est pas tué).
        let mut ending = false;
        let mut input_open = true;
        let grace = Duration::from_secs(20);
        loop {
            tokio::select! {
                biased;
                _ = &mut cancel, if !ending => {
                    cancelled = true;
                    ending = true;
                    // Arrêt PROPRE : on AVORTE d'abord le tour en vol (frontière propre → pas
                    // de tool_use pendouillant → session RESUMABLE), PUIS EOF stdin pour
                    // terminer la session (le SDK flush le transcript). Un EOF nu laisserait le
                    // tour courant finir (potentiellement long) → dépassement du budget de drain
                    // → SIGKILL → troncature, exactement ce qu'on cherche à éviter.
                    if let Some(s) = stdin.as_mut() {
                        if turn_active_local {
                            let _ = s.write_all(b"{\"type\":\"interrupt\"}\n").await;
                            let _ = s.flush().await;
                        }
                        let _ = s.shutdown().await;
                    }
                    stdin = None;
                    deadline.as_mut().reset(tokio::time::Instant::now() + grace);
                }
                _ = &mut deadline => {
                    if ending { break; } // silence prolongé après l'arrêt → on force (kill plus bas)
                    timed_out = true;
                    ending = true;
                    if let Some(s) = stdin.as_mut() { let _ = s.shutdown().await; }
                    stdin = None;
                    deadline.as_mut().reset(tokio::time::Instant::now() + grace);
                }
                maybe = input_rx.recv(), if input_open && !ending => {
                    match maybe {
                        Some(line) => {
                            // Un message/réponse/décision relance un tour → plafond dur + état actif.
                            // Les contrôles (interrupt/set_mode/set_model) ne changent pas l'état.
                            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                                if matches!(v.get("type").and_then(|x| x.as_str()),
                                    Some("user_message") | Some("answer") | Some("plan_decision"))
                                {
                                    turn_active_local = true;
                                    deadline.as_mut().reset(tokio::time::Instant::now() + hard);
                                }
                            }
                            if let Some(s) = stdin.as_mut() {
                                let _ = s.write_all(line.as_bytes()).await;
                                let _ = s.write_all(b"\n").await;
                                let _ = s.flush().await;
                            }
                        }
                        None => input_open = false, // tous les senders droppés (cleanup)
                    }
                }
                next = lines.next_line() => match next {
                    Ok(Some(line)) => {
                        // Activité runner → ré-arme le timeout : grâce courte pendant l'arrêt,
                        // sinon plafond dur si un tour est en vol, idle court entre tours.
                        deadline.as_mut().reset(tokio::time::Instant::now() + if ending { grace } else if turn_active_local { hard } else { idle });
                        let trimmed = line.trim();
                        if trimmed.is_empty() { continue; }
                        let obj: Value = match serde_json::from_str(trimmed) {
                            Ok(v) => v,
                            Err(_) => { continue; } // ligne non-JSON ignorée (robustesse)
                        };
                        let t = obj.get("t").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        // Fin de tour → bascule sur l'idle court (la session est entre tours).
                        if t == "turn_done" {
                            turn_active_local = false;
                            if !ending { deadline.as_mut().reset(tokio::time::Instant::now() + idle); }
                        }
                        // 1re ligne `system` → on lie session_id ↔ run_id (conversation
                        // vivante). Tous les events suivants (et le `system` lui-même)
                        // portent alors session_id, clé de routage stable du frontend.
                        if t == "system" && session_id.is_none() {
                            if let Some(sid) = obj.get("session_id").and_then(|x| x.as_str()) {
                                session_id = Some(sid.to_string());
                                SID_RUN.lock().unwrap().insert(sid.to_string(), run_id.clone());
                                if let Some(r) = RUNS.lock().unwrap().get_mut(&run_id) {
                                    r.session_id = Some(sid.to_string());
                                }
                                info!(run_id = %run_id, session_id = %sid, "agent session bound");
                            }
                        }
                        if t == "assistant_delta" || t == "thinking_delta" {
                            let text = obj.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if pending_kind.as_deref() != Some(t.as_str()) {
                                flush(&events, &run_id, session_id.as_deref(), &slug, &mut seq, &mut pending_text, &mut pending_kind);
                                pending_kind = Some(t.clone());
                            }
                            pending_text.push_str(text);
                            if pending_text.len() >= 200 {
                                flush(&events, &run_id, session_id.as_deref(), &slug, &mut seq, &mut pending_text, &mut pending_kind);
                            }
                        } else {
                            flush(&events, &run_id, session_id.as_deref(), &slug, &mut seq, &mut pending_text, &mut pending_kind);
                            publish(&events, &run_id, session_id.as_deref(), &slug, &mut seq, &t, obj);
                        }
                    }
                    Ok(None) => break, // EOF
                    Err(e) => { warn!(?e, "runner stdout read error"); break; }
                },
            }
        }
    }
    flush(&events, &run_id, session_id.as_deref(), &slug, &mut seq, &mut pending_text, &mut pending_kind);

    // Reap : après un arrêt propre le runner sort de lui-même (flush terminé) → wait
    // court. S'il traîne (flush bloqué / pas d'EOF), on force le groupe (SIGKILL pour
    // reaper le binaire `claude` petit-fils du SDK).
    let status = match tokio::time::timeout(Duration::from_secs(8), child.wait()).await {
        Ok(s) => s.ok(),
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = child_pid {
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            }
            let _ = child.start_kill();
            child.wait().await.ok()
        }
    };
    let stderr = match stderr_task {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    let exit_ok = status.map(|s| s.success()).unwrap_or(false);
    // Un exit non-clean HORS arrêt demandé est un vrai échec : on remonte la queue de
    // stderr du runner (diagnostics, jamais de secret) pour le diagnostic.
    if !exit_ok && !cancelled {
        let tail: String = stderr.chars().rev().take(800).collect::<String>().chars().rev().collect();
        warn!(run_id = %run_id, timed_out, stderr_tail = %tail, "agent runner exited non-clean");
    }
    info!(run_id = %run_id, exit_ok, cancelled, timed_out, "agent run done");
    // Conversation plus vivante : on la retire des registres. La session est sur disque
    // (persistée par le SDK de façon incrémentale) → snapshot/list la reliront depuis là.
    if let Some(sid) = &session_id {
        SID_RUN.lock().unwrap().remove(sid);
    }
    RUNS.lock().unwrap().remove(&run_id);
    publish(
        &events,
        &run_id,
        session_id.as_deref(),
        &slug,
        &mut seq,
        "done",
        json!({"exit_ok": exit_ok, "cancelled": cancelled, "timed_out": timed_out}),
    );
}

/// Drain de tous les runs vivants à l'arrêt d'Atelier. Envoie le signal d'arrêt PROPRE à
/// chaque `run_agent` (interrupt du tour + EOF stdin → le SDK flush un transcript
/// RESUMABLE), puis attend que `RUNS` se vide ou jusqu'à `deadline`. Appelé par le handler
/// SIGTERM du binaire (`main.rs`) AVANT l'exit : un `make deploy` ne tronque plus un tour
/// en vol (sinon la session devient non-relançable, cf. cause racine du symptôme #1).
#[instrument(skip_all)]
pub async fn drain_agent_runs(deadline: Duration) {
    // Prendre les cancel_tx HORS du lock (on ne tient pas un std Mutex à travers un await).
    let cancels: Vec<oneshot::Sender<()>> = {
        let mut runs = RUNS.lock().unwrap();
        runs.values_mut().filter_map(|r| r.cancel_tx.take()).collect()
    };
    let n = cancels.len();
    if n == 0 {
        return;
    }
    info!(runs = n, "draining live agent runs before shutdown");
    for tx in cancels {
        let _ = tx.send(()); // déclenche interrupt+EOF dans chaque run_agent
    }
    // Chaque run_agent retire son entrée de RUNS en fin de flush → RUNS vide = drain terminé.
    let _ = tokio::time::timeout(deadline, async {
        loop {
            if RUNS.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let remaining = RUNS.lock().unwrap().len();
    info!(remaining, "agent drain complete");
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
    let pkg = runner_dir()?.join("node_modules/@anthropic-ai/claude-agent-sdk/package.json");
    let s = std::fs::read_to_string(pkg).ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

/// Le paquet SDK et sa dep native optionnelle (linux-x64), relatifs à `node_modules`.
const SDK_PKG: &str = "@anthropic-ai/claude-agent-sdk";
const SDK_NATIVE: &str = "@anthropic-ai/claude-agent-sdk-linux-x64";

/// Tronque un log à ses `n` derniers caractères (sûr UTF-8), pour le renvoyer en cas d'échec.
fn tail(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
}

/// Purge un éventuel reliquat de backup `*.sdk-bak` (run précédent interrompu, ou nettoyage post-succès).
fn cleanup_sdk_bak(dir: &FsPath) {
    let nm = dir.join("node_modules");
    let _ = std::fs::remove_dir_all(nm.join(format!("{SDK_PKG}.sdk-bak")));
    let _ = std::fs::remove_dir_all(nm.join(format!("{SDK_NATIVE}.sdk-bak")));
    let _ = std::fs::remove_file(dir.join("package.json.sdk-bak"));
    let _ = std::fs::remove_file(dir.join("package-lock.json.sdk-bak"));
}

/// Snapshot des artefacts SDK avant install, pour rollback : manifests copiés, dossiers SDK
/// `rename`és de côté (atomique, même FS → npm les réinstalle frais). Best-effort sur l'absent.
fn snapshot_sdk(dir: &FsPath) -> std::io::Result<()> {
    cleanup_sdk_bak(dir);
    let nm = dir.join("node_modules");
    std::fs::copy(dir.join("package.json"), dir.join("package.json.sdk-bak"))?;
    let lock = dir.join("package-lock.json");
    if lock.exists() {
        std::fs::copy(&lock, dir.join("package-lock.json.sdk-bak"))?;
    }
    std::fs::rename(nm.join(SDK_PKG), nm.join(format!("{SDK_PKG}.sdk-bak")))?;
    let native = nm.join(SDK_NATIVE);
    if native.exists() {
        std::fs::rename(&native, nm.join(format!("{SDK_NATIVE}.sdk-bak")))?;
    }
    Ok(())
}

/// Rollback : dégage les dossiers fraîchement (mal) installés et remet le snapshot + les manifests.
/// Best-effort — on ne peut rien faire de plus utile que de loguer si une étape échoue.
fn restore_sdk(dir: &FsPath) {
    let nm = dir.join("node_modules");
    let _ = std::fs::remove_dir_all(nm.join(SDK_PKG));
    let _ = std::fs::rename(nm.join(format!("{SDK_PKG}.sdk-bak")), nm.join(SDK_PKG));
    let _ = std::fs::remove_dir_all(nm.join(SDK_NATIVE));
    let _ = std::fs::rename(nm.join(format!("{SDK_NATIVE}.sdk-bak")), nm.join(SDK_NATIVE));
    let pkg_bak = dir.join("package.json.sdk-bak");
    if pkg_bak.exists() {
        let _ = std::fs::rename(&pkg_bak, dir.join("package.json"));
    }
    let lock_bak = dir.join("package-lock.json.sdk-bak");
    if lock_bak.exists() {
        let _ = std::fs::rename(&lock_bak, dir.join("package-lock.json"));
    }
}

/// `npm install <spec>` dans le runner. Retourne le log combiné (Ok) ou un message d'échec (Err).
/// WHY env : sous `ProtectSystem=strict` `/root` est read-only → on force HOME/cache npm vers `/tmp`
/// (writable). `--omit=dev` (jamais `--omit=optional` : la dep native linux-x64 est requise au runtime).
async fn run_npm_install(dir: &FsPath, spec: &str) -> Result<String, String> {
    let mut cmd = Command::new(npm_bin());
    cmd.arg("install")
        .arg(spec)
        .arg("--no-audit")
        .arg("--no-fund")
        .arg("--save-exact")
        .arg("--omit=dev")
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("HOME", "/tmp")
        .env("npm_config_cache", "/tmp/.npm-atelier")
        .env("CI", "true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = tokio::time::timeout(npm_timeout(), cmd.output())
        .await
        .map_err(|_| format!("npm install: timeout ({}s)", npm_timeout().as_secs()))?
        .map_err(|e| format!("npm install: spawn impossible: {e}"))?;
    let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
    log.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(log)
    } else {
        Err(format!("npm install: {}\n{log}", out.status))
    }
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

#[derive(Debug, Deserialize)]
struct SdkUpdateBody {
    #[serde(default)]
    version: Option<String>,
}

/// MAJ in-place (ÉPHÉMÈRE) du Claude Agent SDK dans le runner installé : snapshot → `npm install`
/// → vérif (version cible + dep native) → smoke-test (`op:list`, charge le SDK sous l'exec réel
/// hr-studio) → rollback si échec. L'effet porte sur la PROCHAINE session agent (runner spawné
/// frais) — pas de restart. Annulé au prochain `make deploy` (resync depuis le pin source) ;
/// `sdk_version` lisant la version live sur disque, l'UI reste cohérente.
#[instrument(skip(body))]
async fn sdk_update(body: Option<Json<SdkUpdateBody>>) -> axum::response::Response {
    if SDK_UPDATING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return err(StatusCode::CONFLICT, "MAJ SDK déjà en cours");
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            SDK_UPDATING.store(false, Ordering::Release);
        }
    }
    let _guard = Guard;

    let dir = match runner_dir() {
        Some(d) if d.join("node_modules").is_dir() => d,
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "runner introuvable"),
    };

    // Cible : version explicite (body) sinon la dernière publiée au registry npm.
    let target = match body.and_then(|b| b.0.version).filter(|v| !v.trim().is_empty()) {
        Some(v) => v.trim().to_string(),
        None => match fetch_latest_sdk().await {
            Some(v) => v,
            None => return err(StatusCode::BAD_GATEWAY, "registry npm injoignable"),
        },
    };

    let installed = installed_sdk_version();
    if installed.as_deref() == Some(target.as_str()) {
        info!(version = %target, "MAJ SDK : déjà à jour");
        return (
            StatusCode::OK,
            Json(json!({ "installed": installed, "latest": target, "updated": false, "note": "déjà à jour" })),
        )
            .into_response();
    }

    info!(target = %target, dir = %dir.display(), from = ?installed, "MAJ SDK : début");

    if let Err(e) = snapshot_sdk(&dir) {
        error!(error = %e, "MAJ SDK : snapshot impossible");
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("snapshot impossible: {e}"));
    }

    let spec = format!("{SDK_PKG}@{target}");
    let outcome: Result<(), String> = match run_npm_install(&dir, &spec).await {
        Err(log) => Err(log),
        Ok(_) => {
            let now = installed_sdk_version();
            if now.as_deref() != Some(target.as_str()) {
                Err(format!("version post-install inattendue: {now:?} (attendu {target})"))
            } else if !dir.join("node_modules").join(SDK_NATIVE).is_dir() {
                Err(format!("dep native {SDK_NATIVE} absente après install"))
            } else {
                // Smoke-test : op:list importe le SDK et tourne sous l'exec réel hr-studio.
                let init = json!({ "op": "list", "cwd": dir.to_string_lossy() }).to_string();
                match run_runner_op(&dir, init).await {
                    Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("sessions") => Ok(()),
                    Ok(v) => Err(format!("smoke-test op:list inattendu: {v}")),
                    Err(e) => Err(format!("smoke-test op:list échoué: {e}")),
                }
            }
        }
    };

    match outcome {
        Ok(()) => {
            cleanup_sdk_bak(&dir);
            info!(version = %target, "MAJ SDK : succès");
            (
                StatusCode::OK,
                Json(json!({ "installed": target, "latest": target, "updated": true })),
            )
                .into_response()
        }
        Err(log) => {
            restore_sdk(&dir);
            let restored = installed_sdk_version();
            warn!(error = %tail(&log, 500), restored = ?restored, "MAJ SDK : rollback");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "MAJ SDK échouée (rollback effectué)",
                    "installed": restored,
                    "log": tail(&log, 4000),
                })),
            )
                .into_response()
        }
    }
}
