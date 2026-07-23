//! Agent SDK chat — pilote le runner Node (`/opt/atelier/runner`) via le même
//! pattern de sous-process que la surveillance ([`atelier_watcher::claude`]) :
//! on spawn le runner, on lui écrit un JSON d'init sur stdin, on lit son NDJSON
//! ligne à ligne et on republie chaque ligne (normalisée + taggée `run_id`) sur
//! l'EventBus → WebSocket. Le runner tourne en `hr-studio` (OAuth abonnement),
//! jamais en root, et les secrets passent par l'env (jamais par l'argv).
//!
//! DEUX moteurs derrière ce même protocole ([`Engine`]) : `claude` (Claude Agent
//! SDK, `runner/src/runner.js`) et `codex` (Codex SDK d'OpenAI,
//! `runner/src/codex.js`). Seuls diffèrent le script lancé, la variable de chemin
//! whitelistée par sudo, l'espace de sessions persistées et la source d'auth
//! (token OAuth injecté par stdin côté Claude ; fichier `$CODEX_HOME/auth.json`
//! écrit/rotaté par le CLI côté Codex). Le moteur d'une conversation est FIGÉ à
//! son binding de session : les deux stores de transcripts sont disjoints, un
//! thread Codex n'est pas reprenable par Claude et réciproquement.
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

// parking_lot plutôt que std::sync::Mutex : pas d'empoisonnement. Avec std, un
// panic survenu sous guard empoisonnait RUNS/SID_RUN et TOUS les endpoints
// agent paniquaient ensuite sur .lock().unwrap() jusqu'au restart du service.
use parking_lot::Mutex;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, instrument, warn};

use atelier_common::conversation_meta::ConversationMetaStore;
use atelier_common::events::{AgentEvent, AgentOpenTabsEvent, StudioTabEvent};
use atelier_common::usage_stats::{TurnUsage, UsageStatsStore};

use crate::mcp::apps_ops::AppsContext;
use crate::state::ApiState;

/// Moteur d'agent d'une conversation. FIGÉ au binding de session (cf. doc du module) :
/// il détermine le script runner spawné, le store de sessions interrogé et la source
/// d'authentification. `claude` reste le défaut de tout ce qui ne le précise pas
/// (conversations d'avant l'axe engine : aucune ligne `agent_conversation_meta`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Engine {
    Claude,
    Codex,
}

impl Engine {
    fn as_str(self) -> &'static str {
        match self {
            Engine::Claude => "claude",
            Engine::Codex => "codex",
        }
    }

    /// `None` = valeur inconnue → l'appelant répond 400 (jamais de repli silencieux :
    /// un `engine` mal orthographié doit se voir, pas router vers Claude par accident).
    /// La chaîne vide vaut « non précisé » (défaut Claude).
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "" | "claude" => Some(Engine::Claude),
            "codex" => Some(Engine::Codex),
            _ => None,
        }
    }
}

/// Moteur IMPOSÉ par un nom de modèle, quand la famille est reconnaissable.
/// `None` = modèle absent ou famille inconnue — et c'est volontairement toléré :
/// « pas de modèle » signifie « défaut du moteur » (les deux en ont un), et un
/// resume dont le meta serveur a été perdu (Postgres down au binding) repartirait
/// sinon en 400 au lieu de simplement reprendre sur le défaut.
fn engine_of_model(model: Option<&str>) -> Option<Engine> {
    let m = model?.trim().to_ascii_lowercase();
    // Match sur la FAMILLE, pas sur un slug figé : le modèle Codex est `gpt-5.6-sol`
    // (le slug nu `gpt-5.6` n'existe pas côté CLI — métadonnées introuvables → fallback
    // dégradé), et les variantes suivantes resteront préfixées de la même façon.
    if m.starts_with("gpt") {
        Some(Engine::Codex)
    } else if m.starts_with("claude") {
        Some(Engine::Claude)
    } else {
        None
    }
}

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
    /// Moteur du run — figé au spawn. Fait autorité pour tout ce qui s'adresse au
    /// runner de CETTE session (set_model, snapshot, liste live).
    engine: Engine,
    session_id: Option<String>,
    cancel_tx: Option<oneshot::Sender<()>>,
    input_tx: mpsc::UnboundedSender<String>,
    items: Vec<Value>,
    /// Mode courant côté UI ('plan' | 'bypass'). Mis à jour par les events `permission_mode`
    /// (approbation de plan, /set_mode) → exposé dans le snapshot pour survivre au reload.
    mode: String,
    /// Modèle DEMANDÉ au spawn (None = défaut abonnement), suivi par les events `model`
    /// (/set_model live). Exposé dans le snapshot + persisté dans `agent_conversation_meta`.
    model: Option<String>,
    /// Effort demandé au spawn — figé côté SDK pour toute la session (pas d'API live).
    effort: Option<String>,
    /// Un tour est-il en vol ? `true` du dépôt d'un tour (init/`message`/`answer`/
    /// `plan_decision`) jusqu'au `turn_done`/`done`. Exposé dans le snapshot (`running`,
    /// pour restaurer l'indicateur de réflexion après un refresh) et utilisé par le drain.
    turn_active: bool,
    /// `dev` | `pm`; PM sessions are permanently read-only project managers.
    profile: String,
    /// `normal` | `brainstorm`, persisted per PM conversation.
    pm_mode: String,
}

static RUNS: LazyLock<Mutex<HashMap<String, RunState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SID_RUN: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Verrou single-flight de la MAJ SDK : `npm install` n'est pas transactionnel, deux MAJ
/// concurrentes corrompraient le snapshot/rollback. Posé/levé par [`sdk_update`] (garde RAII).
static SDK_UPDATING: AtomicBool = AtomicBool::new(false);

/// Verrou single-flight du smoke-test d'auth SDK (`op:auth_check` = un vrai tour
/// d'inférence). Évite d'empiler des `query()` concurrents (validation + probe).
static AUTH_PROBING: AtomicBool = AtomicBool::new(false);

/// Idem pour Codex, mais SÉPARÉ. WHY : le probe Codex dure jusqu'à 100 s (démarrage à
/// froid du CLI natif) ; partagé, il faisait échouer toute opération d'auth Claude
/// pendant toute sa durée alors que les deux moteurs ont des quotas disjoints.
static CODEX_AUTH_PROBING: AtomicBool = AtomicBool::new(false);

/// Écrit une ligne NDJSON sur le stdin du runner d'un run. `false` si inconnu/terminé
/// OU en cours d'arrêt (`cancel_tx` pris = drain demandé : la boucle ne lira plus
/// `input_rx`, la ligne partirait dans le vide alors que le handler répondrait 200 —
/// le 404 fait basculer le frontend sur son fallback resume).
fn send_input(run_id: &str, line: String) -> bool {
    RUNS.lock()
        .get(run_id)
        .filter(|r| r.cancel_tx.is_some())
        .map(|r| r.input_tx.send(line).is_ok())
        .unwrap_or(false)
}

fn user_item(text: &str) -> Value {
    json!({ "type": "user", "text": text })
}

/// Replie un event (kind+data) dans un buffer transcript. Miroir EXACT d'`appendEvent`
/// côté frontend : deltas consécutifs coalescés dans le dernier item, autres kinds =
/// items discrets. started/system/turn_done/done ne sont pas rendus.
fn fold_item(items: &mut Vec<Value>, kind: &str, data: &Value) {
    match kind {
        "assistant_delta" => {
            let text = data.get("text").and_then(|x| x.as_str()).unwrap_or("");
            if let Some(last) = items.last_mut() {
                if last.get("type").and_then(|x| x.as_str()) == Some("assistant") {
                    let prev = last.get("text").and_then(|x| x.as_str()).unwrap_or("");
                    last["text"] = json!(format!("{prev}{text}"));
                    return;
                }
            }
            items.push(json!({ "type": "assistant", "text": text }));
        }
        // Réflexion : on n'accumule QUE le compteur de caractères (jamais le texte) — le front
        // n'affiche qu'un count, donc le buffer/snapshot ne porte aucun détail de réflexion.
        "thinking_delta" => {
            let dchars = data.get("chars").and_then(|x| x.as_u64()).unwrap_or(0);
            if let Some(last) = items.last_mut() {
                if last.get("type").and_then(|x| x.as_str()) == Some("thinking") {
                    let prev = last.get("chars").and_then(|x| x.as_u64()).unwrap_or(0);
                    last["chars"] = json!(prev + dchars);
                    return;
                }
            }
            items.push(json!({ "type": "thinking", "chars": dchars }));
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
        // Dialogue expiré (annulation CLI du can_use_tool, cf. runner) : la carte reste
        // ACTIONNABLE — on la marque `idle` (l'agent s'est mis en pause) sans la clore.
        // Persistance reload : le snapshot d'une session vivante garde le hint.
        "question_idle" | "plan_idle" => {
            let ty = if kind == "question_idle" { "question" } else { "plan_review" };
            if let Some(rid) = data.get("request_id").and_then(|x| x.as_str()) {
                for it in items.iter_mut().rev() {
                    if it.get("type").and_then(|x| x.as_str()) == Some(ty)
                        && it.get("request_id").and_then(|x| x.as_str()) == Some(rid)
                    {
                        it["idle"] = json!(true);
                        break;
                    }
                }
            }
        }
        "error" => items.push(json!({ "type": "error", "message": data.get("message").and_then(|x| x.as_str()).unwrap_or("erreur") })),
        _ => {}
    }
}

/// Replie l'event dans le buffer du run (si encore présent au registre).
fn fold_into_run(run_id: &str, kind: &str, data: &Value) {
    if let Some(r) = RUNS.lock().get_mut(run_id) {
        // `permission_mode` n'est pas un item de transcript : il met à jour le mode courant
        // (exposé dans le snapshot pour survivre au reload), pas le fil.
        if kind == "permission_mode" {
            if let Some(m) = data.get("mode").and_then(|x| x.as_str()) {
                r.mode = m.to_string();
            }
        }
        // `model` (set_model live) : maj du modèle demandé — null = retour au défaut
        // abonnement (état explicite, distinct de « pas de changement »).
        if kind == "model" {
            r.model = data.get("model").and_then(|x| x.as_str()).map(String::from);
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
        // Réglages persistés de la conversation (agent_conversation_meta). Seul
        // l'effort est mutable ici : il n'a PAS d'API SDK live (figé au démarrage),
        // le changer recycle la session côté client (cancel → resume au prochain
        // message) — cet endpoint persiste l'INTENTION pour que les snapshots et
        // les autres PCs ne revertent pas le sélecteur avant ce resume.
        .route("/{slug}/agent/conversations/{sid}/settings", axum::routing::patch(patch_conversation_settings))
        // État d'UI des onglets ouverts (sync cross-PC) : autoritaire côté serveur,
        // poussé live via le canal WS `agent:open-tabs`.
        .route(
            "/{slug}/agent/open-tabs",
            get(get_open_tabs).put(put_open_tabs),
        )
        // Onglet TOP-NIVEAU du Studio par app (code/preview/…/surveillance) :
        // source de vérité serveur + porte le deep-link homepage→Studio via le
        // broadcast WS `studio:tab` (un onglet déjà ouvert bascule live).
        .route(
            "/{slug}/studio/tab",
            get(get_studio_tab).put(put_studio_tab),
        )
}

/// Routes globales, montées sous `/api/agent` :
///   GET    /api/agent/sdk/version
///   POST   /api/agent/sdk/update
///   GET    /api/agent/sdk/auth        (statut masqué ; ?probe=1 = smoke-test live)
///   POST   /api/agent/sdk/auth        (set token ; ?probe=1 = valider sans persister)
///   DELETE /api/agent/sdk/auth        (retire le token)
///   GET    /api/agent/apps-token      (token Claude des apps : statut ; ?probe=1)
///   POST   /api/agent/apps-token      (set token apps — validé avant persist)
///   DELETE /api/agent/apps-token      (retire le token apps)
///   GET    /api/agent/codex/sdk/version
///   POST   /api/agent/codex/sdk/update
///   GET    /api/agent/codex/auth      (statut + auth_file ; ?probe=1 = smoke-test live)
///   POST   /api/agent/codex/auth      (colle un auth.json — validé avant installation)
///   DELETE /api/agent/codex/auth      (efface fichier + seed)
///   {POST,GET,DELETE} /api/agent/codex/auth/device-login  (flow `codex login --device-auth`)
pub fn global_router() -> Router<ApiState> {
    Router::new()
        .route("/sdk/version", get(sdk_version))
        .route("/sdk/update", post(sdk_update))
        .route(
            "/sdk/auth",
            get(get_sdk_auth).post(set_sdk_auth).delete(delete_sdk_auth),
        )
        .route(
            "/apps-token",
            get(get_apps_token).post(set_apps_token).delete(delete_apps_token),
        )
        // Moteur Codex : mêmes surfaces (version/MAJ du SDK, auth) + le flow device-login
        // propre à l'OAuth abonnement ChatGPT en headless.
        .route("/codex/sdk/version", get(codex_sdk_version))
        .route("/codex/sdk/update", post(codex_sdk_update))
        .route(
            "/codex/auth",
            get(get_codex_auth).post(set_codex_auth).delete(delete_codex_auth),
        )
        .route(
            "/codex/auth/device-login",
            post(codex_device_login_start)
                .get(codex_device_login_status)
                .delete(codex_device_login_cancel),
        )
}

// --- État d'UI des onglets ouverts (sync cross-PC) ---
// Source de vérité côté serveur (`atelier_meta`). Le front charge cet état au
// montage, le PUT à chaque changement (debouncé), et reçoit les changements des
// autres PCs via le canal WS `agent:open-tabs` (broadcast émis par le PUT).

#[derive(Deserialize)]
struct OpenTabsBody {
    #[serde(default)]
    tabs: Value,
    #[serde(default)]
    active: Option<String>,
}

#[instrument(skip(state))]
async fn get_open_tabs(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    let (tabs, active) = state.open_tabs.get(&slug).await;
    Json(json!({ "tabs": tabs, "active": active }))
}

#[instrument(skip(state, body))]
async fn put_open_tabs(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<OpenTabsBody>,
) -> impl IntoResponse {
    if !body.tabs.is_array() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "tabs must be an array" })))
            .into_response();
    }
    if let Err(e) = state.open_tabs.set(&slug, &body.tabs, body.active.as_deref()).await {
        error!(slug = %slug, error = %e, "open_tabs set failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response();
    }
    // Broadcast à TOUS les clients connectés (y compris les autres PCs) → re-sync live.
    let _ = state.events.agent_open_tabs.send(AgentOpenTabsEvent {
        slug: slug.clone(),
        tabs: body.tabs.clone(),
        active: body.active.clone(),
    });
    info!(slug = %slug, "open tabs updated");
    Json(json!({ "ok": true })).into_response()
}

// --- Onglet top-niveau du Studio (sync cross-PC + deep-link homepage→Studio) ---
// Le front seed l'onglet depuis son cache localStorage (rendu instantané), lit
// CET état au montage (autoritaire, cross-PC), et reçoit les changements live via
// `studio:tab` (un onglet DÉJÀ ouvert bascule sans rechargement). La homepage PUT
// cet état avant d'ouvrir le Studio = le deep-link.

#[derive(Deserialize)]
struct StudioTabBody {
    tab: String,
    #[serde(default)]
    kind: Option<String>,
}

#[instrument(skip(state))]
async fn get_studio_tab(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    let (tab, kind) = state.open_tabs.get_studio_tab(&slug).await;
    Json(json!({ "tab": tab, "kind": kind }))
}

#[instrument(skip(state, body))]
async fn put_studio_tab(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<StudioTabBody>,
) -> impl IntoResponse {
    if body.tab.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "tab is required" })))
            .into_response();
    }
    if let Err(e) = state.open_tabs.set_studio_tab(&slug, &body.tab, body.kind.as_deref()).await {
        error!(slug = %slug, error = %e, "studio_tab set failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response();
    }
    // Broadcast → un onglet Studio déjà ouvert de cette app bascule en direct.
    let _ = state.events.studio_tab.send(StudioTabEvent {
        slug: slug.clone(),
        tab: body.tab.clone(),
        kind: body.kind.clone(),
    });
    info!(slug = %slug, tab = %body.tab, "studio tab updated");
    Json(json!({ "ok": true })).into_response()
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
/// Racine du runner d'un script (= parent de `src/`), où vivent `node_modules` + manifests
/// npm. `.../runner/src/runner.js` → `.../runner`. Source unique partagée par la MAJ SDK,
/// le check de version et la résolution du CLI codex vendorisé.
fn script_dir(script: &str) -> Option<PathBuf> {
    FsPath::new(script).parent()?.parent().map(|p| p.to_path_buf())
}
fn runner_dir() -> Option<PathBuf> {
    script_dir(&runner_script())
}
fn runner_script() -> String {
    std::env::var("ATELIER_AGENT_RUNNER").unwrap_or_else(|_| "/opt/atelier/runner/src/runner.js".into())
}
/// Shim NDJSON du moteur Codex (jumeau de `runner.js`, même protocole).
fn codex_script() -> String {
    std::env::var("ATELIER_CODEX_RUNNER").unwrap_or_else(|_| "/opt/atelier/runner/src/codex.js".into())
}
/// `$CODEX_HOME` : auth.json (écrit ET rotaté par le CLI lui-même) + sidecar de
/// conversations du shim. C'est un CHEMIN, pas un secret → il traverse sudo par
/// `--preserve-env`, exactement comme `CLAUDE_CONFIG_DIR`.
fn codex_home() -> String {
    std::env::var("ATELIER_AGENT_CODEX_HOME").unwrap_or_else(|_| "/var/lib/hr-studio/.codex".into())
}
/// CLI `codex` vendorisé par la dep optionnelle `@openai/codex-linux-x64`, relatif à
/// `node_modules`. Le shim laisse le SDK le résoudre ; on n'en a besoin en direct QUE
/// pour le device-login (flow interactif, hors SDK).
const CODEX_BIN_REL: &str =
    "node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex";
fn codex_bin() -> Option<PathBuf> {
    Some(script_dir(&codex_script())?.join(CODEX_BIN_REL))
}
/// Racine du runner Codex, avec repli sur l'arbre déployé (les ops one-shot n'ont pas
/// besoin d'un workspace d'app : n'importe quel cwd existant suffit).
fn codex_runner_dir() -> PathBuf {
    script_dir(&codex_script()).unwrap_or_else(|| FsPath::new("/opt/atelier/runner").to_path_buf())
}
fn run_as_user() -> String {
    std::env::var("ATELIER_AGENT_USER").unwrap_or_else(|_| "hr-studio".into())
}
/// Arbre SOURCE du runner (dépôt dev, sur Medion). La MAJ SDK y bumpe le pin durablement :
/// `make deploy` resynchronise /opt/atelier/runner DEPUIS cet arbre, donc sans bump source la MAJ
/// serait éphémère. Absent sur un hôte non-dev → la MAJ reste éphémère (signalé à l'UI).
fn source_runner_dir() -> PathBuf {
    std::env::var("ATELIER_RUNNER_SOURCE_DIR")
        .unwrap_or_else(|_| "/home/romain/atelier/runner".into())
        .into()
}
/// User propriétaire du dépôt source : on (ré)installe en SON nom pour préserver l'ownership git
/// (un `npm install` en root polluerait l'arbre de fichiers root-owned).
fn source_runner_user() -> String {
    std::env::var("ATELIER_RUNNER_SOURCE_USER").unwrap_or_else(|_| "romain".into())
}
fn claude_config_dir() -> String {
    std::env::var("ATELIER_AGENT_CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|_| "/var/lib/hr-studio/.claude".into())
}
fn pilot_claude_config_dir() -> String {
    std::env::var("ATELIER_PILOT_CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|_| "/var/lib/atelier/pilot/.claude".into())
}
fn atelier_source_root() -> PathBuf {
    std::env::var("ATELIER_SOURCE_ROOT")
        .unwrap_or_else(|_| "/home/romain/atelier".into())
        .into()
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

/// Construit la commande `sudo -u hr-studio node <shim>.js` partagée par le mode
/// session ([`run_agent`]) et le mode introspection ([`run_runner_op`]) : même user,
/// même cwd, un seul CHEMIN whitelisté dans l'env selon le moteur.
fn runner_command(cwd: &FsPath, engine: Engine) -> Command {
    // Whitelist d'env : une seule variable (un chemin, jamais un secret) par moteur
    // traverse l'env_reset de sudo. Les secrets — MCP_TOKEN, token OAuth, contenu
    // d'auth.json — passent TOUJOURS par stdin (sudo journalise son ENV=).
    let global_pm = engine == Engine::Claude && cwd == atelier_source_root();
    let (env_key, env_val, script) = match engine {
        Engine::Claude => (
            "CLAUDE_CONFIG_DIR",
            if global_pm { pilot_claude_config_dir() } else { claude_config_dir() },
            runner_script(),
        ),
        Engine::Codex => ("CODEX_HOME", codex_home(), codex_script()),
    };
    let mut cmd = Command::new("sudo");
    cmd.arg("-n")
        .arg("-H")
        .arg("-u")
        .arg(if global_pm { "romain".to_string() } else { run_as_user() })
        .arg(format!("--preserve-env={env_key}"))
        .arg("--")
        .arg(node_bin())
        .arg(script);
    cmd.current_dir(cwd);
    cmd.env(env_key, env_val);
    cmd
}

/// Budget par défaut d'une op one-shot (list/messages/rename/delete/tag) : purement
/// disque des deux côtés, 30 s est très large.
const RUNNER_OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Lance le runner en mode introspection one-shot (op:list/messages/rename/delete) :
/// écrit l'init sur stdin, ferme stdin (EOF), lit le PREMIER objet NDJSON émis, reape
/// le process. Pas d'EventBus — la réponse part directe en HTTP. Timeout court.
async fn run_runner_op(
    cwd: &FsPath,
    init_json: String,
    oauth_token: Option<&str>,
    engine: Engine,
) -> Result<Value, String> {
    run_runner_op_timeout(cwd, init_json, oauth_token, engine, RUNNER_OP_TIMEOUT).await
}

/// Variante à budget explicite : `op:auth_check` est un VRAI tour d'inférence (et,
/// côté Codex, un démarrage à froid du CLI natif) — il lui faut bien plus que les
/// 30 s des ops disque.
async fn run_runner_op_timeout(
    cwd: &FsPath,
    init_json: String,
    oauth_token: Option<&str>,
    engine: Engine,
    timeout: Duration,
) -> Result<Value, String> {
    // Le token OAuth abonnement (setup-token) est fusionné dans l'init ICI, pour
    // TOUS les ops : `assertOAuthOnly` (runner.js) exige désormais soit ce token
    // soit un `.credentials.json`. Passe par stdin (comme mcpToken) — jamais argv/env.
    // Côté Codex il n'y a rien à fusionner : l'auth EST le fichier `auth.json`.
    let init_json = match oauth_token.filter(|t| engine == Engine::Claude && !t.is_empty()) {
        Some(tok) => match serde_json::from_str::<Value>(&init_json) {
            Ok(mut v) => {
                v["oauthToken"] = json!(tok);
                v.to_string()
            }
            Err(_) => init_json,
        },
        None => init_json,
    };
    let mut cmd = runner_command(cwd, engine);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    // Groupe de process dédié, comme [`run_agent`] : `op:auth_check` spawne un PETIT-FILS
    // natif (`claude` / `codex exec`) que le seul `start_kill()` du sudo/node laisserait
    // vivant (100 s de CLI orphelin, quota consommé pour rien).
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("spawn runner: {e}"))?;
    let child_pid = child.id();
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
    let read = tokio::time::timeout(timeout, async {
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
    // Reap : le SIGKILL vise le GROUPE (le petit-fils natif n'est pas fils direct), puis
    // `start_kill()` couvre le cas non-unix / groupe déjà éteint.
    #[cfg(unix)]
    if let Some(pid) = child_pid {
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
    match read {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err("runner: aucune sortie".into()),
        Err(_) => Err("runner: timeout".into()),
    }
}

/// Image collée par l'utilisateur, transmise telle quelle au runner (qui en fait un
/// bloc `image` du message SDK). `data` = base64 brut (sans préfixe data-URL).
#[derive(Debug, Deserialize, Serialize)]
struct ImageInput {
    media_type: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct QueryBody {
    prompt: String,
    /// Moteur demandé ('claude' par défaut). Sur un `resume`, le moteur PERSISTÉ de la
    /// conversation prime (cf. [`query`]) — il est figé au binding.
    #[serde(default)]
    engine: Option<String>,
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
    #[serde(default)]
    images: Option<Vec<ImageInput>>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    pm_mode: Option<String>,
}

fn workspace_cwd(state: &ApiState, slug: &str) -> PathBuf {
    if slug == "@pilot" {
        PathBuf::from(std::env::var("ATELIER_SOURCE_ROOT").unwrap_or_else(|_| "/home/romain/atelier".into()))
    } else {
        state.apps_src_root.join(slug).join("src")
    }
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
    let n_images = body.images.as_ref().map(|v| v.len()).unwrap_or(0);
    if body.prompt.trim().is_empty() && n_images == 0 {
        return err(StatusCode::BAD_REQUEST, "prompt vide");
    }
    let profile = if body.profile.as_deref() == Some("pm") { "pm" } else { "dev" };
    let pm_mode = if body.pm_mode.as_deref() == Some("brainstorm") { "brainstorm" } else { "normal" };
    if slug == "@pilot" && profile != "pm" {
        return err(StatusCode::BAD_REQUEST, "@pilot est réservé au profil chef de projet");
    }
    // PM global reads the Atelier source root; app profiles keep the app workspace.
    let cwd = workspace_cwd(&state, &slug);
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, format!("app source introuvable: {}", cwd.display()));
    }

    let Some(mut engine) = Engine::parse(body.engine.as_deref().unwrap_or("")) else {
        return err(StatusCode::BAD_REQUEST, "engine invalide (claude|codex)");
    };
    if profile == "pm" { engine = Engine::Claude; }
    // Reprise : le moteur PERSISTÉ fait autorité (il est figé au binding, jamais mis à
    // jour). WHY on ne rejette PAS un désaccord : un client qui a perdu le meta de la
    // conversation (Postgres muet au moment du snapshot) enverrait le défaut `claude` et
    // se verrait refuser la reprise de son thread Codex — alors que la base sait à quel
    // runner l'adresser. On corrige et on trace.
    if let Some(sid) = body.resume.as_deref() {
        let persisted_meta = state.conversation_meta.get(&slug, sid).await;
        if let Some(bound_profile) = persisted_meta.as_ref()
            .and_then(|v| v.get("profile")).and_then(|v| v.as_str())
            && bound_profile != profile
        {
            return err(StatusCode::CONFLICT, "conversation liée à un autre profil");
        }
        let persisted = persisted_meta.as_ref()
            .and_then(|v| v.get("engine").and_then(|x| x.as_str()))
            .and_then(Engine::parse);
        if profile != "pm" && let Some(p) = persisted {
            if p != engine {
                warn!(slug = %slug, sid = %sid, asked = engine.as_str(), effective = p.as_str(), "resume: engine du client ignoré au profit du moteur persisté");
            }
            engine = p;
        }
    }
    // PM safety is invariant across resumes, including legacy metadata that
    // might have been bound to Codex before profiles existed.
    if profile == "pm" { engine = Engine::Claude; }
    // Incohérence modèle/moteur = bug client (le mauvais runner échouerait de façon
    // opaque). Un modèle absent ou de famille inconnue reste accepté (cf. engine_of_model).
    if let Some(want) = engine_of_model(body.model.as_deref())
        && want != engine
    {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "modèle « {} » incompatible avec le moteur {}",
                body.model.as_deref().unwrap_or(""),
                engine.as_str()
            ),
        );
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
        // La session est peut-être encore en train de se fermer (ex. changement d'effort →
        // cancel → flush SDK) : reprendre PENDANT le flush lirait un transcript tronqué et
        // ouvrirait un double-writer sur le même fichier de session. On attend la fin du
        // drain (borné) ; une session vivante NON en arrêt → 409 immédiat (le tour suivant
        // doit passer par /message, pas par un resume qui forkerait le runner).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let rid = SID_RUN.lock().get(sid).cloned();
            let Some(rid) = rid else { break };
            // Entrée RUNS absente (cleanup en cours) = arrêt en cours → on continue d'attendre.
            let ending = RUNS.lock().get(&rid).map(|r| r.cancel_tx.is_none()).unwrap_or(true);
            if !ending {
                return err(StatusCode::CONFLICT, "conversation encore vivante — envoie le message sur la session en cours");
            }
            if tokio::time::Instant::now() >= deadline {
                return err(StatusCode::CONFLICT, "session en cours de fermeture — réessaie dans un instant");
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        // Précharge le buffer d'AFFICHAGE depuis le transcript persisté (le modèle, lui, a
        // tout le contexte via le resume SDK). Un échec (timeout 30s sous charge) tronquerait
        // l'affichage du tour relancé TANT QU'IL EST LIVE → on réessaie une fois et on logge
        // au lieu d'avaler silencieusement. La perte n'est que transitoire : une fois le tour
        // fini, le snapshot repasse par op:messages sur disque (= transcript complet, resume
        // ne forkant pas la session).
        let m = json!({ "op": "messages", "sessionId": sid, "cwd": cwd.to_string_lossy() }).to_string();
        let oauth = state.agent_auth.token().await;
        let preload = match run_runner_op(&cwd, m.clone(), oauth.as_deref(), engine).await {
            Ok(v) => Ok(v),
            Err(_) => run_runner_op(&cwd, m, oauth.as_deref(), engine).await,
        };
        match preload {
            Ok(v) => {
                if let Some(arr) = v.get("items").and_then(|x| x.as_array()) {
                    items = arr.clone();
                }
            }
            Err(e) => warn!(slug = %slug, sid = %sid, error = %e, "resume transcript preload failed (display buffer truncated until turn ends)"),
        }
    }
    items.push(user_item(&body.prompt));

    // Mode initial côté UI ('plan' | 'bypass'), dérivé du permissionMode SDK demandé.
    let permission_mode = if profile == "pm" { "plan".into() } else { body.permission_mode.clone().unwrap_or_else(|| "plan".into()) };
    let ui_mode = if permission_mode == "bypassPermissions" { "bypass" } else { "plan" };

    // L'init est consommé par le shim du moteur (clés camelCase, communes aux deux).
    // Côté Claude, le token MCP ET le token OAuth abonnement (setup-token) passent ICI,
    // par stdin (pipe) — que ni Atelier ni sudo ne journalisent. (Les passer en env via
    // sudo --preserve-env les ferait apparaître en clair dans journald.) `oauthToken`
    // relu FRAIS ici → une ré-auth depuis Paramètres s'applique au prochain run sans
    // redémarrer le service.
    // Côté Codex l'init est volontairement PLUS PAUVRE : pas de MCP studio en v1 (donc
    // ni endpoint ni token), pas d'allowlist d'outils (le garde-fou est le sandbox du
    // CLI, piloté par `permissionMode`), et l'auth est le fichier `$CODEX_HOME/auth.json`.
    let wire_prompt = if profile == "pm" {
        format!("{}\n⟦/PM⟧\n{}", crate::pm_prompts::mode_header(pm_mode), body.prompt)
    } else { body.prompt.clone() };
    let pm_system = if slug == "@pilot" { crate::pm_prompts::PM_PREAMBLE_GLOBAL } else { crate::pm_prompts::PM_PREAMBLE_APP };
    let init = match engine {
        Engine::Codex => json!({
            "prompt": wire_prompt,
            "effort": body.effort, // None → null → le shim retombe sur 'medium'
            "permissionMode": permission_mode,
            "cwd": cwd.to_string_lossy(),
            "resume": body.resume,
            "model": body.model, // None → null → modèle par défaut du shim
            "images": body.images,
        }),
        Engine::Claude => json!({
            "prompt": wire_prompt,
            "effort": if profile == "pm" { Some("xhigh".to_string()) } else { body.effort.clone() },
            "permissionMode": permission_mode,
            "allowedTools": allowed_tools,
            "cwd": cwd.to_string_lossy(),
            "mcpEndpoint": if profile == "pm" {
                if slug == "@pilot" { format!("{}?scope=pilot", mcp_endpoint_base()) }
                else { format!("{}?scope=pilot&project={}", mcp_endpoint_base(), slug) }
            } else { format!("{}?project={}", mcp_endpoint_base(), slug) },
            "mcpToken": std::env::var("MCP_TOKEN").ok(),
            "oauthToken": state.agent_auth.token().await, // None → null → runner ignore (fallback creds)
            "resume": body.resume,
            "model": if profile == "pm" { Some("claude-opus-4-8[1m]".to_string()) } else { body.model.clone() },
            "images": body.images, // None → null → runner omet (texte seul)
            "systemAppend": if profile == "pm" { Some(pm_system) } else { None },
            "disallowedTools": if profile == "pm" { crate::pm_prompts::PM_DISALLOWED } else { &[] },
            "profile": profile,
        }),
    };
    if n_images > 0 {
        info!(run_id = %run_id, images = n_images, "agent query with pasted image(s)");
    }

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let (input_tx, input_rx) = mpsc::unbounded_channel::<String>();
    RUNS.lock().insert(
        run_id.clone(),
        RunState {
            slug: slug.clone(),
            engine,
            session_id: None,
            cancel_tx: Some(cancel_tx),
            input_tx,
            items,
            mode: ui_mode.to_string(),
            model: body.model.clone(),
            effort: body.effort.clone(),
            turn_active: true, // le prompt d'init est le tour #1
            profile: profile.to_string(),
            pm_mode: pm_mode.to_string(),
        },
    );

    info!(run_id = %run_id, engine = engine.as_str(), "agent run started");
    let events = state.events.clone();
    let meta = state.conversation_meta.clone();
    let notifications = state.notifications.clone();
    let agent_auth = state.agent_auth.clone();
    let codex_auth = state.codex_auth.clone();
    let usage_stats = state.usage_stats.clone();
    let run_id_task = run_id.clone();
    let slug_task = slug.clone();
    tokio::spawn(async move {
        run_agent(
            events,
            slug_task,
            run_id_task,
            engine,
            cwd,
            init.to_string(),
            cancel_rx,
            input_rx,
            meta,
            notifications,
            agent_auth,
            codex_auth,
            usage_stats,
        )
        .await;
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
    #[serde(default)]
    images: Option<Vec<ImageInput>>,
    #[serde(default)]
    pm_mode: Option<String>,
}

#[instrument(skip(state, body), fields(slug = %slug, run_id = %run_id))]
async fn message(
    State(state): State<ApiState>,
    Path((slug, run_id)): Path<(String, String)>,
    Json(body): Json<MessageBody>,
) -> impl IntoResponse {
    let _ = &state; // signature homogène ; état non requis
    let n_images = body.images.as_ref().map(|v| v.len()).unwrap_or(0);
    if body.text.trim().is_empty() && n_images == 0 {
        return err(StatusCode::BAD_REQUEST, "message vide");
    }
    let pm_mode = if body.pm_mode.as_deref() == Some("brainstorm") { "brainstorm" } else { "normal" };
    let (wire_text, sid, is_pm) = {
        let mut runs = RUNS.lock();
        let Some(r) = runs.get_mut(&run_id) else { return err(StatusCode::NOT_FOUND, "session inconnue ou terminée") };
        let is_pm = r.profile == "pm";
        if is_pm { r.pm_mode = pm_mode.into(); }
        let text = if is_pm { format!("{}\n⟦/PM⟧\n{}", crate::pm_prompts::mode_header(pm_mode), body.text) } else { body.text.clone() };
        (text, r.session_id.clone(), is_pm)
    };
    let line = json!({ "type": "user_message", "text": wire_text, "images": body.images }).to_string();
    // Marqueur d'image dans le buffer d'affichage (reload) si le tour est image-only.
    let display_text = if body.text.trim().is_empty() && n_images > 0 { "🖼 image".to_string() } else { body.text.clone() };
    if send_input(&run_id, line) {
        // Le runner ne ré-émet pas le tour user → on l'ajoute au buffer (reload).
        if let Some(r) = RUNS.lock().get_mut(&run_id) {
            r.items.push(user_item(&display_text));
            r.turn_active = true; // nouveau tour soumis
        }
        info!(run_id = %run_id, "agent message sent");
        if is_pm {
            if let Some(sid) = sid { state.conversation_meta.set_pm_mode(&slug, &sid, pm_mode).await; }
        }
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
        if let Some(r) = RUNS.lock().get_mut(&run_id) {
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
        if let Some(r) = RUNS.lock().get_mut(&run_id) {
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
    if RUNS.lock().get(&run_id).map(|r| r.profile.as_str()) == Some("pm") && body.mode != "plan" {
        return err(StatusCode::FORBIDDEN, "le profil chef de projet reste en lecture seule");
    }
    let line = json!({ "type": "set_mode", "mode": body.mode }).to_string();
    if send_input(&run_id, line) {
        if let Some(r) = RUNS.lock().get_mut(&run_id) {
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
    // Le moteur d'une session est figé : basculer sur un modèle de l'AUTRE moteur ne
    // veut rien dire (le shim l'accepterait puis le SDK échouerait au tour suivant).
    let Some(engine) = RUNS.lock().get(&run_id).map(|r| r.engine) else {
        return err(StatusCode::NOT_FOUND, "run inconnu ou terminé");
    };
    if RUNS.lock().get(&run_id).map(|r| r.profile.as_str()) == Some("pm") {
        return err(StatusCode::FORBIDDEN, "le modèle du profil chef de projet est figé");
    }
    if let Some(want) = engine_of_model(body.model.as_deref())
        && want != engine
    {
        return err(
            StatusCode::BAD_REQUEST,
            format!(
                "modèle « {} » incompatible avec le moteur {} de cette conversation",
                body.model.as_deref().unwrap_or(""),
                engine.as_str()
            ),
        );
    }
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

/// Sessions vivantes de cet app : `(session_id, run_id, résumé, moteur, profil, mode PM)`.
/// Sert à annoter
/// la liste `live` ET à y injecter les sessions pas encore flushées sur disque.
fn live_sessions_for(slug: &str) -> Vec<(String, String, String, Engine, String, String)> {
    let sid_runs: Vec<(String, String)> =
        SID_RUN.lock().iter().map(|(s, r)| (s.clone(), r.clone())).collect();
    let runs = RUNS.lock();
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
            Some((sid, rid, summary, r.engine, r.profile.clone(), r.pm_mode.clone()))
        })
        .collect()
}

/// Scheduler gate exposed to the Pilote wiring. It intentionally returns only
/// a boolean so the private engine/session metadata never leaks across modules.
pub fn has_live_sessions(slug: &str) -> bool {
    !live_sessions_for(slug).is_empty()
}

/// Moteur d'une conversation EXISTANTE, par ordre d'autorité DÉCROISSANT : run vivant
/// (`SID_RUN`/`RUNS`) → `agent_conversation_meta` → query-param client → `claude` (legacy :
/// aucune ligne meta avant l'axe engine). `Err` = param invalide.
///
/// WHY le param client en DERNIER recours : ce n'est qu'un écho du front, qui peut être
/// périmé (liste chargée avant la création de la conversation, onglet resté ouvert). Lui
/// donner l'autorité adressait rename/delete au mauvais shim et rendait le snapshot vide ;
/// pire, un `PATCH .../settings` sur une conversation SANS ligne meta CRÉAIT la ligne avec
/// le mauvais `engine` — jamais mis à jour ensuite, donc routage définitivement faux. Même
/// doctrine que la reprise dans `query` : la source autoritaire gagne, le désaccord est tracé.
async fn engine_for_sid(
    state: &ApiState,
    slug: &str,
    sid: &str,
    asked: Option<&str>,
) -> Result<Engine, axum::response::Response> {
    // Le param reste VALIDÉ même s'il ne tranche qu'en dernier : une valeur inconnue est
    // un bug client, pas un défaut silencieux.
    let asked = match asked.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => Some(
            Engine::parse(raw)
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "engine invalide (claude|codex)"))?,
        ),
        None => None,
    };
    // Locks pris l'un APRÈS l'autre (jamais imbriqués) : même ordre que partout ailleurs
    // dans ce fichier, pas d'inversion possible.
    let rid = SID_RUN.lock().get(sid).cloned();
    let live = rid.and_then(|rid| RUNS.lock().get(&rid).map(|r| r.engine));
    let authoritative = match live {
        Some(e) => Some((e, "run")),
        None => state
            .conversation_meta
            .get(slug, sid)
            .await
            .and_then(|v| v.get("engine").and_then(|x| x.as_str()).map(String::from))
            .and_then(|e| Engine::parse(&e))
            .map(|e| (e, "meta")),
    };
    if let Some((effective, source)) = authoritative {
        if let Some(a) = asked
            && a != effective
        {
            warn!(slug = %slug, sid = %sid, asked = a.as_str(), effective = effective.as_str(), source, "engine du client ignoré au profit du moteur autoritaire");
        }
        return Ok(effective);
    }
    Ok(asked.unwrap_or(Engine::Claude))
}

/// Query-param `?engine=` des endpoints de conversation (rename/delete/snapshot/settings).
#[derive(Debug, Deserialize)]
struct EngineQuery {
    #[serde(default)]
    engine: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationListQuery {
    #[serde(default)]
    profile: Option<String>,
}

/// `GET /api/apps/{slug}/agent/conversations` — liste les sessions des DEUX moteurs
/// (`op:list` de chaque shim, sur disque), chacune taggée `engine`, annotées
/// `live`/`run_id`, plus les sessions vivantes pas encore persistées (injectées en tête).
#[instrument(skip(state, q), fields(slug = %slug))]
async fn list_conversations(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<ConversationListQuery>,
) -> impl IntoResponse {
    let requested_profile = q.profile.as_deref().unwrap_or("dev");
    if !matches!(requested_profile, "dev" | "pm" | "all") {
        return err(StatusCode::BAD_REQUEST, "profile invalide (dev|pm|all)");
    }
    let cwd = workspace_cwd(&state, &slug);
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    let init = json!({ "op": "list", "cwd": cwd.to_string_lossy() }).to_string();
    let oauth = state.agent_auth.token().await;
    // Les deux moteurs persistent dans des espaces DISJOINTS : on interroge les deux
    // shims EN PARALLÈLE (deux spawns concurrents, pas deux fois la latence) et on
    // fusionne. L'indisponibilité d'un moteur (Codex pas encore déployé, par ex.) ne
    // doit PAS vider la liste de l'autre → liste partielle + warn.
    // Exception : la sentinelle @pilot (PM global) est Claude-only — le shim Codex
    // tournerait en hr-studio avec cwd /home/romain/atelier (EACCES) et produirait
    // un `unavailable` systématique et trompeur ; on ne le spawne pas du tout.
    let results: Vec<(Engine, Result<Value, String>)> = if slug == "@pilot" {
        vec![(
            Engine::Claude,
            run_runner_op(&cwd, init, oauth.as_deref(), Engine::Claude).await,
        )]
    } else {
        let (claude, codex) = tokio::join!(
            run_runner_op(&cwd, init.clone(), oauth.as_deref(), Engine::Claude),
            run_runner_op(&cwd, init, None, Engine::Codex),
        );
        vec![(Engine::Claude, claude), (Engine::Codex, codex)]
    };
    let queried = results.len();

    let mut conversations: Vec<Value> = Vec::new();
    // WHY nommer les moteurs en échec plutôt que les compter : fondue dans un compteur
    // anonyme, une panne du moteur PAR DÉFAUT (Claude) ressortait en 200 avec la seule
    // liste Codex — vide en pratique — donc en historique EFFACÉ à l'écran. La liste
    // partielle reste tolérée (Codex jamais configuré ne doit pas casser l'historique
    // Claude), mais le champ `unavailable` rend la panne visible au front.
    let mut unavailable: Vec<String> = Vec::new();
    for (engine, res) in results {
        match res {
            Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("sessions") => {
                for mut s in v.get("sessions").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
                    s["engine"] = json!(engine.as_str());
                    conversations.push(s);
                }
            }
            Ok(v) => {
                unavailable.push(engine.as_str().to_string());
                warn!(slug = %slug, engine = engine.as_str(), response = %v, "conversations: réponse runner inattendue (liste partielle)");
            }
            Err(e) => {
                unavailable.push(engine.as_str().to_string());
                warn!(slug = %slug, engine = engine.as_str(), error = %e, "conversations: moteur indisponible (liste partielle)");
            }
        }
    }
    if unavailable.len() == queried {
        return err(StatusCode::BAD_GATEWAY, "aucun moteur n'a pu lister les conversations");
    }

    let live = live_sessions_for(&slug);
    let mut on_disk: Vec<String> = Vec::new();
    for s in conversations.iter_mut() {
        let Some(sid) = s.get("sessionId").and_then(|x| x.as_str()).map(String::from) else {
            continue;
        };
        on_disk.push(sid.clone());
        match live.iter().find(|(lsid, ..)| *lsid == sid) {
            Some((_, rid, _, _, profile, pm_mode)) => {
                s["live"] = json!(true);
                s["run_id"] = json!(rid);
                s["profile"] = json!(profile);
                s["pm_mode"] = json!(pm_mode);
            }
            None => {
                s["live"] = json!(false);
                let meta = state.conversation_meta.get(&slug, &sid).await;
                s["profile"] = meta.as_ref().and_then(|m| m.get("profile")).cloned().unwrap_or_else(|| json!("dev"));
                s["pm_mode"] = meta.as_ref().and_then(|m| m.get("pm_mode")).cloned().unwrap_or_else(|| json!("normal"));
            }
        }
    }
    // Chaque shim trie SA liste : la fusion doit re-trier (`lastModified` en ms unix des
    // deux côtés), sinon toutes les conversations Codex se retrouveraient sous les Claude.
    conversations.sort_by_key(|c| {
        std::cmp::Reverse(c.get("lastModified").and_then(|x| x.as_u64()).unwrap_or(0))
    });
    for (sid, rid, summary, engine, profile, pm_mode) in live {
        if !on_disk.contains(&sid) {
            conversations.insert(0, json!({
                "sessionId": sid,
                "engine": engine.as_str(),
                "live": true,
                "run_id": rid,
                "summary": summary,
                "profile": profile,
                "pm_mode": pm_mode,
                "lastModified": now_ms(),
            }));
        }
    }
    if requested_profile != "all" {
        conversations.retain(|c| {
            c.get("profile").and_then(Value::as_str).unwrap_or("dev") == requested_profile
        });
    }
    Json(json!({ "conversations": conversations, "unavailable": unavailable })).into_response()
}

/// `GET /api/apps/{slug}/agent/conversations/{sid}` — snapshot : transcript (runner
/// `op:messages`) + `live`/`run_id` pour que le frontend se rebranche au WS.
#[instrument(skip(state, q), fields(slug = %slug, sid = %sid))]
async fn get_conversation(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
    Query(q): Query<EngineQuery>,
) -> axum::response::Response {
    let cwd = workspace_cwd(&state, &slug);
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    let engine = match engine_for_sid(&state, &slug, &sid, q.engine.as_deref()).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    // Session vivante → fil servi depuis le buffer mémoire (pas encore sur disque).
    let rid = SID_RUN.lock().get(&sid).cloned();
    if let Some(rid) = rid {
        let snap = RUNS
            .lock()
            .get(&rid)
            .map(|r| (r.items.clone(), r.mode.clone(), r.turn_active, r.model.clone(), r.effort.clone(), r.engine, r.profile.clone(), r.pm_mode.clone()));
        if let Some((items, mode, turn_active, model, effort, engine, profile, pm_mode)) = snap {
            // `running` (tour en vol) + `pending` (dialogue non résolu) permettent au
            // frontend de restaurer l'indicateur de réflexion / la carte d'attente après
            // un refresh (le WS broadcast ne rejoue pas `started`/`question`).
            let pending = pending_dialog(&items);
            // Le buffer ne porte déjà que des réflexions allégées (compteur `chars`, pas de
            // texte — cf. fold_item) → servi tel quel. `settings` = réglages demandés de la
            // session (le frontend y resynchronise ses sélecteurs, cross-PC).
            return Json(json!({
                "items": items,
                "live": true,
                "run_id": rid,
                "mode": mode.clone(),
                "running": turn_active,
                "pending": pending,
                "settings": { "engine": engine.as_str(), "model": model, "effort": effort, "mode": mode, "profile": profile, "pm_mode": pm_mode },
            }))
            .into_response();
        }
    }
    // Sinon → transcript persisté sur disque, chez le shim du moteur de la conversation.
    let init = json!({ "op": "messages", "sessionId": sid, "cwd": cwd.to_string_lossy() }).to_string();
    let oauth = state.agent_auth.token().await;
    match run_runner_op(&cwd, init, oauth.as_deref(), engine).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("transcript") => {
            // Réglages de la dernière exécution depuis le store. `settings: null` =
            // conversation legacy sans meta → le frontend garde ses défauts locaux.
            // `mode` top-level : même clé que le chemin vivant (le reducer front lit
            // `a.mode` → activeMode, y compris pour une conversation morte désormais).
            let settings = state.conversation_meta.get(&slug, &sid).await;
            let mode = settings.as_ref().and_then(|s| s.get("mode")).cloned().unwrap_or(Value::Null);
            Json(json!({
                "items": v.get("items").cloned().unwrap_or_else(|| json!([])),
                "live": false,
                "run_id": Value::Null,
                "mode": mode,
                "settings": settings.unwrap_or(Value::Null),
            }))
            .into_response()
        }
        Ok(v) => runner_bad_gateway(&v, "runner: réponse inattendue"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
struct SettingsBody {
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    pm_mode: Option<String>,
}

/// `PATCH /api/apps/{slug}/agent/conversations/{sid}/settings` — persiste l'effort
/// choisi pour la conversation (cf. commentaire de route : intention pré-resume).
#[instrument(skip(state, q, body), fields(slug = %slug, sid = %sid))]
async fn patch_conversation_settings(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
    Query(q): Query<EngineQuery>,
    Json(body): Json<SettingsBody>,
) -> axum::response::Response {
    // SUPERSET des paliers des deux moteurs : `max` n'existe que côté Claude, `minimal`
    // n'est pas exposé par l'UI. On ne filtre PAS par moteur ici — l'effort n'est qu'une
    // intention persistée, et le shim Codex clampe lui-même `max` → `xhigh` au spawn.
    // Rejeter `max` pour une conversation Codex casserait une préférence héritée d'un
    // sélecteur Claude sans rien protéger.
    if body.effort.is_none() && body.pm_mode.is_none() {
        return err(StatusCode::BAD_REQUEST, "aucun réglage fourni");
    }
    if let Some(effort) = body.effort.as_deref()
        && !["low", "medium", "high", "xhigh", "max"].contains(&effort)
    {
        return err(StatusCode::BAD_REQUEST, "effort invalide");
    }
    if let Some(mode) = body.pm_mode.as_deref()
        && !matches!(mode, "normal" | "brainstorm")
    {
        return err(StatusCode::BAD_REQUEST, "pm_mode invalide");
    }
    let engine = match engine_for_sid(&state, &slug, &sid, q.engine.as_deref()).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    // Cohérence du snapshot live pendant la fenêtre de drain (le run mourant sert
    // encore le buffer mémoire) : on reflète aussi l'effort dans le RunState.
    let rid = SID_RUN.lock().get(&sid).cloned();
    if body.pm_mode.is_some() {
        let live_is_pm = rid.as_ref().and_then(|id| RUNS.lock().get(id).map(|r| r.profile == "pm")).unwrap_or(false);
        let stored_is_pm = state.conversation_meta.get(&slug, &sid).await
            .and_then(|v| v.get("profile").and_then(|p| p.as_str()).map(|p| p == "pm"))
            .unwrap_or(false);
        if !live_is_pm && !stored_is_pm {
            return err(StatusCode::FORBIDDEN, "réglage PM réservé au profil chef de projet");
        }
    }
    if let Some(rid) = rid.as_ref() {
        if let Some(r) = RUNS.lock().get_mut(rid) {
            if let Some(effort) = body.effort.clone() { r.effort = Some(effort); }
            if let Some(mode) = body.pm_mode.clone() { r.pm_mode = mode; }
        }
    }
    if let Some(effort) = body.effort.as_deref() {
        state.conversation_meta.set_effort(&slug, &sid, engine.as_str(), effort).await;
    }
    if let Some(mode) = body.pm_mode.as_deref() {
        state.conversation_meta.set_pm_mode(&slug, &sid, mode).await;
    }
    (StatusCode::OK, Json(json!({"ok": true}))).into_response()
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

/// `PATCH /api/apps/{slug}/agent/conversations/{sid}` — renomme la session (titre SDK).
#[instrument(skip(state, q, body), fields(slug = %slug, sid = %sid))]
async fn rename_conversation(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
    Query(q): Query<EngineQuery>,
    Json(body): Json<RenameBody>,
) -> axum::response::Response {
    let cwd = workspace_cwd(&state, &slug);
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    let engine = match engine_for_sid(&state, &slug, &sid, q.engine.as_deref()).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let init = json!({ "op": "rename", "sessionId": sid, "title": body.title, "cwd": cwd.to_string_lossy() }).to_string();
    let oauth = state.agent_auth.token().await;
    match run_runner_op(&cwd, init, oauth.as_deref(), engine).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("ok") => {
            (StatusCode::OK, Json(json!({"ok": true}))).into_response()
        }
        Ok(v) => runner_bad_gateway(&v, "runner: échec rename"),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// `DELETE /api/apps/{slug}/agent/conversations/{sid}` — coupe le run vivant éventuel
/// puis supprime la session du disque.
#[instrument(skip(state, q), fields(slug = %slug, sid = %sid))]
async fn delete_conversation(
    State(state): State<ApiState>,
    Path((slug, sid)): Path<(String, String)>,
    Query(q): Query<EngineQuery>,
) -> axum::response::Response {
    let cwd = workspace_cwd(&state, &slug);
    if !cwd.is_dir() {
        return err(StatusCode::NOT_FOUND, "app source introuvable");
    }
    // Résolu AVANT de couper le run : une fois la session tuée, `RUNS` n'a plus son moteur.
    let engine = match engine_for_sid(&state, &slug, &sid, q.engine.as_deref()).await {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    // Conversation vivante sur cette session → on la coupe avant de supprimer le fichier.
    let rid = SID_RUN.lock().get(&sid).cloned();
    if let Some(rid) = rid {
        if let Some(tx) = RUNS.lock().get_mut(&rid).and_then(|r| r.cancel_tx.take()) {
            let _ = tx.send(());
        }
    }
    let init = json!({ "op": "delete", "sessionId": sid, "cwd": cwd.to_string_lossy() }).to_string();
    let oauth = state.agent_auth.token().await;
    match run_runner_op(&cwd, init, oauth.as_deref(), engine).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("ok") => {
            state.conversation_meta.delete(&slug, &sid).await;
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
/// Clone direct du pattern [`atelier_watcher::claude::ClaudeRunner::exec`] :
/// process group + SIGKILL au cancel/timeout pour reaper le binaire `claude`
/// petit-fils du SDK.
#[allow(clippy::too_many_arguments)]
async fn run_agent(
    events: std::sync::Arc<atelier_common::events::EventBus>,
    slug: String,
    run_id: String,
    engine: Engine,
    cwd: PathBuf,
    init_json: String,
    mut cancel: oneshot::Receiver<()>,
    mut input_rx: mpsc::UnboundedReceiver<String>,
    meta: ConversationMetaStore,
    notifications: atelier_common::notification_store::NotificationStore,
    agent_auth: atelier_common::agent_auth::AgentAuthStore,
    codex_auth: atelier_common::codex_auth::CodexAuthStore,
    usage_stats: UsageStatsStore,
) {
    let mut seq: u64 = 0;
    // Clé stable de la conversation : inconnue jusqu'à la 1re ligne `system` du runner.
    let mut session_id: Option<String> = None;
    publish(&events, &run_id, None, &slug, &mut seq, "started", json!({}));

    // Le MCP_TOKEN passe par l'init JSON (stdin), JAMAIS par l'env — sinon sudo le
    // journalise en clair dans son ENV=. ANTHROPIC_API_KEY / OPENAI_API_KEY et les DSN
    // root sont écartés par env_reset (hors whitelist d'un unique chemin par moteur).
    let mut cmd = runner_command(&cwd, engine);
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
                // Réflexion : on ne diffuse au front QUE le compteur (jamais le texte). Le texte
                // transite seulement runner→API (pipe interne) ; il s'arrête ici.
                let data = if kind == "thinking_delta" {
                    json!({ "chars": pending_text.chars().count() })
                } else {
                    json!({ "text": pending_text.clone() })
                };
                publish(events, run_id, session_id, slug, seq, &kind, data);
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
                        // authentication_failed du SDK (token OAuth abonnement mort/révoqué),
                        // émis typé par le runner (`code:'sdk_auth_failed'`). On remonte UNE
                        // notification plateforme (dédup atomique côté DB) EN PLUS de laisser
                        // l'event `error` s'afficher dans le chat. spawn : un Postgres lent ne
                        // doit pas geler le relay des events.
                        if t == "error"
                            && obj.get("code").and_then(|x| x.as_str()) == Some("sdk_auth_failed")
                        {
                            let msg = obj
                                .get("message")
                                .and_then(|x| x.as_str())
                                .unwrap_or("authentication_failed")
                                .to_string();
                            let notif = notifications.clone();
                            // Chaque moteur a SON store de télémétrie/dédup : une auth
                            // Claude morte ne doit pas éteindre la notification Codex
                            // (et réciproquement).
                            let (claude_auth, cx_auth) = (agent_auth.clone(), codex_auth.clone());
                            tokio::spawn(async move {
                                let interval = atelier_common::agent_auth::notify_interval_secs();
                                let (won, title, body) = match engine {
                                    Engine::Codex => (
                                        cx_auth.record_failure(&msg, interval).await,
                                        "Authentification Codex expirée",
                                        format!(
                                            "L'agent Codex ne peut plus appeler le modèle (session \
                                             ChatGPT expirée ou révoquée). Reconnecte le compte \
                                             depuis Paramètres → Moteur Codex. Détail : {msg}"
                                        ),
                                    ),
                                    Engine::Claude => (
                                        claude_auth.record_failure(&msg, interval).await,
                                        "Authentification Claude expirée",
                                        format!(
                                            "L'agent ne peut plus appeler le modèle (token OAuth \
                                             abonnement expiré/révoqué). Renouvelle-le \
                                             (`claude setup-token`) puis Paramètres → \
                                             Authentification Claude. Détail : {msg}"
                                        ),
                                    ),
                                };
                                if won {
                                    let _ = notif
                                        .push(None, "system", "notice", "error", title, Some(&body))
                                        .await;
                                }
                            });
                        }
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
                                SID_RUN.lock().insert(sid.to_string(), run_id.clone());
                                // Réglages demandés (modèle/effort/mode) → persistés au binding.
                                // Couvre query ET resume (chaque reprise re-émet `system` depuis
                                // un runner frais). Clonés SOUS le lock, upsertés HORS lock via
                                // spawn : un Postgres lent ne doit pas geler le relay des events.
                                let settings = {
                                    let mut runs = RUNS.lock();
                                    runs.get_mut(&run_id).map(|r| {
                                        r.session_id = Some(sid.to_string());
                                        (r.model.clone(), r.effort.clone(), r.mode.clone(), r.profile.clone(), r.pm_mode.clone())
                                    })
                                };
                                if let Some((model, effort, mode, profile, pm_mode)) = settings {
                                    let (meta, slug, sid) = (meta.clone(), slug.clone(), sid.to_string());
                                    // `engine` n'est écrit qu'à la CRÉATION de la ligne
                                    // (cf. ConversationMetaStore) : c'est ici qu'il fige
                                    // le moteur de la conversation.
                                    tokio::spawn(async move {
                                        meta.upsert(&slug, &sid, engine.as_str(), model.as_deref(), effort.as_deref(), &mode, &profile, &pm_mode).await;
                                    });
                                }
                                info!(run_id = %run_id, session_id = %sid, engine = engine.as_str(), "agent session bound");
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
                            // set_model / set_mode live → meta persisté (session déjà liée ;
                            // le `permission_mode` initial arrive AVANT `system` → skip, le
                            // binding ci-dessus écrit le mode courant juste après).
                            if let Some(sid) = session_id.as_deref() {
                                if t == "model" {
                                    let model = obj.get("model").and_then(|x| x.as_str()).map(String::from);
                                    let (meta, slug, sid) = (meta.clone(), slug.clone(), sid.to_string());
                                    tokio::spawn(async move { meta.set_model(&slug, &sid, engine.as_str(), model.as_deref()).await });
                                } else if t == "permission_mode" {
                                    if let Some(mode) = obj.get("mode").and_then(|x| x.as_str()).map(String::from) {
                                        let (meta, slug, sid) = (meta.clone(), slug.clone(), sid.to_string());
                                        tokio::spawn(async move { meta.set_mode(&slug, &sid, engine.as_str(), &mode).await });
                                    }
                                }
                            }
                            // Fin de tour → persiste tokens/coût/durée (page /stats), hors du
                            // chemin de relay (spawn). Modèle = celui demandé au spawn (RunState),
                            // None = défaut abonnement. `obj` est encore possédé (publish suit).
                            if t == "result" {
                                let usage = obj.get("usage");
                                let turn = TurnUsage {
                                    slug: slug.clone(),
                                    session_id: session_id.clone(),
                                    model: RUNS.lock().get(&run_id).and_then(|r| r.model.clone()),
                                    tokens_in: usage.and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64()),
                                    tokens_out: usage.and_then(|u| u.get("output_tokens")).and_then(|v| v.as_i64()),
                                    cache_read: usage.and_then(|u| u.get("cache_read_input_tokens")).and_then(|v| v.as_i64()),
                                    cache_creation: usage.and_then(|u| u.get("cache_creation_input_tokens")).and_then(|v| v.as_i64()),
                                    cost_usd: obj.get("total_cost_usd").and_then(|v| v.as_f64()),
                                    num_turns: obj.get("num_turns").and_then(|v| v.as_i64()),
                                    duration_ms: obj.get("duration_ms").and_then(|v| v.as_i64()),
                                    is_error: obj.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false),
                                };
                                let usage_stats = usage_stats.clone();
                                tokio::spawn(async move { usage_stats.insert_turn(turn).await });
                            }
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
    // stderr du runner (diagnostics, jamais de secret) pour le diagnostic, ET on
    // publie un event `error` — sans lui, un runner mort en plein tour (OOM, crash
    // SDK) se manifestait par un tour évaporé sans aucune explication dans l'UI.
    if !exit_ok && !cancelled {
        let tail: String = stderr.chars().rev().take(800).collect::<String>().chars().rev().collect();
        warn!(run_id = %run_id, timed_out, stderr_tail = %tail, "agent runner exited non-clean");
        let msg = if timed_out {
            "Le runner de l'agent a dépassé le délai maximum et a été arrêté.".to_string()
        } else if tail.trim().is_empty() {
            "Le runner de l'agent s'est arrêté de façon inattendue.".to_string()
        } else {
            format!(
                "Le runner de l'agent s'est arrêté de façon inattendue : {}",
                tail.trim()
            )
        };
        publish(
            &events,
            &run_id,
            session_id.as_deref(),
            &slug,
            &mut seq,
            "error",
            json!({"message": msg}),
        );
    }
    info!(run_id = %run_id, exit_ok, cancelled, timed_out, "agent run done");
    // Conversation plus vivante : on la retire des registres. La session est sur disque
    // (persistée par le SDK de façon incrémentale) → snapshot/list la reliront depuis là.
    if let Some(sid) = &session_id {
        SID_RUN.lock().remove(sid);
    }
    RUNS.lock().remove(&run_id);
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
        let mut runs = RUNS.lock();
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
            if RUNS.lock().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let remaining = RUNS.lock().len();
    info!(remaining, "agent drain complete");
}

// --- SDK version check / update (paramétré par moteur) ---

/// Ce qui change d'un moteur à l'autre dans la MAJ in-place d'un SDK (snapshot →
/// `npm install` → vérif → smoke-test → rollback). Le reste du pipeline est commun :
/// un seul chemin de code, donc un seul comportement à garantir.
struct EngineSdk {
    engine: Engine,
    /// Spec passée à `npm install`. Une seule suffit côté Codex : `@openai/codex-sdk`
    /// dépend EN DUR de `@openai/codex` à la MÊME version, qui tire lui-même le binaire
    /// natif `@openai/codex-linux-x64`.
    install_pkg: &'static str,
    /// Paquet dont on lit la version installée (= ce qu'on compare au registry npm).
    version_pkg: &'static str,
    /// Dossiers à snapshoter/restaurer ENSEMBLE : un rollback partiel laisserait le SDK
    /// et son binaire natif désaccordés (le pire des deux mondes).
    snapshot_pkgs: &'static [&'static str],
    /// Artefact dont la présence prouve que l'install est exploitable au runtime
    /// (dep optionnelle : `npm` la saute en silence si elle échoue).
    probe: &'static str,
    probe_is_dir: bool,
    script: fn() -> String,
}

impl EngineSdk {
    /// Racine du runner de ce moteur (là où vivent `node_modules` + les manifests).
    fn dir(&self) -> Option<PathBuf> {
        script_dir(&(self.script)())
    }
}

const CLAUDE_SDK: EngineSdk = EngineSdk {
    engine: Engine::Claude,
    install_pkg: "@anthropic-ai/claude-agent-sdk",
    version_pkg: "@anthropic-ai/claude-agent-sdk",
    snapshot_pkgs: &[
        "@anthropic-ai/claude-agent-sdk",
        "@anthropic-ai/claude-agent-sdk-linux-x64",
    ],
    probe: "@anthropic-ai/claude-agent-sdk-linux-x64",
    probe_is_dir: true,
    script: runner_script,
};

const CODEX_SDK: EngineSdk = EngineSdk {
    engine: Engine::Codex,
    install_pkg: "@openai/codex-sdk",
    version_pkg: "@openai/codex-sdk",
    snapshot_pkgs: &["@openai/codex-sdk", "@openai/codex", "@openai/codex-linux-x64"],
    probe: CODEX_BIN_REL_PROBE,
    probe_is_dir: false,
    script: codex_script,
};

/// Sonde Codex = le binaire CLI lui-même (fichier, pas dossier), relatif à `node_modules`.
const CODEX_BIN_REL_PROBE: &str =
    "@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex";

async fn fetch_latest_sdk(pkg: &str) -> Option<String> {
    let url = format!("https://registry.npmjs.org/{pkg}/latest");
    let resp = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?;
    let v: Value = resp.json().await.ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

/// Version d'un paquet installée dans `dir` (lue depuis son `node_modules`). Sert au déployé ET au source.
fn sdk_version_in(dir: &FsPath, pkg: &str) -> Option<String> {
    let manifest = dir.join("node_modules").join(pkg).join("package.json");
    let s = std::fs::read_to_string(manifest).ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(String::from)
}

fn installed_sdk_version(sdk: &EngineSdk) -> Option<String> {
    sdk_version_in(&sdk.dir()?, sdk.version_pkg)
}

/// Tronque un log à ses `n` derniers caractères (sûr UTF-8), pour le renvoyer en cas d'échec.
fn tail(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
}

/// Purge un éventuel reliquat de backup `*.sdk-bak` (run précédent interrompu, ou nettoyage post-succès).
fn cleanup_sdk_bak(dir: &FsPath, sdk: &EngineSdk) {
    let nm = dir.join("node_modules");
    for pkg in sdk.snapshot_pkgs {
        let _ = std::fs::remove_dir_all(nm.join(format!("{pkg}.sdk-bak")));
    }
    let _ = std::fs::remove_file(dir.join("package.json.sdk-bak"));
    let _ = std::fs::remove_file(dir.join("package-lock.json.sdk-bak"));
}

/// Snapshot des artefacts SDK avant install, pour rollback : manifests copiés, dossiers SDK
/// `rename`és de côté (atomique, même FS → npm les réinstalle frais). Le PREMIER paquet est
/// obligatoire (son absence = arbre cassé, on refuse d'installer à l'aveugle) ; les suivants
/// (deps natives optionnelles) sont best-effort.
fn snapshot_sdk(dir: &FsPath, sdk: &EngineSdk) -> std::io::Result<()> {
    cleanup_sdk_bak(dir, sdk);
    let nm = dir.join("node_modules");
    std::fs::copy(dir.join("package.json"), dir.join("package.json.sdk-bak"))?;
    let lock = dir.join("package-lock.json");
    if lock.exists() {
        std::fs::copy(&lock, dir.join("package-lock.json.sdk-bak"))?;
    }
    for (i, pkg) in sdk.snapshot_pkgs.iter().enumerate() {
        let src = nm.join(pkg);
        if i == 0 || src.exists() {
            std::fs::rename(&src, nm.join(format!("{pkg}.sdk-bak")))?;
        }
    }
    Ok(())
}

/// Rollback : dégage les dossiers fraîchement (mal) installés et remet le snapshot + les manifests.
/// Best-effort — on ne peut rien faire de plus utile que de loguer si une étape échoue.
fn restore_sdk(dir: &FsPath, sdk: &EngineSdk) {
    let nm = dir.join("node_modules");
    for pkg in sdk.snapshot_pkgs {
        let _ = std::fs::remove_dir_all(nm.join(pkg));
        let _ = std::fs::rename(nm.join(format!("{pkg}.sdk-bak")), nm.join(pkg));
    }
    let pkg_bak = dir.join("package.json.sdk-bak");
    if pkg_bak.exists() {
        let _ = std::fs::rename(&pkg_bak, dir.join("package.json"));
    }
    let lock_bak = dir.join("package-lock.json.sdk-bak");
    if lock_bak.exists() {
        let _ = std::fs::rename(&lock_bak, dir.join("package-lock.json"));
    }
}

/// `npm install <spec>` dans `dir`. `as_user=None` → exécution directe (process service = root,
/// arbre DÉPLOYÉ /opt/atelier/runner). `as_user=Some(u)` → `sudo -n -u u` (arbre SOURCE, écrit en
/// `u` pour préserver l'ownership du dépôt git). Retourne le log combiné (Ok) ou un échec (Err).
/// WHY env : sous `ProtectSystem=strict` les HOME réels sont read-only → HOME/cache npm forcés vers
/// `/tmp` (writable) ; cache SÉPARÉ par cas (le cache root n'est pas writable par le user source).
/// Via sudo, `--preserve-env` est requis (env_reset stripe sinon HOME/npm_config_cache/CI).
/// `--omit=dev` (jamais `--omit=optional` : la dep native linux-x64 est requise au runtime).
async fn run_npm_install(dir: &FsPath, spec: &str, as_user: Option<&str>) -> Result<String, String> {
    let npm = npm_bin();
    let (cache, mut cmd) = match as_user {
        Some(user) => {
            let mut c = Command::new("sudo");
            c.arg("-n")
                .arg("-u")
                .arg(user)
                .arg("--preserve-env=HOME,npm_config_cache,CI")
                .arg("--")
                .arg(&npm);
            ("/tmp/.npm-atelier-src", c)
        }
        None => ("/tmp/.npm-atelier", Command::new(&npm)),
    };
    cmd.arg("install")
        .arg(spec)
        .arg("--no-audit")
        .arg("--no-fund")
        .arg("--save-exact")
        .arg("--omit=dev")
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("HOME", "/tmp")
        .env("npm_config_cache", cache)
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

/// Bump DURABLE du pin SOURCE après une MAJ déployée réussie. WHY : `make deploy` resynchronise
/// /opt/atelier/runner DEPUIS l'arbre source — sans ce bump la MAJ serait écrasée au prochain
/// deploy. (Ré)installe en `source_runner_user()` (ownership git préservé). Best-effort : si ceci
/// échoue, l'appelant NE rollback PAS le déployé (déjà effectif), il signale juste `source_pinned:false`.
async fn pin_sdk_source(sdk: &EngineSdk, target: &str) -> Result<(), String> {
    let src = source_runner_dir();
    if !src.join("package.json").is_file() {
        return Err("arbre source absent (MAJ éphémère)".into());
    }
    let user = source_runner_user();
    let spec = format!("{}@{target}", sdk.install_pkg);
    run_npm_install(&src, &spec, Some(user.as_str())).await?;
    match sdk_version_in(&src, sdk.version_pkg) {
        Some(v) if v == target => Ok(()),
        other => Err(format!("pin source inattendu: {other:?} (attendu {target})")),
    }
}

// ===================== Authentification du Claude Agent SDK =====================
// Le runner tourne headless en hr-studio → on ne peut pas y relancer `claude login`
// (flow navigateur). Romain génère un token longue durée sur son poste
// (`claude setup-token`, ~1 an, inference-only) et le colle ici ; il est validé par
// un VRAI tour d'inférence (op:auth_check) puis stocké, et injecté au runner/scan par
// stdin (jamais argv/env). Une ré-auth s'applique au prochain run, sans restart.

#[derive(Debug, Deserialize)]
struct SdkAuthBody {
    token: String,
}

#[derive(Debug, Deserialize)]
struct AuthProbeQuery {
    #[serde(default)]
    probe: Option<String>,
}

/// Smoke-test d'auth : lance `op:auth_check` (un tour d'inférence minimal) sous
/// l'exec réel hr-studio. `candidate` = token à valider (None → utilise le
/// `.credentials.json` / le token déjà injecté par run_runner_op côté stocké).
/// `Ok(())` si l'inférence répond, `Err(msg)` sinon (auth morte / erreur). Le
/// verrou single-flight sérialise les tests concurrents.
async fn smoke_auth_check(candidate: Option<&str>, isolate: bool) -> Result<(), String> {
    if AUTH_PROBING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("un test d'authentification est déjà en cours".into());
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            AUTH_PROBING.store(false, Ordering::Release);
        }
    }
    let _guard = Guard;

    // cwd du process = le dossier runner installé (toujours présent). auth_check
    // n'a pas besoin d'un workspace d'app (pas de MCP, pas de settingSources).
    let dir = runner_dir().unwrap_or_else(|| FsPath::new("/opt/atelier/runner").to_path_buf());
    // `isolate` (token apps) : le runner valide le candidat SEUL, dans un
    // CLAUDE_CONFIG_DIR temp vide — aucun `.credentials.json` local ne peut masquer
    // un token invalide (une app n'a pas ce fallback). Cf. runner.js op:auth_check.
    let init = if isolate {
        json!({ "op": "auth_check", "authIsolate": true }).to_string()
    } else {
        json!({ "op": "auth_check" }).to_string()
    };
    match run_runner_op(&dir, init, candidate, Engine::Claude).await {
        Ok(v) => match v.get("t").and_then(|x| x.as_str()) {
            Some("auth_ok") => Ok(()),
            _ => {
                // Message runner si présent, sinon générique (auth échouée).
                let m = v
                    .get("message")
                    .and_then(|x| x.as_str())
                    .unwrap_or("le token n'authentifie pas (authentication_failed)");
                Err(m.to_string())
            }
        },
        Err(e) => Err(format!("smoke-test auth_check échoué: {e}")),
    }
}

/// `GET /api/agent/sdk/auth` — statut masqué (jamais la valeur du token).
/// `?probe=1` lance en plus un smoke-test live avec le token STOCKÉ et met à jour
/// la télémétrie (record_ok / record_failure).
#[instrument(skip(state))]
async fn get_sdk_auth(
    State(state): State<ApiState>,
    Query(q): Query<AuthProbeQuery>,
) -> impl IntoResponse {
    let mut status = state.agent_auth.status().await;
    if q.probe.as_deref() == Some("1") {
        let token = state.agent_auth.token().await;
        let probe = match smoke_auth_check(token.as_deref(), false).await {
            Ok(()) => {
                state.agent_auth.record_ok().await;
                json!({ "ok": true })
            }
            Err(e) => {
                // Débounce partagé avec la détection en vol (une seule notif/intervalle).
                if state
                    .agent_auth
                    .record_failure(&e, atelier_common::agent_auth::notify_interval_secs())
                    .await
                {
                    let _ = state
                        .notifications
                        .push(
                            None,
                            "system",
                            "notice",
                            "error",
                            "Authentification Claude expirée",
                            Some(&format!(
                                "Le test d'authentification a échoué : {e}. Renouvelle le token \
                                 (`claude setup-token`) dans Paramètres → Authentification Claude."
                            )),
                        )
                        .await;
                }
                json!({ "ok": false, "error": e })
            }
        };
        // Recharge le statut (record_ok/failure a bougé la télémétrie).
        status = state.agent_auth.status().await;
        if let Value::Object(ref mut m) = status {
            m.insert("probe".into(), probe);
        }
    }
    Json(status)
}

/// `POST /api/agent/sdk/auth` `{token}` — valide le token candidat par un vrai tour
/// d'inférence PUIS le persiste. Refuse un token vide ; 400 s'il n'authentifie pas.
/// La valeur n'est JAMAIS loguée (seulement sa longueur).
#[instrument(skip(state, body))]
async fn set_sdk_auth(
    State(state): State<ApiState>,
    Json(body): Json<SdkAuthBody>,
) -> impl IntoResponse {
    let token = body.token.trim().to_string();
    if token.is_empty() {
        return err(StatusCode::BAD_REQUEST, "token vide");
    }
    if !state.agent_auth.status().await.get("available").and_then(|v| v.as_bool()).unwrap_or(false) {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "control-plane Postgres indisponible — impossible de persister le token",
        );
    }
    // Valide AVANT de persister (le token candidat, pas le stocké).
    if let Err(e) = smoke_auth_check(Some(&token), false).await {
        info!(token_len = token.len(), "SDK auth: token rejeté (validation)"); // jamais la valeur
        return err(StatusCode::BAD_REQUEST, format!("le token n'authentifie pas : {e}"));
    }
    if let Err(e) = state.agent_auth.set_token(&token).await {
        error!(error = %e, "SDK auth: persistance du token échouée");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "échec de persistance du token");
    }
    info!(token_len = token.len(), "SDK auth: token validé et persisté");
    let _ = state
        .notifications
        .push(
            None,
            "system",
            "action",
            "info",
            "Authentification Claude reconfigurée",
            Some("Nouveau token OAuth abonnement validé — l'agent et les scans repartent."),
        )
        .await;
    Json(state.agent_auth.status().await).into_response()
}

/// `DELETE /api/agent/sdk/auth` — retire le token (retour au fallback
/// `.credentials.json` s'il existe).
#[instrument(skip(state))]
async fn delete_sdk_auth(State(state): State<ApiState>) -> impl IntoResponse {
    if let Err(e) = state.agent_auth.clear_token().await {
        error!(error = %e, "SDK auth: clear token échoué");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "échec du retrait du token");
    }
    Json(state.agent_auth.status().await).into_response()
}

// --- Token Claude destiné aux APPS (injecté en CLAUDE_CODE_OAUTH_TOKEN aux apps
// opt-in `claude_access`) — SÉPARÉ du token runner/scan ci-dessus. Même UX (colle
// un `claude setup-token`), même validation (op:auth_check), stocké dans
// `atelier_meta.app_claude_auth`. Endpoints sous `/api/agent/apps-token`.

/// Re-render le `.env` de chaque app opt-in `claude_access`. À appeler après
/// (re)configuration OU suppression du token Claude des apps.
///
/// WHY : `reconcile_app_env` est le SEUL writer du `.env` (cf. `env_ops.rs`).
/// `platform_env` lit le token FRAIS à chaque render, mais rien ne re-render
/// après un `set`/`clear` du token → un token posé/retiré APRÈS l'opt-in
/// n'atteignait jamais l'app (le restart relit un `.env` périmé, symptôme b de
/// iss-fa13bdb2 sur `hevy`). Même mécanique que la rotation `HR_DV_TOKEN`.
/// Best-effort, non bloquant. Renvoie le nombre d'apps effectivement reconciliées.
#[instrument(skip(state))]
async fn reconcile_claude_access_apps(state: &ApiState) -> usize {
    let ctx = AppsContext::from_api_state(state);
    let mut reconciled = 0usize;
    for app in state.supervisor.registry.list().await {
        if !app.claude_access {
            continue;
        }
        match ctx.reconcile_app_env(&app.slug, false).await {
            Ok(_) => reconciled += 1,
            Err(e) => {
                warn!(slug = %app.slug, error = %e, "apps-token: reconcile du .env échoué")
            }
        }
    }
    info!(reconciled, "apps-token: .env des apps claude_access re-rendus");
    reconciled
}

/// `GET /api/agent/apps-token` — statut masqué. `?probe=1` = smoke-test live avec
/// le token STOCKÉ + MAJ télémétrie.
#[instrument(skip(state))]
async fn get_apps_token(
    State(state): State<ApiState>,
    Query(q): Query<AuthProbeQuery>,
) -> impl IntoResponse {
    let mut status = state.app_claude_auth.status().await;
    if q.probe.as_deref() == Some("1") {
        let token = state.app_claude_auth.token().await;
        // isolate=true : le token apps est validé SEUL (les apps n'ont pas de fallback).
        let probe = match smoke_auth_check(token.as_deref(), true).await {
            Ok(()) => {
                state.app_claude_auth.record_ok().await;
                json!({ "ok": true })
            }
            Err(e) => {
                state.app_claude_auth.record_failure(&e).await;
                json!({ "ok": false, "error": e })
            }
        };
        status = state.app_claude_auth.status().await;
        if let Value::Object(ref mut m) = status {
            m.insert("probe".into(), probe);
        }
    }
    Json(status)
}

/// `POST /api/agent/apps-token` `{token}` — valide le token candidat par un vrai
/// tour d'inférence PUIS le persiste. La valeur n'est JAMAIS loguée.
#[instrument(skip(state, body))]
async fn set_apps_token(
    State(state): State<ApiState>,
    Json(body): Json<SdkAuthBody>,
) -> impl IntoResponse {
    let token = body.token.trim().to_string();
    if token.is_empty() {
        return err(StatusCode::BAD_REQUEST, "token vide");
    }
    if !state.app_claude_auth.status().await.get("available").and_then(|v| v.as_bool()).unwrap_or(false)
    {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "control-plane Postgres indisponible — impossible de persister le token",
        );
    }
    if let Err(e) = smoke_auth_check(Some(&token), true).await {
        info!(token_len = token.len(), "apps-token: token rejeté (validation)");
        return err(StatusCode::BAD_REQUEST, format!("le token n'authentifie pas : {e}"));
    }
    if let Err(e) = state.app_claude_auth.set_token(&token).await {
        error!(error = %e, "apps-token: persistance échouée");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "échec de persistance du token");
    }
    info!(token_len = token.len(), "apps-token: token validé et persisté");
    // Propagation immédiate : re-render le `.env` des apps opt-in pour que le token
    // neuf les atteigne SANS re-toggle ni attente du boot-sweep (un restart relirait
    // sinon un `.env` périmé, sans `CLAUDE_CODE_OAUTH_TOKEN`).
    let reconciled = reconcile_claude_access_apps(&state).await;
    let body = format!(
        "CLAUDE_CODE_OAUTH_TOKEN propagé à {reconciled} app(s) opt-in (claude_access) — \
         redémarre-les pour qu'elles reprennent le nouveau token."
    );
    let _ = state
        .notifications
        .push(
            None,
            "system",
            "action",
            "info",
            "Token Claude des apps configuré",
            Some(body.as_str()),
        )
        .await;
    Json(state.app_claude_auth.status().await).into_response()
}

/// `DELETE /api/agent/apps-token` — retire le token ET re-render immédiatement le
/// `.env` des apps opt-in pour en retirer `CLAUDE_CODE_OAUTH_TOKEN` (sinon la valeur
/// révoquée persiste dans le `.env` jusqu'au prochain reconcile fortuit).
#[instrument(skip(state))]
async fn delete_apps_token(State(state): State<ApiState>) -> impl IntoResponse {
    if let Err(e) = state.app_claude_auth.clear_token().await {
        error!(error = %e, "apps-token: clear échoué");
        return err(StatusCode::INTERNAL_SERVER_ERROR, "échec du retrait du token");
    }
    reconcile_claude_access_apps(&state).await;
    Json(state.app_claude_auth.status().await).into_response()
}

// ========================= Authentification du moteur Codex =========================
// OAuth abonnement ChatGPT UNIQUEMENT — aucune clé API n'est acceptée nulle part dans
// la chaîne (le shim échoue d'ailleurs si `OPENAI_API_KEY`/`CODEX_API_KEY` traîne dans
// l'env). Différence de nature avec Claude : la vérité runtime est le fichier
// `$CODEX_HOME/auth.json`, écrit ET rotaté par le CLI lui-même. Postgres n'en garde
// qu'un SEED (le contenu collé depuis Paramètres) + la télémétrie et la dédup de
// notification — d'où `auth_file` composé ici, à côté du statut de la base.
//
// Deux chemins d'authentification, tous deux headless :
//   1. device-login : `codex login --device-auth` → lien + code affichés dans l'UI, le
//      CLI écrit auth.json quand l'utilisateur approuve depuis n'importe quel navigateur.
//   2. coller un `auth.json` généré sur un poste : validé par un vrai tour ISOLÉ avant
//      d'être installé (un auth.json déjà présent masquerait sinon un candidat invalide).

#[derive(Debug, Deserialize)]
struct CodexAuthBody {
    auth_json: String,
}

/// Contrôle de FORME d'un `auth.json` candidat : abonnement ChatGPT UNIQUEMENT.
///
/// WHY (invariant contournable sans ça) : le format réel du fichier (struct `AuthDotJson`
/// du CLI) est `{auth_mode, OPENAI_API_KEY, tokens:{id_token, access_token, refresh_token,
/// account_id}, last_refresh, agent_identity, personal_access_token, bedrock_api_key}` —
/// un fichier en MODE CLÉ API porte donc sa clé au PREMIER niveau et reste un objet JSON
/// parfaitement valide : il passait les seules gardes « ne commence pas par sk- » et
/// « est un objet JSON ». On rejette explicitement tous les porteurs de clé et on exige
/// POSITIVEMENT le couple de tokens OAuth. Le shim applique le même contrôle en miroir
/// dans `op:set_auth_json` (défense en profondeur : cet endpoint n'est pas le seul writer).
fn validate_codex_auth_json(raw: &str) -> Result<(), String> {
    let parsed: Value = serde_json::from_str(raw).map_err(|_| {
        "contenu invalide : colle le CONTENU de ~/.codex/auth.json (un objet JSON), pas une clé API"
            .to_string()
    })?;
    let obj = parsed.as_object().ok_or_else(|| {
        "contenu invalide : colle le CONTENU de ~/.codex/auth.json (un objet JSON), pas une clé API"
            .to_string()
    })?;
    let non_empty = |k: &str| {
        obj.get(k)
            .and_then(|x| x.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    };
    for key in ["OPENAI_API_KEY", "openai_api_key", "personal_access_token", "bedrock_api_key", "api_key"] {
        if non_empty(key) {
            return Err(format!(
                "clé API refusée : cet auth.json porte le champ `{key}` — Codex doit s'authentifier \
                 avec l'abonnement ChatGPT (retire ce champ, ou reconnecte le compte via device-login)"
            ));
        }
    }
    if let Some(mode) = obj.get("auth_mode").and_then(|x| x.as_str())
        && !mode.trim().eq_ignore_ascii_case("chatgpt")
    {
        return Err(format!(
            "mode d'authentification refusé : `auth_mode` vaut « {mode} » — seul l'abonnement \
             ChatGPT (« chatgpt ») est accepté"
        ));
    }
    let tokens = obj.get("tokens").and_then(|x| x.as_object()).ok_or_else(|| {
        "auth.json incomplet : l'objet `tokens` (abonnement ChatGPT) est absent — ce fichier \
         n'authentifie pas par abonnement"
            .to_string()
    })?;
    for key in ["access_token", "refresh_token"] {
        let ok = tokens
            .get(key)
            .and_then(|x| x.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if !ok {
            return Err(format!(
                "auth.json incomplet : `tokens.{key}` est absent ou vide (attendu : le fichier \
                 généré par `codex login` sur un poste connecté)"
            ));
        }
    }
    Ok(())
}

/// Budget d'`op:auth_check` côté Codex. WHY largement au-dessus des 30 s des ops disque :
/// c'est un vrai tour d'inférence PLUS un démarrage à froid du CLI natif (~300 Mo) ; le
/// shim s'auto-avorte à 90 s, on lui laisse le temps de rendre SON verdict typé plutôt
/// que de renvoyer un « runner: timeout » qui ne dit rien.
const CODEX_AUTH_CHECK_TIMEOUT: Duration = Duration::from_secs(100);

/// Smoke-test d'auth Codex : `op:auth_check` = un mini-tour RÉEL (un `codex login status`
/// ne dirait rien d'un refresh token révoqué côté serveur). `candidate` = contenu
/// d'auth.json à valider EN ISOLATION (le shim l'écrit dans un `$CODEX_HOME` temporaire) ;
/// `None` = teste l'auth installée. Sérialisé par un verrou PROPRE à Codex
/// ([`CODEX_AUTH_PROBING`]) : ces tours coûtent du quota, on n'en empile pas — mais ils
/// ne doivent pas bloquer les tests d'auth de l'autre moteur.
async fn smoke_codex_auth(candidate: Option<&str>) -> Result<(), String> {
    if CODEX_AUTH_PROBING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("un test d'authentification Codex est déjà en cours".into());
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            CODEX_AUTH_PROBING.store(false, Ordering::Release);
        }
    }
    let _guard = Guard;

    let init = match candidate {
        // Le candidat voyage par STDIN (init JSON), jamais en argv/env.
        Some(c) => json!({ "op": "auth_check", "authJson": c }),
        None => json!({ "op": "auth_check" }),
    }
    .to_string();
    match run_runner_op_timeout(
        &codex_runner_dir(),
        init,
        None,
        Engine::Codex,
        CODEX_AUTH_CHECK_TIMEOUT,
    )
    .await
    {
        Ok(v) => match v.get("t").and_then(|x| x.as_str()) {
            Some("auth_ok") => Ok(()),
            _ => Err(v
                .get("message")
                .and_then(|x| x.as_str())
                .unwrap_or("l'authentification Codex ne répond pas")
                .to_string()),
        },
        Err(e) => Err(format!("smoke-test auth_check échoué: {e}")),
    }
}

/// Présence de `$CODEX_HOME/auth.json` — SEULE vérité runtime (un device-login ne passe
/// jamais par Postgres, donc `configured=false` + `auth_file=true` est un état NORMAL).
/// Interrogée via `op:auth_status` du shim (qui résout `CODEX_HOME` exactement comme le
/// CLI et tourne en hr-studio, propriétaire du dossier) plutôt qu'en stat direct.
///
/// `None` = présence INDÉTERMINÉE (spawn KO, timeout, réponse d'une garde du shim). WHY
/// ne surtout PAS l'assimiler à « absent » : `get_codex_auth` réhydrate `auth.json` depuis
/// le seed Postgres quand le fichier manque, et le CLI rotate ce fichier tout seul — un
/// seed périmé écrasant un fichier VIVANT produit « refresh token was already used ». Une
/// simple lenteur de spawn casserait alors une authentification qui marchait.
async fn codex_auth_file_present() -> Option<bool> {
    let init = json!({ "op": "auth_status" }).to_string();
    match run_runner_op(&codex_runner_dir(), init, None, Engine::Codex).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("auth_status") => {
            v.get("auth_file").and_then(|x| x.as_bool())
        }
        Ok(v) => {
            warn!(response = %v, "codex: auth_status — réponse inattendue, présence indéterminée");
            None
        }
        Err(e) => {
            warn!(error = %e, "codex: auth_status indisponible, présence indéterminée");
            None
        }
    }
}

/// Statut de la base + `auth_file` (présence du fichier). Forme unique de réponse des
/// trois verbes de `/api/agent/codex/auth`. `auth_file: null` = état inconnu — on ne
/// ment pas avec `false`, le front doit pouvoir distinguer « pas de fichier » de
/// « shim muet ».
async fn codex_auth_status(state: &ApiState) -> Value {
    let mut status = state.codex_auth.status().await;
    let auth_file = codex_auth_file_present().await;
    if let Value::Object(ref mut m) = status {
        m.insert("auth_file".into(), json!(auth_file));
    }
    status
}

/// `GET /api/agent/codex/auth` — statut masqué (jamais le contenu d'auth.json).
/// `?probe=1` lance en plus un smoke-test live sur l'auth INSTALLÉE et met à jour la
/// télémétrie (record_ok / record_failure + notification dédupliquée).
#[instrument(skip(state))]
async fn get_codex_auth(
    State(state): State<ApiState>,
    Query(q): Query<AuthProbeQuery>,
) -> impl IntoResponse {
    let mut status = codex_auth_status(&state).await;
    // Rehydratation du fichier depuis le SEED — la raison d'être annoncée de ce stockage
    // (restaurer l'auth après une perte de /var/lib ou une réinstallation du runner).
    // WHY jamais d'écrasement : le CLI rotate `auth.json` tout seul et fait foi ; réécrire
    // par-dessus avec un seed périmé casserait une authentification VIVANTE. On ne restaure
    // donc que sur une ABSENCE CERTIFIÉE (`Some(false)`) — jamais sur `null`, qui ne dit
    // rien du fichier (shim injoignable). Best-effort : un échec laisse le statut tel quel.
    if status.get("auth_file").and_then(|x| x.as_bool()) == Some(false)
        && let Some(seed) = state.codex_auth.token().await
    {
        let init = json!({ "op": "set_auth_json", "authJson": seed }).to_string();
        match run_runner_op(&codex_runner_dir(), init, None, Engine::Codex).await {
            Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("ok") => {
                info!("codex auth: auth.json restauré depuis le seed");
                status = codex_auth_status(&state).await;
            }
            Ok(v) => warn!(response = %v, "codex auth: restauration du seed refusée par le shim"),
            Err(e) => warn!(error = %e, "codex auth: restauration du seed échouée"),
        }
    }
    if q.probe.as_deref() == Some("1") {
        let probe = match smoke_codex_auth(None).await {
            Ok(()) => {
                state.codex_auth.record_ok().await;
                json!({ "ok": true })
            }
            Err(e) => {
                // Débounce PARTAGÉ avec la détection en vol (cf. run_agent) : une seule
                // notification par intervalle, quel que soit le nombre de sources.
                if state
                    .codex_auth
                    .record_failure(&e, atelier_common::agent_auth::notify_interval_secs())
                    .await
                {
                    let _ = state
                        .notifications
                        .push(
                            None,
                            "system",
                            "notice",
                            "error",
                            "Authentification Codex expirée",
                            Some(&format!(
                                "Le test d'authentification a échoué : {e}. Reconnecte le compte \
                                 ChatGPT depuis Paramètres → Moteur Codex."
                            )),
                        )
                        .await;
                }
                json!({ "ok": false, "error": e })
            }
        };
        status = codex_auth_status(&state).await;
        if let Value::Object(ref mut m) = status {
            m.insert("probe".into(), probe);
        }
    }
    Json(status)
}

/// `POST /api/agent/codex/auth` `{auth_json}` — valide le contenu candidat par un vrai
/// tour d'inférence ISOLÉ, l'installe dans `$CODEX_HOME/auth.json` (0600, écrit par le
/// shim en hr-studio) puis persiste le seed. Le contenu n'est JAMAIS logué (longueur seule).
#[instrument(skip(state, body))]
async fn set_codex_auth(
    State(state): State<ApiState>,
    Json(body): Json<CodexAuthBody>,
) -> axum::response::Response {
    let raw = body.auth_json.trim().to_string();
    if raw.is_empty() {
        return err(StatusCode::BAD_REQUEST, "auth.json vide");
    }
    // Refus EXPLICITE d'une clé API : c'est la confusion attendue (le champ ressemble à
    // celui du token Claude), et une clé basculerait silencieusement la facturation hors
    // abonnement si elle atteignait le CLI.
    if raw.starts_with("sk-") {
        return err(
            StatusCode::BAD_REQUEST,
            "clé API refusée : Codex s'authentifie avec l'abonnement ChatGPT — colle le CONTENU de ~/.codex/auth.json",
        );
    }
    // Puis la FORME du credential (un auth.json en mode clé API est un objet JSON valide
    // qui ne commence pas par « sk- » : les deux gardes ci-dessus ne suffisent pas).
    if let Err(e) = validate_codex_auth_json(&raw) {
        info!(len = raw.len(), "codex auth: auth.json rejeté (forme)"); // jamais le contenu
        return err(StatusCode::BAD_REQUEST, e);
    }
    // Validation AVANT installation : un auth.json invalide écrasé sur le fichier courant
    // déconnecterait le moteur pour rien.
    if let Err(e) = smoke_codex_auth(Some(&raw)).await {
        info!(len = raw.len(), "codex auth: auth.json rejeté (validation)"); // jamais le contenu
        return err(StatusCode::BAD_REQUEST, format!("cet auth.json n'authentifie pas : {e}"));
    }
    let init = json!({ "op": "set_auth_json", "authJson": raw }).to_string();
    match run_runner_op(&codex_runner_dir(), init, None, Engine::Codex).await {
        Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("ok") => {}
        Ok(v) => return runner_bad_gateway(&v, "runner: échec de l'installation d'auth.json"),
        Err(e) => return err(StatusCode::BAD_GATEWAY, e),
    }
    // Le seed n'est qu'une COPIE de secours (le fichier fait foi) : son échec ne doit pas
    // annuler une authentification qui, elle, fonctionne déjà.
    let mut status = codex_auth_status(&state).await;
    if let Err(e) = state.codex_auth.set_token(&raw).await {
        warn!(error = %e, "codex auth: seed non persisté (auth.json installé quand même)");
        if let Value::Object(ref mut m) = status {
            m.insert("note".into(), json!("auth.json installé, seed non persisté (control-plane indisponible)"));
        }
    } else {
        status = codex_auth_status(&state).await;
    }
    info!(len = raw.len(), "codex auth: auth.json validé et installé");
    let _ = state
        .notifications
        .push(
            None,
            "system",
            "action",
            "info",
            "Authentification Codex configurée",
            Some("Nouvel auth.json validé (abonnement ChatGPT) — le moteur Codex est prêt."),
        )
        .await;
    Json(status).into_response()
}

/// `DELETE /api/agent/codex/auth` — efface le SEED PUIS le fichier.
///
/// WHY cet ordre, et pourquoi un échec du seed est fatal : l'inverse (fichier d'abord)
/// laissait, quand `clear_token` échouait (Postgres down), un seed orphelin en base — et
/// le prochain `GET` voyait `auth_file=false` et RÉINSTALLAIT l'authentification depuis ce
/// seed. Un retrait qui ressuscite tout seul est pire que pas de retrait du tout : on
/// s'arrête donc net, sans toucher au fichier, et on le dit à l'utilisateur.
#[instrument(skip(state))]
async fn delete_codex_auth(State(state): State<ApiState>) -> axum::response::Response {
    if let Err(e) = state.codex_auth.clear_token().await {
        error!(error = %e, "codex auth: seed non effacé — retrait interrompu, auth.json conservé");
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "seed non effacé (control-plane indisponible) : {e}. auth.json a été CONSERVÉ \
                 pour ne pas laisser un retrait à moitié fait (le seed le réinstallerait). Réessaie."
            ),
        );
    }
    let init = json!({ "op": "clear_auth_json" }).to_string();
    if let Err(e) = run_runner_op(&codex_runner_dir(), init, None, Engine::Codex).await {
        warn!(error = %e, "codex auth: suppression d'auth.json échouée (seed déjà effacé)");
    }
    info!("codex auth: authentification retirée");
    Json(codex_auth_status(&state).await).into_response()
}

// --- Flow « device auth » (`codex login --device-auth`) ---------------------------
// Le serveur est headless : impossible d'y ouvrir le navigateur du flow OAuth classique.
// Le CLI sait en revanche afficher un lien + un code d'appairage sur stdout, puis attendre
// l'approbation avant d'écrire auth.json lui-même. On pilote donc ce process : on lit son
// stdout, on en extrait lien et code pour l'UI, et on surveille sa sortie.

/// Durée de vie du code d'appairage annoncée par le CLI (15 min) : au-delà, le process
/// attend une approbation qui ne viendra plus → on le tue pour ne pas laisser un `codex
/// login` orphelin pendu sur le serveur.
const DEVICE_LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Attente MAXIMALE avant de répondre au POST : le lien et le code sortent en ~1-3 s, mais
/// on ne bloque pas la requête au-delà — le client HTTP coupe à 30 s, et son GET de suivi
/// récupérera de toute façon ce qui manque.
const DEVICE_LOGIN_FIRST_WAIT: Duration = Duration::from_secs(20);

/// Flow device-login en cours (un seul à la fois, tout Atelier confondu : deux `codex
/// login` concurrents se battraient pour écrire le même auth.json).
#[derive(Debug, Clone, Default)]
struct DeviceLoginState {
    /// "" (= idle) | "pending" | "ok" | "error"
    status: String,
    url: Option<String>,
    code: Option<String>,
    error: Option<String>,
    started_at: u64,
    /// PID du `sudo` chef de groupe — sert au kill (annulation / timeout).
    child_pid: Option<u32>,
}

static DEVICE_LOGIN: LazyLock<Mutex<DeviceLoginState>> =
    LazyLock::new(|| Mutex::new(DeviceLoginState::default()));

fn device_login_json(st: &DeviceLoginState) -> Value {
    json!({
        "status": if st.status.is_empty() { "idle" } else { st.status.as_str() },
        "url": st.url,
        "code": st.code,
        "error": st.error,
    })
}

/// Retire les séquences d'échappement ANSI : le CLI colore sa sortie même sans TTY, et
/// l'URL/le code extraits porteraient sinon des `\x1b[…m` collés (lien mort, code faux).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.next() {
            // CSI : ESC [ … <lettre finale>
            Some('[') => {
                for c2 in it.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            // OSC : ESC ] … BEL ou ESC
            Some(']') => {
                for c2 in it.by_ref() {
                    if c2 == '\u{7}' || c2 == '\u{1b}' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// URL d'appairage dans une ligne du CLI (`https://auth.openai.com/codex/device`).
fn extract_url(line: &str) -> Option<String> {
    let start = line.find("https://").or_else(|| line.find("http://"))?;
    let rest = &line[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ')', '"', '\'']);
    (!url.is_empty()).then(|| url.to_string())
}

/// Code d'appairage : deux groupes MAJUSCULES/chiffres séparés d'un tiret (`JAQK-FRN2R`).
fn extract_device_code(line: &str) -> Option<String> {
    let alnum_upper = |p: &str, min: usize, max: usize| {
        p.len() >= min
            && p.len() <= max
            && p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    };
    for tok in line.split_whitespace() {
        let t = tok.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
        let Some((a, b)) = t.split_once('-') else { continue };
        if alnum_upper(a, 4, 4) && alnum_upper(b, 4, 8) {
            return Some(t.to_string());
        }
    }
    None
}

/// Tue le groupe de process du flow (SIGKILL, comme le reap de [`run_agent`]) : le chef de
/// groupe est `sudo`, le CLI codex est son enfant — un simple kill du PID le laisserait vivant.
fn kill_device_login(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// `$CODEX_HOME` doit exister AVANT le spawn (le CLI refuse de démarrer sinon) et
/// appartenir à hr-studio (c'est lui qui y écrira auth.json) → mkdir EN SON NOM, jamais
/// en root, sous peine d'un dossier root-owned que le CLI ne pourra pas remplir.
async fn ensure_codex_home() -> Result<(), String> {
    let home = codex_home();
    let out = Command::new("sudo")
        .arg("-n")
        .arg("-u")
        .arg(run_as_user())
        .arg("--")
        .arg("/bin/mkdir")
        .arg("-p")
        .arg("-m")
        .arg("700")
        .arg(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("mkdir {home}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("mkdir {home}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// `POST /api/agent/codex/auth/device-login` — démarre le flow. 202 avec `url`+`code` dès
/// qu'ils sont sortis du CLI (sinon `pending` nu : le GET les fournira), 409 si un flow
/// est déjà en cours.
#[instrument(skip(state))]
async fn codex_device_login_start(State(state): State<ApiState>) -> axum::response::Response {
    // Single-flight, avec purge d'un `pending` PÉRIMÉ : la tâche de suivi est censée
    // reprendre la main au plus tard au timeout, mais si elle disparaît (panic) l'état
    // resterait « en cours » à vie et plus personne ne pourrait s'authentifier. Au-delà
    // du budget du flow, on récupère la place (et on tue le process resté derrière).
    {
        let mut st = DEVICE_LOGIN.lock();
        if st.status == "pending" {
            let age = now_ms().saturating_sub(st.started_at);
            if age < DEVICE_LOGIN_TIMEOUT.as_millis() as u64 {
                return err(
                    StatusCode::CONFLICT,
                    "une connexion par code d'appareil est déjà en cours",
                );
            }
            let stale = st.child_pid.take();
            *st = DeviceLoginState::default();
            drop(st);
            warn!(pid = ?stale, "codex device-login: flow périmé récupéré");
            kill_device_login(stale);
        }
    }
    let Some(bin) = codex_bin().filter(|p| p.is_file()) else {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "CLI codex introuvable (dep optionnelle @openai/codex-linux-x64 absente du runner)",
        );
    };
    if let Err(e) = ensure_codex_home().await {
        error!(error = %e, "codex device-login: $CODEX_HOME non préparé");
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("préparation de CODEX_HOME impossible: {e}"));
    }

    let mut cmd = Command::new("sudo");
    cmd.arg("-n")
        .arg("-H")
        .arg("-u")
        .arg(run_as_user())
        .arg("--preserve-env=CODEX_HOME")
        .arg("--")
        .arg(&bin)
        .arg("login")
        .arg("--device-auth")
        .env("CODEX_HOME", codex_home())
        // cwd neutre : le flow n'a rien à voir avec un workspace d'app.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "codex device-login: spawn impossible");
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("spawn codex login: {e}"));
        }
    };
    let pid = child.id();
    {
        let mut st = DEVICE_LOGIN.lock();
        *st = DeviceLoginState {
            status: "pending".into(),
            started_at: now_ms(),
            child_pid: pid,
            ..Default::default()
        };
    }
    info!(pid = ?pid, "codex device-login: démarré");

    let codex_auth = state.codex_auth.clone();
    let notifications = state.notifications.clone();
    tokio::spawn(async move {
        // Lecture ligne à ligne : le CLI écrit le lien PUIS le code, puis reste en attente
        // de l'approbation (il ne sort qu'une fois auth.json écrit).
        let reader = child.stdout.take().map(|o| {
            tokio::spawn(async move {
                let mut lines = BufReader::new(o).lines();
                while let Ok(Some(raw)) = lines.next_line().await {
                    let line = strip_ansi(&raw);
                    let mut st = DEVICE_LOGIN.lock();
                    if st.status != "pending" || st.child_pid != pid {
                        break; // flow annulé ou remplacé : plus rien à alimenter
                    }
                    if st.url.is_none() {
                        st.url = extract_url(&line);
                    }
                    if st.code.is_none() {
                        st.code = extract_device_code(&line);
                    }
                }
            })
        });
        let err_task = child.stderr.take().map(|e| {
            tokio::spawn(async move {
                let mut buf = String::new();
                let _ = BufReader::new(e).read_to_string(&mut buf).await;
                buf
            })
        });

        let waited = tokio::time::timeout(DEVICE_LOGIN_TIMEOUT, child.wait()).await;
        let verdict: Result<(), String> = match waited {
            Err(_) => {
                kill_device_login(pid);
                let _ = child.start_kill();
                let _ = child.wait().await;
                Err("délai dépassé (15 min) — le code a expiré, relance la connexion".into())
            }
            Ok(Ok(s)) if s.success() => Ok(()),
            Ok(Ok(s)) => Err(format!("codex login s'est arrêté ({s})")),
            Ok(Err(e)) => Err(format!("codex login: {e}")),
        };
        if let Some(r) = reader {
            let _ = r.await;
        }
        let stderr_tail = match err_task {
            Some(h) => tail(h.await.unwrap_or_default().trim(), 300),
            None => String::new(),
        };

        // Le flow a pu être annulé (DELETE) ou remplacé entre-temps : on ne réécrit QUE
        // l'état qui est encore le nôtre, sinon on ressusciterait un « error » sur un
        // flow que l'utilisateur a déjà clos.
        {
            let mut st = DEVICE_LOGIN.lock();
            if st.status != "pending" || st.child_pid != pid {
                return;
            }
            match &verdict {
                Ok(()) => {
                    st.status = "ok".into();
                    st.error = None;
                }
                Err(e) => {
                    st.status = "error".into();
                    st.error = Some(if stderr_tail.is_empty() {
                        e.clone()
                    } else {
                        format!("{e} — {stderr_tail}")
                    });
                }
            }
            st.child_pid = None;
        }
        match verdict {
            Ok(()) => {
                info!("codex device-login: authentifié");
                codex_auth.record_ok().await;
                let _ = notifications
                    .push(
                        None,
                        "system",
                        "action",
                        "info",
                        "Authentification Codex configurée",
                        Some("Connexion par code d'appareil réussie — le moteur Codex est prêt."),
                    )
                    .await;
            }
            Err(e) => warn!(error = %e, stderr = %stderr_tail, "codex device-login: échec"),
        }
    });

    // Court sursis pour renvoyer lien + code DANS la réponse (l'UI les affiche aussitôt).
    let deadline = tokio::time::Instant::now() + DEVICE_LOGIN_FIRST_WAIT;
    loop {
        {
            let st = DEVICE_LOGIN.lock();
            if st.status != "pending" || (st.url.is_some() && st.code.is_some()) {
                return (StatusCode::ACCEPTED, Json(device_login_json(&st))).into_response();
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let st = DEVICE_LOGIN.lock();
    (StatusCode::ACCEPTED, Json(device_login_json(&st))).into_response()
}

/// `GET /api/agent/codex/auth/device-login` — avancement du flow (consulté en boucle par
/// l'UI le temps de l'approbation).
#[instrument]
async fn codex_device_login_status() -> impl IntoResponse {
    let st = DEVICE_LOGIN.lock();
    Json(device_login_json(&st))
}

/// `DELETE /api/agent/codex/auth/device-login` — annule : kill du groupe de process et
/// retour à `idle` (la tâche de suivi verra que l'état n'est plus le sien et se taira).
#[instrument]
async fn codex_device_login_cancel() -> impl IntoResponse {
    let pid = {
        let mut st = DEVICE_LOGIN.lock();
        let pid = st.child_pid.take();
        *st = DeviceLoginState::default();
        pid
    };
    kill_device_login(pid);
    info!(pid = ?pid, "codex device-login: annulé");
    Json(json!({ "status": "idle" }))
}

async fn sdk_version_json(sdk: &EngineSdk) -> Json<Value> {
    let installed = installed_sdk_version(sdk);
    let latest = fetch_latest_sdk(sdk.version_pkg).await;
    let update_available = matches!((&installed, &latest), (Some(i), Some(l)) if i != l);
    Json(json!({
        "installed": installed,
        "latest": latest,
        "update_available": update_available,
    }))
}

#[instrument]
async fn sdk_version() -> impl IntoResponse {
    sdk_version_json(&CLAUDE_SDK).await
}

#[instrument]
async fn codex_sdk_version() -> impl IntoResponse {
    sdk_version_json(&CODEX_SDK).await
}

#[derive(Debug, Deserialize)]
struct SdkUpdateBody {
    #[serde(default)]
    version: Option<String>,
}

/// MAJ DURABLE du SDK d'un moteur dans le runner installé : snapshot → `npm install` → vérif
/// (version cible + artefact natif) → smoke-test (`op:list`, charge le SDK sous l'exec réel
/// hr-studio) → rollback si échec. L'effet déployé porte sur la PROCHAINE session agent (runner
/// spawné frais) — pas de restart. En cas de succès, le pin SOURCE est aussi bumpé
/// (`pin_sdk_source`) pour survivre aux `make deploy` (qui resynchronisent le déployé depuis la
/// source) ; ce bump source est best-effort (non-fatal, reporté via `source_pinned`/`source_note`).
async fn sdk_update_impl(
    state: ApiState,
    body: Option<Json<SdkUpdateBody>>,
    sdk: &EngineSdk,
) -> axum::response::Response {
    // Verrou GLOBAL, tous moteurs confondus (WHY) : les deux SDK vivent dans le MÊME
    // `node_modules` (un seul arbre runner) — deux `npm install` concurrents s'y
    // marcheraient dessus, et chacun snapshoterait les manifests que l'autre réécrit.
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

    let dir = match sdk.dir() {
        Some(d) if d.join("node_modules").is_dir() => d,
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "runner introuvable"),
    };

    // Cible : version explicite (body) sinon la dernière publiée au registry npm.
    let target = match body.and_then(|b| b.0.version).filter(|v| !v.trim().is_empty()) {
        Some(v) => v.trim().to_string(),
        None => match fetch_latest_sdk(sdk.version_pkg).await {
            Some(v) => v,
            None => return err(StatusCode::BAD_GATEWAY, "registry npm injoignable"),
        },
    };

    let installed = installed_sdk_version(sdk);
    if installed.as_deref() == Some(target.as_str()) {
        info!(engine = sdk.engine.as_str(), version = %target, "MAJ SDK : déjà à jour");
        return (
            StatusCode::OK,
            Json(json!({ "installed": installed, "latest": target, "updated": false, "note": "déjà à jour" })),
        )
            .into_response();
    }

    info!(engine = sdk.engine.as_str(), target = %target, dir = %dir.display(), from = ?installed, "MAJ SDK : début");

    if let Err(e) = snapshot_sdk(&dir, sdk) {
        error!(engine = sdk.engine.as_str(), error = %e, "MAJ SDK : snapshot impossible");
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("snapshot impossible: {e}"));
    }

    let spec = format!("{}@{target}", sdk.install_pkg);
    let outcome: Result<(), String> = match run_npm_install(&dir, &spec, None).await {
        Err(log) => Err(log),
        Ok(_) => {
            let now = installed_sdk_version(sdk);
            let probe = dir.join("node_modules").join(sdk.probe);
            let probe_ok = if sdk.probe_is_dir { probe.is_dir() } else { probe.is_file() };
            if now.as_deref() != Some(target.as_str()) {
                Err(format!("version post-install inattendue: {now:?} (attendu {target})"))
            } else if !probe_ok {
                Err(format!("dep native {} absente après install", sdk.probe))
            } else {
                // Smoke-test : op:list charge le SDK et tourne sous l'exec réel hr-studio.
                // Le token OAuth n'a de sens que côté Claude (Codex s'authentifie par
                // fichier, et `op:list` est de toute façon disque-only).
                let init = json!({ "op": "list", "cwd": dir.to_string_lossy() }).to_string();
                let oauth = match sdk.engine {
                    Engine::Claude => state.agent_auth.token().await,
                    Engine::Codex => None,
                };
                match run_runner_op(&dir, init, oauth.as_deref(), sdk.engine).await {
                    Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("sessions") => Ok(()),
                    Ok(v) => Err(format!("smoke-test op:list inattendu: {v}")),
                    Err(e) => Err(format!("smoke-test op:list échoué: {e}")),
                }
            }
        }
    };

    match outcome {
        Ok(()) => {
            cleanup_sdk_bak(&dir, sdk);
            // Bump DURABLE du pin source (survit aux make deploy). Non-fatal : le déployé est déjà
            // à jour et effectif — un échec ici ne fait que ramener à l'état éphémère (signalé UI).
            let (source_pinned, source_note) = match pin_sdk_source(sdk, &target).await {
                Ok(()) => {
                    info!(engine = sdk.engine.as_str(), version = %target, "MAJ SDK : pin source mis à jour");
                    (true, None)
                }
                Err(e) => {
                    warn!(engine = sdk.engine.as_str(), error = %tail(&e, 500), "MAJ SDK : pin source NON mis à jour (reviendra au prochain deploy)");
                    (false, Some(tail(&e, 500)))
                }
            };
            info!(engine = sdk.engine.as_str(), version = %target, source_pinned, "MAJ SDK : succès");
            (
                StatusCode::OK,
                Json(json!({
                    "installed": target,
                    "latest": target,
                    "updated": true,
                    "source_pinned": source_pinned,
                    "source_note": source_note,
                })),
            )
                .into_response()
        }
        Err(log) => {
            restore_sdk(&dir, sdk);
            let restored = installed_sdk_version(sdk);
            warn!(engine = sdk.engine.as_str(), error = %tail(&log, 500), restored = ?restored, "MAJ SDK : rollback");
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

#[instrument(skip(state, body))]
async fn sdk_update(
    State(state): State<ApiState>,
    body: Option<Json<SdkUpdateBody>>,
) -> axum::response::Response {
    sdk_update_impl(state, body, &CLAUDE_SDK).await
}

#[instrument(skip(state, body))]
async fn codex_sdk_update(
    State(state): State<ApiState>,
    body: Option<Json<SdkUpdateBody>>,
) -> axum::response::Response {
    sdk_update_impl(state, body, &CODEX_SDK).await
}
