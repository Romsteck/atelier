//! MCP (Model Context Protocol) HTTP endpoint for Atelier.
//!
//! Ported verbatim from homeroute's `hr-orchestrator::mcp` after the Atelier
//! cutover. Adapted for Atelier's `ApiState`. The homeroute-only infra tools
//! (hosts.*, monitoring.*, reverseproxy.*) were removed — they
//! belong to the router half, not Atelier.
//!
//! Implements JSON-RPC 2.0 over HTTP POST, with Bearer token authentication.
//! Tools: app.*, db.*, docs.*, todos.*, flow.*, studio.*, git.*

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use atelier_docs::{DocType, Frontmatter, Store, validate_app_id, validate_entry_name};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::state::ApiState;

// ── JSON-RPC types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

// JSON-RPC error codes
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

// ── Shared state ────────────────────────────────────────────────────

/// MCP shared state, derived from Atelier's `ApiState`. The fields the
/// ported handlers expect are kept under the same names so the tool
/// function bodies remain untouched.
#[derive(Clone)]
pub struct McpState {
    pub token: Arc<String>,
    pub git: Arc<atelier_git::GitService>,
    pub apps_ctx: Option<crate::mcp::apps_ops::AppsContext>,
    /// FTS5 index for `docs.search`. None if FTS init failed at boot.
    pub docs_index: Option<Arc<atelier_docs::Index>>,
    /// Docs filesystem root. Mirrors `ApiState::docs_dir`; passed explicitly
    /// (resolved from `ATELIER_DOCS_DIR`) rather than relying on the default.
    pub docs_dir: PathBuf,
    /// Surveillance IA service — exposes findings_* / memory_* / runs_* tools.
    pub surveillance: atelier_watcher::SurveillanceService,
}

impl McpState {
    /// Build the MCP state from the Atelier `ApiState` + the `MCP_TOKEN`
    /// env var. Returns `None` if no token is set (MCP is then disabled).
    pub fn from_api_state(state: &ApiState) -> Option<Self> {
        let token = std::env::var("MCP_TOKEN").ok()?;
        if token.is_empty() {
            return None;
        }
        // Gestion des routes hr-edge non câblée ici (`edge: None` dans
        // AppsContext::from_api_state) ; les call sites set/remove route s'auto-skip
        // avec un warn. À reprendre : le socket hr-edge est désormais local.
        // Canal de build = le canal PARTAGÉ (state.events.app_build) relayé par le
        // WebSocket, pas un canal jetable — sinon les AppBuildEvent du MCP `app.build`
        // partiraient dans le vide (badge mort).
        let apps_ctx = crate::mcp::apps_ops::AppsContext::from_api_state(state);
        Some(Self {
            token: Arc::new(token),
            git: state.git.clone(),
            apps_ctx: Some(apps_ctx),
            docs_index: state.docs_index.clone(),
            docs_dir: state.docs_dir.clone(),
            surveillance: state.surveillance.clone(),
        })
    }
}

/// Mount the `/mcp` POST endpoint on top of `ApiState`. The handler
/// reconstructs an `McpState` from the inner state on every request — cheap
/// (a handful of `Arc::clone`s).
pub fn router() -> Router<ApiState> {
    Router::new().route("/", post(mcp_entrypoint))
}

async fn mcp_entrypoint(
    State(state): State<ApiState>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
    body: String,
) -> axum::response::Response {
    let Some(mcp) = McpState::from_api_state(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {
                    "code": -32000,
                    "message": "MCP disabled: MCP_TOKEN env var is not set"
                }
            })),
        )
            .into_response();
    };
    mcp_handler(State(mcp), query, headers, body)
        .await
        .into_response()
}

// ── Handler ─────────────────────────────────────────────────────────

pub async fn mcp_handler(
    State(state): State<McpState>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
    body: String,
) -> axum::response::Response {
    let project_slug = query.get("project").cloned();
    // Read-only surveillance scope: `?scope=surveillance` restricts the tool set
    // (list + dispatch) to a read-only whitelist, so the surveillance Codex
    // cannot reach destructive tools (app.delete, db_drop_table, app.exec, …)
    // even with the global MCP token. Enforced by capability, not just by the
    // AGENTS.md instruction. Inert unless the param is present.
    let readonly = query.get("scope").map(|s| s == "surveillance").unwrap_or(false);
    // ── Auth ──
    let authorized = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t == state.token.as_str())
        .unwrap_or(false);

    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32000, "message": "Unauthorized"}}),
            ),
        )
            .into_response();
    }

    // ── Parse JSON-RPC request ──
    let request: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::OK,
                Json(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    format!("Parse error: {e}"),
                )),
            )
                .into_response();
        }
    };

    // ── Notifications (no `id`) get NO JSON-RPC response. The MCP Streamable
    // HTTP transport expects 202 Accepted with an empty body. Replying with a
    // JSON-RPC error here (e.g. "method not found" for notifications/initialized)
    // breaks strict clients like Codex's rmcp (deserialize fatal on the
    // initialized handshake). See incident: codex MCP worker quit.
    if request.id.is_none() {
        debug!(method = %request.method, "MCP notification (no response)");
        return StatusCode::ACCEPTED.into_response();
    }

    let id = request.id.clone().unwrap_or(Value::Null);

    if request.jsonrpc != "2.0" {
        return (
            StatusCode::OK,
            Json(error_response(
                id,
                INVALID_REQUEST,
                "Invalid JSON-RPC version".into(),
            )),
        )
            .into_response();
    }

    debug!(method = %request.method, "MCP request");

    // ── Route method ──
    let response = match request.method.as_str() {
        "initialize" => handle_initialize(id),
        "tools/list" => handle_tools_list(id, &project_slug, readonly),
        "tools/call" => handle_tools_call(id, request.params, &state, project_slug, readonly).await,
        _ => error_response(
            id,
            METHOD_NOT_FOUND,
            format!("Method not found: {}", request.method),
        ),
    };

    (StatusCode::OK, Json(response)).into_response()
}

// ── Tool definitions ────────────────────────────────────────────────

fn tool_definitions() -> Value {
    let mut tools = tool_definitions_core();
    tools.as_array_mut().unwrap().extend(
        tool_definitions_extended()
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    tools
        .as_array_mut()
        .unwrap()
        .extend(tool_definitions_apps().as_array().unwrap().iter().cloned());
    tools.as_array_mut().unwrap().extend(
        tool_definitions_surveillance()
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    tools
}

/// Global-scope surveillance tools (explicit `slug`). The project-scope
/// variants in `tool_definitions_project()` omit `slug` (auto-injected).
fn tool_definitions_surveillance() -> Value {
    json!([
        { "name": "findings_list", "description": "List surveillance findings across apps (or one app via slug). Filter by kind/category/severity/status.", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "category": { "type": "string" }, "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] }, "status": { "type": "string", "enum": ["open", "dismissed", "resolved"] }, "limit": { "type": "integer", "minimum": 1, "maximum": 1000 } } } },
        { "name": "findings_upsert", "description": "Create/update a finding for an app (dedup by fingerprint). `kind` is the scan: security|code_review|business (default business). `category` is coerced to that kind's allowed set. `summary` = présentation courte (liste) ; `plan` = document de résolution complet (annexe).", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "kind": { "type": "string", "enum": ["security", "code_review", "business"] }, "category": { "type": "string" }, "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] }, "title": { "type": "string" }, "summary": { "type": "string" }, "plan": { "type": "string" }, "fingerprint": { "type": "string" }, "evidence": { "type": "object" } }, "required": ["slug", "category", "severity", "title", "summary", "plan", "fingerprint"] } },
        { "name": "findings_dismiss", "description": "Dismiss a finding as false positive (records dismissed_pattern memory).", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "id": { "type": "integer" }, "reason": { "type": "string" } }, "required": ["slug", "id"] } },
        { "name": "findings_resolve", "description": "Mark a finding resolved (records applied_fix memory).", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "id": { "type": "integer" }, "commit_sha": { "type": "string" } }, "required": ["slug", "id"] } },
        { "name": "surveillance_run", "description": "Trigger a scan run for one of the app's three scans (`kind`: security | code_review | business). Async — findings appear via findings_list once Codex finishes.", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "kind": { "type": "string", "enum": ["security", "code_review", "business"] } }, "required": ["slug", "kind"] } },
        { "name": "memory_get", "description": "Read an app's surveillance memory.", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "kind": { "type": "string" } }, "required": ["slug"] } },
        { "name": "memory_remember", "description": "Store a surveillance memory entry for an app (upsert by kind+key).", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "kind": { "type": "string" }, "key": { "type": "string" }, "value": {} }, "required": ["slug", "kind", "key", "value"] } },
        { "name": "runs_list", "description": "List recent surveillance runs for an app.", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 200 } } } },
        { "name": "pm_query", "description": "Read-only SELECT against an app's database for surveillance forensics (post-mortem). JOINs/aggregates/temporal windows allowed; the `_dv_audit` table holds row mutation history (before/after/diff) for corroborating data freezes and score changes. SELECT-only — any mutation is rejected. Rows returned as JSON.", "inputSchema": { "type": "object", "properties": { "slug": { "type": "string" }, "sql": { "type": "string", "description": "A single SELECT (or read-only WITH) statement." }, "limit": { "type": "integer", "minimum": 1, "maximum": 5000 } }, "required": ["slug", "sql"] } }
    ])
}

fn tool_definitions_core() -> Value {
    json!([
        // ── Git (Atelier owns the bare repos for apps under /var/lib/atelier/git/repos) ──
        {
            "name": "git.repos",
            "description": "List all git repositories managed by Atelier, with size, branch count, and last commit date.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "git.log",
            "description": "Get the last N commits of a git repository.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository slug" },
                    "limit": { "type": "integer", "description": "Number of commits (default 20, max 100)", "default": 20 }
                },
                "required": ["repo"]
            }
        },
        {
            "name": "git.branches",
            "description": "List branches of a git repository.",
            "inputSchema": {
                "type": "object",
                "properties": { "repo": { "type": "string", "description": "Repository slug" } },
                "required": ["repo"]
            }
        },
        {
            "name": "git.activity",
            "description": "Per-day commit counts of a repository over the last N days (GitHub-style contribution timeline).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository slug" },
                    "days": { "type": "integer", "description": "Window in days (default 365, max 1825)", "default": 365 }
                },
                "required": ["repo"]
            }
        },
        {
            "name": "git.show",
            "description": "Full detail of a single commit: metadata, per-file changes (status + lines added/removed) and the unified diff patch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "Repository slug" },
                    "sha": { "type": "string", "description": "Commit SHA (hex, 4-40 chars)" }
                },
                "required": ["repo", "sha"]
            }
        },
    ])
}

fn tool_definitions_extended() -> Value {
    json!([
        // ── Docs (v2: structured by overview/screens/features/components + mermaid) ──
        {
            "name": "docs.overview",
            "description": "DOC-FIRST OBLIGATOIRE. Premier appel à faire avant toute exploration de code dans une app. Renvoie la vue d'ensemble (overview), un index compact de tous les écrans/features/composants (titre + résumé 1 ligne), et des stats. À utiliser pour cadrer la tâche avant tout grep/Read.",
            "inputSchema": {
                "type": "object",
                "properties": { "app_id": { "type": "string" } },
                "required": ["app_id"]
            }
        },
        {
            "name": "docs.list_entries",
            "description": "Liste compacte des entrées de doc d'une app, filtrable par type. Préférer docs.search dès qu'on a un mot-clé — list_entries sert pour explorer une catégorie complète.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["screen", "feature", "component"] }
                },
                "required": ["app_id"]
            }
        },
        {
            "name": "docs.get",
            "description": "Lire une entrée de doc complète (frontmatter + body markdown + diagramme mermaid si présent). Type ∈ {overview, screen, feature, component}. Pour overview, name doit être 'overview'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] },
                    "name": { "type": "string", "description": "Entry name (alphanumeric + - _ .). Use 'overview' for type=overview." }
                },
                "required": ["app_id", "type", "name"]
            }
        },
        {
            "name": "docs.search",
            "description": "Recherche full-text BM25 dans la doc. Filtres optionnels app_id et type. Retourne snippets surlignés et ranking.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                },
                "required": ["query"]
            }
        },
        {
            "name": "docs.list_apps",
            "description": "Liste toutes les apps documentées avec stats de complétude (counts par type, has_overview).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "docs.completeness",
            "description": "Diagnostic de complétude pour une app : has_overview, counts par type, missing_summaries, missing_diagrams.",
            "inputSchema": {
                "type": "object",
                "properties": { "app_id": { "type": "string" } },
                "required": ["app_id"]
            }
        },
        {
            "name": "docs.diagram_get",
            "description": "Récupère le diagramme mermaid attaché à une entrée. Retourne {mermaid: string|null}.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] },
                    "name": { "type": "string" }
                },
                "required": ["app_id", "type", "name"]
            }
        },
        {
            "name": "docs.update",
            "description": "Crée ou met à jour une entrée de doc. Le frontmatter est un objet structuré (title, summary, scope, parent_screen, code_refs[], links[]). Le body est markdown brut. Pour features : scope ∈ {global, screen:<name>}. Stamp updated_at automatique.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] },
                    "name": { "type": "string" },
                    "frontmatter": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "summary": { "type": "string", "description": "≤120 chars, affiché dans l'index compact" },
                            "scope": { "type": "string", "description": "Pour features uniquement: 'global' ou 'screen:<name>'" },
                            "parent_screen": { "type": "string" },
                            "code_refs": { "type": "array", "items": { "type": "string" } },
                            "links": { "type": "array", "items": { "type": "string" } }
                        }
                    },
                    "body": { "type": "string" }
                },
                "required": ["app_id", "type", "name", "body"]
            }
        },
        {
            "name": "docs.delete",
            "description": "Supprime une entrée et son diagramme attaché. Refuse de supprimer l'overview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["screen", "feature", "component"] },
                    "name": { "type": "string" }
                },
                "required": ["app_id", "type", "name"]
            }
        },
        {
            "name": "docs.diagram_set",
            "description": "Attache ou met à jour un diagramme mermaid à une entrée. Taille max 32 KB. Bonnes pratiques : flowchart LR/TD, boîtes carrées [Texte], max 12 nœuds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string" },
                    "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] },
                    "name": { "type": "string" },
                    "mermaid": { "type": "string" }
                },
                "required": ["app_id", "type", "name", "mermaid"]
            }
        },
    ])
}

// ── Method handlers ─────────────────────────────────────────────────

fn handle_initialize(id: Value) -> Value {
    success_response(
        id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "hr-orchestrator",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

/// Tools permitted in the read-only surveillance scope (`?scope=surveillance`).
/// Allowed: the surveillance findings/memory/runs surface (writes only to the
/// `atelier_meta` meta-DB — the scan's job), the forensic read `pm_query`, and
/// read-only schema/git/docs/app-status introspection. EXCLUDED: everything that
/// mutates app code, business data, schema, or lifecycle (app.delete/create/
/// build/exec/control, db.execute, db_create_table/drop_table/add_column/…,
/// raw db_query/db_exec).
fn is_readonly_tool(name: &str) -> bool {
    matches!(
        name,
        // Surveillance surface (meta-DB only) + forensic read
        "findings_list" | "findings_upsert" | "findings_dismiss" | "findings_resolve"
            | "surveillance_run" | "memory_get" | "memory_remember" | "runs_list" | "pm_query"
            | "scan_get"
            // Read-only schema/counts (no row data except via pm_query)
            | "db_tables" | "db_schema" | "db_overview" | "db_count_rows" | "db_get_schema"
            | "db.tables" | "db.list_tables" | "db.describe" | "db.describe_table"
            | "db.overview" | "db.count_rows" | "db.get_schema"
            // Git read
            | "git.repos" | "git.log" | "git.branches" | "git.activity" | "git.show"
            | "git_log" | "git_branches"
            // Docs read
            | "docs.overview" | "docs.list_entries" | "docs.get" | "docs.search"
            | "docs.list_apps" | "docs.completeness" | "docs.diagram_get"
            | "docs_overview" | "docs_list_entries" | "docs_get" | "docs_search"
            | "docs_completeness" | "docs_diagram_get"
            // App read-only introspection
            | "app.list" | "app.get" | "app.status" | "status"
    )
}

fn handle_tools_list(id: Value, project_slug: &Option<String>, readonly: bool) -> Value {
    if readonly {
        // Read-only surveillance scope: the global set filtered to the read-only
        // whitelist (findings/memory/runs + pm_query + read-only schema/git/docs).
        let all = tool_definitions();
        let filtered: Vec<Value> = all
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| {
                        t.get("name")
                            .and_then(|n| n.as_str())
                            .map(is_readonly_tool)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        return success_response(id, json!({ "tools": filtered }));
    }
    if project_slug.is_some() {
        // Project-scoped: only app/db/docs/studio/git tools
        success_response(id, json!({ "tools": tool_definitions_project() }))
    } else {
        // Global: all tools (infra + apps)
        success_response(id, json!({ "tools": tool_definitions() }))
    }
}

/// Single source of truth for the simplified tool names exposed when the MCP
/// server is queried with `?project=<slug>`. Any name here MUST also appear
/// (1) in `tool_definitions_project()` with a schema, and (2) as a match arm
/// in `handle_tools_call`. The `project_scoped_tools_are_consistent` test
/// enforces (1); `project_scoped_tools_are_dispatched` enforces (2) by
/// requiring `is_dispatched_project_tool()` to stay in sync with the match.
fn is_project_simplified_tool(name: &str) -> bool {
    matches!(
        name,
        "status" | "start" | "stop" | "restart" | "exec" | "logs"
            | "db_tables" | "db_schema" | "db_query" | "db_exec"
            | "db_overview" | "db_count_rows"
            | "db_get_schema" | "db_sync_schema"
            | "db_create_table" | "db_drop_table"
            | "db_add_column" | "db_remove_column" | "db_create_relation"
            | "docs_overview" | "docs_list_entries" | "docs_get" | "docs_search"
            | "docs_completeness" | "docs_diagram_get"
            | "docs_update" | "docs_delete" | "docs_diagram_set"
            | "git_log" | "git_branches"
            | "findings_list" | "findings_upsert" | "findings_dismiss"
            | "findings_resolve" | "surveillance_run"
            | "memory_get" | "memory_remember" | "runs_list" | "scan_get" | "scan_set"
    )
}

/// Returns true iff `name` has a corresponding dispatch arm in the
/// project-scope block of `handle_tools_call`. MUST be kept in sync with that
/// match — the `project_scoped_tools_are_dispatched` test enforces parity
/// against `tool_definitions_project()`. A drift surfaces at runtime as
/// "Tool not found" (e.g. `db_count_rows` before this guard was added).
#[cfg(test)]
fn is_dispatched_project_tool(name: &str) -> bool {
    matches!(
        name,
        "status" | "start" | "stop" | "restart" | "exec" | "logs"
            | "db_tables" | "db_schema" | "db_query" | "db_exec"
            | "db_overview" | "db_count_rows"
            | "db_get_schema" | "db_sync_schema"
            | "db_create_table" | "db_drop_table"
            | "db_add_column" | "db_remove_column" | "db_create_relation"
            | "docs_overview" | "docs_list_entries" | "docs_get" | "docs_search"
            | "docs_completeness" | "docs_diagram_get"
            | "docs_update" | "docs_delete" | "docs_diagram_set"
            | "git_log" | "git_branches"
            | "findings_list" | "findings_upsert" | "findings_dismiss"
            | "findings_resolve" | "surveillance_run"
            | "memory_get" | "memory_remember" | "runs_list" | "scan_get" | "scan_set"
    )
}

fn tool_definitions_project() -> Value {
    json!([
        // ── Process control ──
        { "name": "status", "description": "Get the current process state (running/stopped/crashed, PID, port, uptime, restart count).", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "start", "description": "Start the application process.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "stop", "description": "Stop the application process.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "restart", "description": "Restart the application process (stop + start).", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "exec", "description": "Execute a shell command in the project directory. Do NOT use this to run the build — invoke the `app-build` skill instead (it calls the dedicated HTTP endpoint).", "inputSchema": { "type": "object", "properties": { "command": { "type": "string", "description": "Shell command to execute" }, "timeout_secs": { "type": "integer", "default": 60 } }, "required": ["command"] } },
        { "name": "logs", "description": "Get recent application logs.", "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "default": 100 }, "level": { "type": "string", "description": "Filter by level (info, warn, error)" } } } },
        // ── Database ──
        { "name": "db_tables", "description": "List all tables in the application's postgres-dataverse database.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "db_schema", "description": "Describe a table's schema (columns, types, row count).", "inputSchema": { "type": "object", "properties": { "table": { "type": "string" } }, "required": ["table"] } },
        { "name": "db_query", "description": "Run a SELECT query against the database.", "inputSchema": { "type": "object", "properties": { "sql": { "type": "string" }, "params": { "type": "array", "items": {}, "default": [] } }, "required": ["sql"] } },
        { "name": "db_exec", "description": "Raw SQL mutations are not supported on the postgres-dataverse backend — use REST `/api/dv/{slug}/{table}` or MCP `dv_*` tools.", "inputSchema": { "type": "object", "properties": { "sql": { "type": "string" }, "params": { "type": "array", "items": {}, "default": [] } }, "required": ["sql"] } },
        { "name": "db_overview", "description": "Compact overview of the database: table list with column count + row count for each.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "db_count_rows", "description": "Count rows in a single table.", "inputSchema": { "type": "object", "properties": { "table": { "type": "string" } }, "required": ["table"] } },
        { "name": "db_get_schema", "description": "Return the dataverse schema (tables + columns + relations) as JSON. Read-only.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "db_sync_schema", "description": "Rebuild the dataverse `_dv_tables`/`_dv_columns`/`_dv_relations` metadata by introspecting the live PG schema. Use after manual ALTER TABLE.", "inputSchema": { "type": "object", "properties": {} } },
        // Schema-ops (mutations — confirmation required, NOT in auto-approve).
        { "name": "db_create_table", "description": "Create a dataverse-managed table. Emits the right PG type per `field_type` (NUMERIC for decimal, TIMESTAMPTZ for date_time, JSONB for json, UUID for uuid, etc.) and registers it in `_dv_tables`/`_dv_columns`. Audit columns (id, created_at, updated_at, version, is_deleted, created_by, updated_by, *_kind) are added implicitly — do NOT declare them.", "inputSchema": { "type": "object", "properties": { "definition": { "type": "object", "description": "TableDefinition — { name, slug, columns: [{name, field_type, required?, unique?, default_value?, ...}], id_strategy?: \"bigserial\"|\"uuid\" }" } }, "required": ["definition"] } },
        { "name": "db_drop_table", "description": "Drop a dataverse-managed table (DROP TABLE + remove from `_dv_*` metadata).", "inputSchema": { "type": "object", "properties": { "table": { "type": "string" } }, "required": ["table"] } },
        { "name": "db_add_column", "description": "Add a column to an existing dataverse-managed table. Reserved/audit names (created_by, updated_by, version, etc.) are rejected.", "inputSchema": { "type": "object", "properties": { "table": { "type": "string" }, "column": { "type": "object", "description": "ColumnDefinition — { name, field_type, required?, unique?, default_value? }" } }, "required": ["table", "column"] } },
        { "name": "db_remove_column", "description": "Drop a user-defined column from a dataverse-managed table. Refuses to drop audit/reserved columns.", "inputSchema": { "type": "object", "properties": { "table": { "type": "string" }, "column": { "type": "string" } }, "required": ["table", "column"] } },
        { "name": "db_create_relation", "description": "Declare a Lookup foreign-key relation between two dataverse-managed tables.", "inputSchema": { "type": "object", "properties": { "from_table": { "type": "string" }, "from_column": { "type": "string" }, "to_table": { "type": "string" } }, "required": ["from_table", "from_column", "to_table"] } },
        // ── Documentation (DOC-FIRST OBLIGATOIRE — voir .claude/rules/docs.md) ──
        { "name": "docs_overview", "description": "DOC-FIRST OBLIGATOIRE. Premier appel à faire avant toute exploration de code. Renvoie l'overview, l'index compact (écrans/features/composants avec titre+résumé 1 ligne) et les stats de l'app courante.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "docs_list_entries", "description": "Liste compacte des entrées de doc, filtrable par type. Préférer docs_search si on a un mot-clé.", "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["screen", "feature", "component"] } } } },
        { "name": "docs_get", "description": "Lire une entrée complète (frontmatter + body markdown + diagramme mermaid si présent).", "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] }, "name": { "type": "string", "description": "Use 'overview' for type=overview." } }, "required": ["type", "name"] } },
        { "name": "docs_search", "description": "Recherche full-text BM25 dans la doc de l'app. Filtre optionnel par type. Retourne snippets surlignés et ranking.", "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] }, "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }, "required": ["query"] } },
        { "name": "docs_completeness", "description": "Diagnostic : has_overview, counts par type, missing_summaries, missing_diagrams.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "docs_diagram_get", "description": "Récupère le diagramme mermaid attaché à une entrée.", "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] }, "name": { "type": "string" } }, "required": ["type", "name"] } },
        { "name": "docs_update", "description": "Crée/met à jour une entrée. Frontmatter structuré (title, summary≤120, scope=global|screen:<name>, parent_screen, code_refs, links). Body markdown brut.", "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] }, "name": { "type": "string" }, "frontmatter": { "type": "object", "properties": { "title": { "type": "string" }, "summary": { "type": "string" }, "scope": { "type": "string" }, "parent_screen": { "type": "string" }, "code_refs": { "type": "array", "items": { "type": "string" } }, "links": { "type": "array", "items": { "type": "string" } } } }, "body": { "type": "string" } }, "required": ["type", "name", "body"] } },
        { "name": "docs_delete", "description": "Supprime une entrée (refuse l'overview).", "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["screen", "feature", "component"] }, "name": { "type": "string" } }, "required": ["type", "name"] } },
        { "name": "docs_diagram_set", "description": "Attache un diagramme mermaid à une entrée. Bonnes pratiques : flowchart LR/TD, boîtes [Texte], max 12 nœuds.", "inputSchema": { "type": "object", "properties": { "type": { "type": "string", "enum": ["overview", "screen", "feature", "component"] }, "name": { "type": "string" }, "mermaid": { "type": "string" } }, "required": ["type", "name", "mermaid"] } },
        // ── Git ──
        { "name": "git_log", "description": "Get recent git commit history.", "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "default": 20 } } } },
        { "name": "git_branches", "description": "List git branches.", "inputSchema": { "type": "object", "properties": {} } },
        // ── Surveillance IA (3 scans : security, code_review, business + mémoire) ──
        { "name": "findings_list", "description": "List the app's surveillance findings across its three scans (security, code_review, business). Filter by kind/category/severity/status. Read this at session start to triage open issues.", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["security", "code_review", "business"] }, "category": { "type": "string" }, "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] }, "status": { "type": "string", "enum": ["open", "dismissed", "resolved"] }, "limit": { "type": "integer", "minimum": 1, "maximum": 1000 } } } },
        { "name": "findings_upsert", "description": "Create or update a finding (dedup by fingerprint). `kind` is the scan: security|code_review|business (default business). `category` MUST be one of that kind's allowed categories (anything else → 'autres'). `summary` = présentation courte (affichée dans la liste) ; `plan` = document de résolution complet (annexe : ## Contexte / ## Cause racine / ## Fichiers impactés / ## Étapes / ## Validation). Do NOT inflate severity.", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["security", "code_review", "business"] }, "category": { "type": "string", "description": "One of the kind's allowed categories; anything else is coerced to 'autres'." }, "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] }, "title": { "type": "string", "description": "≤120 chars" }, "summary": { "type": "string", "description": "présentation courte de l'issue (markdown)" }, "plan": { "type": "string", "description": "document de résolution complet (markdown)" }, "fingerprint": { "type": "string", "description": "stable hash of the issue for dedup" }, "evidence": { "type": "object", "description": "{file_path?, diff?, ...}" } }, "required": ["category", "severity", "title", "summary", "plan", "fingerprint"] } },
        { "name": "findings_dismiss", "description": "Dismiss a finding as a false positive. Records a dismissed_pattern in memory so future runs skip it. Use when the user (or you) judge the finding irrelevant.", "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" }, "reason": { "type": "string" } }, "required": ["id"] } },
        { "name": "findings_resolve", "description": "Mark a finding as resolved after applying its fix. Records an applied_fix in memory. Pass the commit_sha if you committed the fix (convention: `fix(surveillance:<id>): ...`).", "inputSchema": { "type": "object", "properties": { "id": { "type": "integer" }, "commit_sha": { "type": "string" } }, "required": ["id"] } },
        { "name": "surveillance_run", "description": "Trigger a scan run for one of this app's three scans (`kind`: security | code_review | business). business is skipped if blank or nothing is fresh. Async — findings appear via findings_list once Codex finishes.", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["security", "code_review", "business"] } }, "required": ["kind"] } },
        { "name": "memory_get", "description": "Read the app's surveillance memory (user_preference, dismissed_pattern, applied_fix, recurring_issue). Codex reads this at run start to avoid re-suggesting dismissed/applied items and to respect user preferences.", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["dismissed_pattern", "recurring_issue", "user_preference", "last_run", "applied_fix", "notified"] } } } },
        { "name": "memory_remember", "description": "Store a surveillance memory entry. Use kind=user_preference to record a durable preference (e.g. key='no_new_deps', value='user prefers native code'). Upserts by (kind, key).", "inputSchema": { "type": "object", "properties": { "kind": { "type": "string", "enum": ["dismissed_pattern", "recurring_issue", "user_preference", "applied_fix"] }, "key": { "type": "string" }, "value": {} }, "required": ["kind", "key", "value"] } },
        { "name": "runs_list", "description": "List recent surveillance runs for this app (status, skip_reason, findings_count, tokens). Use to debug why a cron produced nothing.", "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 200 } } } },
        { "name": "scan_get", "description": "Read this app's BUSINESS scan definition (its label, prompt, cadence, gate, gate_sql, categories). `blank=true` means no business scan is defined yet. The security & code_review scans are fixed platform scans (not editable here). Read this at session start before maintaining your business scan.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "scan_set", "description": "Create/replace this app's BUSINESS scan — the ONLY scan you own (no human approval). It targets your app's runtime data & business behaviour; design it for THIS app (no generic template). The prompt is Codex's instructions, run read-only; include the placeholders {{SLUG}} {{STACK}} {{CATEGORIES}} {{DIFF}} {{MEMORY}} {{MAX_OPEN}} {{OPEN_COUNT}} {{REMAINING}}, and tell Codex to emit findings via findings_upsert(kind='business', category, severity, title, summary, plan). Maintain it as the project evolves. (security & code_review are fixed platform scans — not set here.)", "inputSchema": { "type": "object", "properties": { "label": { "type": "string", "description": "short UI name for your scan" }, "prompt": { "type": "string", "description": "the scan's full Codex instructions (with the {{…}} slots)" }, "cadence": { "type": "string", "description": "manual | daily | weekly" }, "gate": { "type": "string", "enum": ["code", "data", "manual"], "description": "code=re-run on git change; data=re-run on new data (needs gate_sql); manual=always" }, "gate_sql": { "type": "string", "description": "read-only SELECT returning ONE scalar high-water mark, tailored to YOUR app's schema (required when gate=data). E.g. SELECT max(<colonne_horodatage>)::text AS w FROM <ta_table>" }, "categories": { "type": "array", "items": { "type": "string" }, "description": "finding buckets (snake_case); 'autres' is added automatically" } }, "required": ["label", "prompt", "cadence", "gate", "categories"] } }
    ])
}

async fn handle_tools_call(
    id: Value,
    params: Value,
    state: &McpState,
    project_slug: Option<String>,
    readonly: bool,
) -> Value {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Read-only surveillance scope: reject any tool outside the read-only
    // whitelist by CAPABILITY (the surveillance Codex cannot mutate app code,
    // data, schema or lifecycle even though it holds the global MCP token).
    if readonly && !is_readonly_tool(tool_name) {
        return tool_error(
            id,
            &format!("tool '{tool_name}' is not permitted in read-only surveillance scope"),
        );
    }

    // Pre-contextualize: inject project slug into tools that need it
    if let Some(ref slug) = project_slug {
        let needs_slug = tool_name.starts_with("db.") || tool_name.starts_with("docs.") || tool_name.starts_with("flow.") || matches!(
            tool_name,
            "app.status" | "app.control" | "app.logs" | "app.exec" | "app.get" |
            "app.health" | "app.regenerate_context" | "app.delete" | "app.build" |
            "app.update" |
            "git.log" | "git.branches" |
            "studio.refresh_context" |
            "secrets.list" | "secrets.get" | "secrets.set" | "secrets.delete"
        ) || is_project_simplified_tool(tool_name);
        if needs_slug {
            if arguments.get("slug").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                arguments["slug"] = json!(slug);
            }
            if arguments.get("app_id").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                arguments["app_id"] = json!(slug);
            }
            if arguments.get("repo").and_then(|v| v.as_str()).unwrap_or("").is_empty() && tool_name.starts_with("git.") {
                arguments["repo"] = json!(slug);
            }
        }
    }

    info!(tool = tool_name, project = ?project_slug, "MCP tools/call");

    match tool_name {
        // ── Git ──
        "git.repos" => tool_git_repos(id, state).await,
        "git.log" => tool_git_log(id, &arguments, state).await,
        "git.branches" => tool_git_branches(id, &arguments, state).await,
        "git.activity" => tool_git_activity(id, &arguments, state).await,
        "git.show" => tool_git_show(id, &arguments, state).await,
        // ── Docs (v2) ──
        "docs.overview" => tool_docs_overview(id, &arguments, state).await,
        "docs.list_entries" => tool_docs_list_entries(id, &arguments, state).await,
        "docs.get" => tool_docs_get(id, &arguments, state).await,
        "docs.search" => tool_docs_search(id, &arguments, state).await,
        "docs.list_apps" => tool_docs_list_apps(id, state).await,
        "docs.completeness" => tool_docs_completeness(id, &arguments, state).await,
        "docs.diagram_get" => tool_docs_diagram_get(id, &arguments, state).await,
        "docs.update" => tool_docs_update(id, &arguments, state).await,
        "docs.delete" => tool_docs_delete(id, &arguments, state).await,
        "docs.diagram_set" => tool_docs_diagram_set(id, &arguments, state).await,
        // ── Database ──
        // ── App* (V3 — atelier-apps direct supervision) ──
        "app.list" => tool_app_list(id, state).await,
        "app.get" => tool_app_get(id, &arguments, state).await,
        "app.control" => tool_app_control(id, &arguments, state).await,
        "app.status" => tool_app_status(id, &arguments, state).await,
        "app.exec" => tool_app_exec(id, &arguments, state).await,
        "app.build" => tool_app_build(id, &arguments, state).await,
        "app.logs" => tool_app_logs(id, &arguments, state).await,
        "app.create" => tool_app_create(id, &arguments, state).await,
        "app.update" => tool_app_update(id, &arguments, state).await,
        "app.delete" => tool_app_delete(id, &arguments, state).await,
        "app.regenerate_context" => tool_app_regenerate_context(id, &arguments, state).await,
        // ── Studio ──
        "studio.refresh_context" => tool_studio_refresh_context(id, &arguments, state).await,
        "studio.refresh_all" => tool_studio_refresh_all(id, state).await,
        // ── DB* (per-app postgres-dataverse) ──
        "db.tables" | "db.list_tables" => tool_db_tables(id, &arguments, state).await,
        "db.describe" | "db.describe_table" => tool_db_describe(id, &arguments, state).await,
        "db.query" | "db.query_data" => tool_db_query(id, &arguments, state).await,
        "db.execute" | "db.insert_data" | "db.update_data" | "db.delete_data" => tool_db_execute(id, &arguments, state).await,
        "db.overview" => tool_db_overview(id, &arguments, state).await,
        "db.count_rows" => tool_db_count_rows(id, &arguments, state).await,
        "db.get_schema" => tool_db_get_schema(id, &arguments, state).await,
        "db.sync_schema" => tool_db_sync_schema(id, &arguments, state).await,
        "db.create_table" => tool_db_create_table(id, &arguments, state).await,
        "db.drop_table" => tool_db_drop_table(id, &arguments, state).await,
        "db.add_column" => tool_db_add_column(id, &arguments, state).await,
        "db.remove_column" => tool_db_remove_column(id, &arguments, state).await,
        "db.create_relation" => tool_db_create_relation(id, &arguments, state).await,
        // ── Project-scoped simplified names (used when ?project=slug) ──
        "status" => tool_app_status(id, &arguments, state).await,
        "start" => {
            let mut a = arguments.clone();
            a["action"] = json!("start");
            tool_app_control(id, &a, state).await
        }
        "stop" => {
            let mut a = arguments.clone();
            a["action"] = json!("stop");
            tool_app_control(id, &a, state).await
        }
        "restart" => {
            let mut a = arguments.clone();
            a["action"] = json!("restart");
            tool_app_control(id, &a, state).await
        }
        "exec" => tool_app_exec(id, &arguments, state).await,
        "logs" => tool_app_logs(id, &arguments, state).await,
        "db_tables" => tool_db_tables(id, &arguments, state).await,
        "db_schema" => tool_db_describe(id, &arguments, state).await,
        "db_query" => tool_db_query(id, &arguments, state).await,
        "db_exec" => tool_db_execute(id, &arguments, state).await,
        "db_get_schema" => tool_db_get_schema(id, &arguments, state).await,
        "db_sync_schema" => tool_db_sync_schema(id, &arguments, state).await,
        "db_create_table" => tool_db_create_table(id, &arguments, state).await,
        "db_drop_table" => tool_db_drop_table(id, &arguments, state).await,
        "db_add_column" => tool_db_add_column(id, &arguments, state).await,
        "db_remove_column" => tool_db_remove_column(id, &arguments, state).await,
        "db_create_relation" => tool_db_create_relation(id, &arguments, state).await,
        "db_overview" => tool_db_overview(id, &arguments, state).await,
        "db_count_rows" => tool_db_count_rows(id, &arguments, state).await,
        "docs_overview" => tool_docs_overview(id, &arguments, state).await,
        "docs_list_entries" => tool_docs_list_entries(id, &arguments, state).await,
        "docs_get" => tool_docs_get(id, &arguments, state).await,
        "docs_search" => tool_docs_search(id, &arguments, state).await,
        "docs_completeness" => tool_docs_completeness(id, &arguments, state).await,
        "docs_diagram_get" => tool_docs_diagram_get(id, &arguments, state).await,
        "docs_update" => tool_docs_update(id, &arguments, state).await,
        "docs_delete" => tool_docs_delete(id, &arguments, state).await,
        "docs_diagram_set" => tool_docs_diagram_set(id, &arguments, state).await,
        "git_log" => tool_git_log(id, &arguments, state).await,
        "git_branches" => tool_git_branches(id, &arguments, state).await,

        // ── Surveillance IA ──
        "findings_list" => tool_findings_list(id, &arguments, state).await,
        "findings_upsert" => tool_findings_upsert(id, &arguments, state).await,
        "findings_dismiss" => tool_findings_dismiss(id, &arguments, state).await,
        "findings_resolve" => tool_findings_resolve(id, &arguments, state).await,
        "surveillance_run" => tool_surveillance_run(id, &arguments, state).await,
        "memory_get" => tool_memory_get(id, &arguments, state).await,
        "memory_remember" => tool_memory_remember(id, &arguments, state).await,
        "runs_list" => tool_runs_list(id, &arguments, state).await,
        "pm_query" => tool_pm_query(id, &arguments, state).await,
        "scan_get" => tool_scan_get(id, &arguments, state).await,
        "scan_set" => tool_scan_set(id, &arguments, state).await,
        _ => {
            warn!(tool = tool_name, "Unknown tool");
            error_response(id, METHOD_NOT_FOUND, format!("Tool not found: {tool_name}"))
        }
    }
}

// ── Git tools ───────────────────────────────────────────────────────

async fn tool_git_repos(id: Value, state: &McpState) -> Value {
    match state.git.list_repos().await {
        Ok(repos) => {
            let result: Vec<Value> = repos
                .iter()
                .map(|r| {
                    json!({
                        "slug": r.slug,
                        "size_bytes": r.size_bytes,
                        "head_ref": r.head_ref,
                        "commit_count": r.commit_count,
                        "last_commit": r.last_commit,
                        "branches": r.branches,
                    })
                })
                .collect();
            tool_success(id, json!(result))
        }
        Err(e) => tool_error(id, &format!("Failed to list repos: {e}")),
    }
}

async fn tool_git_log(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(repo) = args.get("repo").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing repo".into());
    };

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(100) as usize;

    match state.git.get_commits(repo, limit).await {
        Ok(commits) => {
            let result: Vec<Value> = commits
                .iter()
                .map(|c| {
                    json!({
                        "hash": c.hash,
                        "author_name": c.author_name,
                        "author_email": c.author_email,
                        "date": c.date,
                        "message": c.message,
                        "additions": c.additions,
                        "deletions": c.deletions,
                        "files_changed": c.files_changed,
                    })
                })
                .collect();
            tool_success(
                id,
                json!({
                    "repo": repo,
                    "commits": result,
                    "count": result.len(),
                }),
            )
        }
        Err(e) => tool_error(id, &format!("Failed to get commits: {e}")),
    }
}

async fn tool_git_branches(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(repo) = args.get("repo").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing repo".into());
    };
    match state.git.get_branches(repo).await {
        Ok(branches) => {
            let data: Vec<Value> = branches
                .iter()
                .map(|b| {
                    json!({
                        "name": b.name,
                        "is_head": b.is_head,
                    })
                })
                .collect();
            tool_success(id, json!({ "branches": data }))
        }
        Err(e) => tool_error(id, &format!("get_branches: {e}")),
    }
}

async fn tool_git_activity(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(repo) = args.get("repo").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing repo".into());
    };
    let days = args
        .get("days")
        .and_then(|v| v.as_u64())
        .unwrap_or(365)
        .clamp(1, 1825) as u32;

    match state.git.get_commit_activity(repo, days).await {
        Ok(activity) => {
            let data: Vec<Value> = activity
                .iter()
                .map(|a| json!({ "date": a.date, "count": a.count }))
                .collect();
            tool_success(id, json!({ "repo": repo, "activity": data }))
        }
        Err(e) => tool_error(id, &format!("get_commit_activity: {e}")),
    }
}

async fn tool_git_show(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(repo) = args.get("repo").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing repo".into());
    };
    let Some(sha) = args.get("sha").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing sha".into());
    };

    match state.git.get_commit_detail(repo, sha).await {
        Ok(c) => {
            let files: Vec<Value> = c
                .files
                .iter()
                .map(|f| {
                    json!({
                        "path": f.path,
                        "old_path": f.old_path,
                        "status": f.status,
                        "additions": f.additions,
                        "deletions": f.deletions,
                    })
                })
                .collect();
            tool_success(
                id,
                json!({
                    "hash": c.hash,
                    "author_name": c.author_name,
                    "author_email": c.author_email,
                    "author_date": c.author_date,
                    "committer_name": c.committer_name,
                    "committer_email": c.committer_email,
                    "committer_date": c.committer_date,
                    "parents": c.parents,
                    "subject": c.subject,
                    "body": c.body,
                    "files": files,
                    "additions": c.additions,
                    "deletions": c.deletions,
                    "patch": c.patch,
                    "truncated": c.truncated,
                }),
            )
        }
        Err(e) => tool_error(id, &format!("get_commit_detail: {e}")),
    }
}

// ── Docs tools (v2: structured by overview/screens/features/components + mermaid) ──

fn docs_store(state: &McpState) -> Store {
    Store::new(&state.docs_dir)
}

fn parse_doc_type(s: &str) -> Option<DocType> {
    DocType::from_str(s)
}

fn entry_to_json(entry: &atelier_docs::DocEntry, diagram: Option<&str>) -> Value {
    json!({
        "app_id": entry.app_id,
        "type": entry.doc_type.as_str(),
        "name": entry.name,
        "frontmatter": entry.frontmatter,
        "body": entry.body,
        "diagram": diagram,
    })
}

async fn tool_docs_overview(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    match docs_store(state).overview(app_id) {
        Ok(ov) => tool_success(id, serde_json::to_value(&ov).unwrap_or(json!({}))),
        Err(atelier_docs::StoreError::AppNotFound(_)) => tool_error(id, &format!("No docs found for '{app_id}'")),
        Err(e) => tool_error(id, &format!("overview failed: {e}")),
    }
}

async fn tool_docs_list_entries(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let doc_type = match args.get("type").and_then(|v| v.as_str()) {
        None => None,
        Some(s) => match parse_doc_type(s) {
            Some(t) => Some(t),
            None => return tool_error(id, &format!("Invalid type '{s}'")),
        },
    };
    match docs_store(state).list_entries(app_id, doc_type) {
        Ok(entries) => tool_success(id, json!({ "app_id": app_id, "entries": entries })),
        Err(e) => tool_error(id, &format!("list_entries failed: {e}")),
    }
}

async fn tool_docs_get(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    let Some(doc_type_str) = args.get("type").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing type".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let Some(doc_type) = parse_doc_type(doc_type_str) else {
        return tool_error(id, &format!("Invalid type '{doc_type_str}'"));
    };
    let store = docs_store(state);
    match store.read_entry(app_id, doc_type, name) {
        Ok(entry) => {
            let diagram = store.read_diagram(app_id, doc_type, &entry.name).ok().flatten();
            tool_success(id, entry_to_json(&entry, diagram.as_deref()))
        }
        Err(atelier_docs::StoreError::EntryNotFound { .. }) => {
            tool_error(id, &format!("Entry not found: {app_id}/{doc_type_str}/{name}"))
        }
        Err(e) => tool_error(id, &format!("get failed: {e}")),
    }
}

async fn tool_docs_search(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing query".into());
    };
    let app_id = args.get("app_id").and_then(|v| v.as_str());
    if let Some(a) = app_id {
        if !validate_app_id(a) {
            return tool_error(id, "Invalid app_id");
        }
    }
    let doc_type = match args.get("type").and_then(|v| v.as_str()) {
        None => None,
        Some(s) => match parse_doc_type(s) {
            Some(t) => Some(t),
            None => return tool_error(id, &format!("Invalid type '{s}'")),
        },
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).map(|n| n as u32);

    let Some(idx) = state.docs_index.as_ref() else {
        return tool_error(id, "Docs index unavailable (init failed at boot)");
    };
    match idx.search(query, app_id, doc_type, limit).await {
        Ok(hits) => tool_success(
            id,
            json!({ "query": query, "count": hits.len(), "results": hits }),
        ),
        Err(e) => tool_error(id, &format!("search failed: {e}")),
    }
}

async fn tool_docs_list_apps(id: Value, state: &McpState) -> Value {
    let store = docs_store(state);
    let app_ids = match store.list_app_ids() {
        Ok(v) => v,
        Err(e) => return tool_error(id, &format!("list_app_ids failed: {e}")),
    };
    let mut apps = Vec::new();
    for app_id in app_ids {
        let Ok(meta) = store.read_meta(&app_id) else {
            continue;
        };
        let stats = store
            .overview(&app_id)
            .map(|o| o.stats)
            .unwrap_or_default();
        apps.push(json!({
            "app_id": app_id,
            "name": meta.name,
            "schema_version": meta.schema_version,
            "stats": stats,
            "has_overview": stats.has_overview,
        }));
    }
    tool_success(id, json!({ "apps": apps }))
}

async fn tool_docs_completeness(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let store = docs_store(state);
    let overview = match store.overview(app_id) {
        Ok(o) => o,
        Err(atelier_docs::StoreError::AppNotFound(_)) => {
            return tool_error(id, &format!("No docs found for '{app_id}'"));
        }
        Err(e) => return tool_error(id, &format!("completeness failed: {e}")),
    };
    let mut missing_summaries: Vec<String> = Vec::new();
    let mut missing_diagrams: Vec<String> = Vec::new();
    for group in [&overview.index.screens, &overview.index.features, &overview.index.components] {
        for e in group {
            let key = format!("{}:{}", e.doc_type.as_str(), e.name);
            if e.summary.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
                missing_summaries.push(key.clone());
            }
            if !e.has_diagram {
                missing_diagrams.push(key);
            }
        }
    }
    // Orphan links: link points to entry that doesn't exist in the index.
    let mut existing = std::collections::HashSet::new();
    for group in [&overview.index.screens, &overview.index.features, &overview.index.components] {
        for e in group {
            existing.insert(format!("{}:{}", e.doc_type.as_str(), e.name));
        }
    }
    let mut orphan_links: Vec<String> = Vec::new();
    let all_entries = store.list_entries(app_id, None).unwrap_or_default();
    let _ = all_entries; // (kept for potential future per-entry orphan checks)
    if let Some(ov) = overview.overview.as_ref() {
        for link in &ov.frontmatter.links {
            if !existing.contains(link) {
                orphan_links.push(format!("overview→{link}"));
            }
        }
    }
    tool_success(
        id,
        json!({
            "app_id": app_id,
            "has_overview": overview.stats.has_overview,
            "counts": {
                "screens": overview.stats.screens,
                "features": overview.stats.features,
                "components": overview.stats.components,
                "with_diagram": overview.stats.with_diagram,
            },
            "missing_summaries": missing_summaries,
            "missing_diagrams": missing_diagrams,
            "orphan_links": orphan_links,
        }),
    )
}

async fn tool_docs_diagram_get(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    let Some(doc_type_str) = args.get("type").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing type".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let Some(doc_type) = parse_doc_type(doc_type_str) else {
        return tool_error(id, &format!("Invalid type '{doc_type_str}'"));
    };
    match docs_store(state).read_diagram(app_id, doc_type, name) {
        Ok(opt) => tool_success(
            id,
            json!({
                "app_id": app_id,
                "type": doc_type_str,
                "name": name,
                "mermaid": opt,
            }),
        ),
        Err(e) => tool_error(id, &format!("diagram_get failed: {e}")),
    }
}

async fn tool_docs_update(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    let Some(doc_type_str) = args.get("type").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing type".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    let Some(body) = args.get("body").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing body".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let Some(doc_type) = parse_doc_type(doc_type_str) else {
        return tool_error(id, &format!("Invalid type '{doc_type_str}'"));
    };
    if doc_type != DocType::Overview && !validate_entry_name(name) {
        return tool_error(id, "Invalid name");
    }

    // Parse optional frontmatter object.
    let mut frontmatter = match args.get("frontmatter") {
        Some(Value::Object(_)) => {
            match serde_json::from_value::<Frontmatter>(args["frontmatter"].clone()) {
                Ok(fm) => fm,
                Err(e) => return tool_error(id, &format!("Invalid frontmatter: {e}")),
            }
        }
        _ => Frontmatter::default(),
    };

    // Auto-derive parent_screen from scope=screen:<name> if not explicit.
    if doc_type == DocType::Feature {
        if let Some(ref s) = frontmatter.scope {
            if let Some(ps) = s.strip_prefix("screen:") {
                if frontmatter.parent_screen.is_none() && !ps.is_empty() {
                    frontmatter.parent_screen = Some(ps.to_string());
                }
            }
        }
    }

    // Ensure the app's docs dir exists (auto-create if missing — keeps the agent's flow simple).
    let store = docs_store(state);
    let _ = store.ensure_layout(app_id);
    if !store.app_dir(app_id).exists() {
        let _ = std::fs::create_dir_all(store.app_dir(app_id));
    }
    if !store.app_dir(app_id).join("meta.json").exists() {
        let _ = store.write_meta(app_id, &atelier_docs::Meta::new(app_id));
    }

    match store.write_entry(app_id, doc_type, name, frontmatter, body) {
        Ok(entry) => {
            // Sync the search index.
            if let Some(idx) = state.docs_index.as_ref() {
                if let Err(e) = idx.upsert(&entry).await {
                    warn!(error = %e, "Docs index upsert failed");
                }
            }
            info!(app_id, doc_type = doc_type_str, name = %entry.name, "Docs entry updated");
            tool_success(
                id,
                json!({
                    "app_id": app_id,
                    "type": doc_type_str,
                    "name": entry.name,
                    "updated_at": entry.frontmatter.updated_at,
                }),
            )
        }
        Err(e) => tool_error(id, &format!("update failed: {e}")),
    }
}

async fn tool_docs_delete(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    let Some(doc_type_str) = args.get("type").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing type".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let Some(doc_type) = parse_doc_type(doc_type_str) else {
        return tool_error(id, &format!("Invalid type '{doc_type_str}'"));
    };
    if doc_type == DocType::Overview {
        return tool_error(id, "Cannot delete the overview");
    }
    match docs_store(state).delete_entry(app_id, doc_type, name) {
        Ok(deleted) => {
            if deleted {
                if let Some(idx) = state.docs_index.as_ref() {
                    if let Err(e) = idx.remove(app_id, doc_type, name).await {
                        warn!(error = %e, "Docs index remove failed");
                    }
                }
                info!(app_id, doc_type = doc_type_str, name, "Docs entry deleted");
            }
            tool_success(
                id,
                json!({
                    "app_id": app_id,
                    "type": doc_type_str,
                    "name": name,
                    "deleted": deleted,
                }),
            )
        }
        Err(e) => tool_error(id, &format!("delete failed: {e}")),
    }
}

async fn tool_docs_diagram_set(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    let Some(doc_type_str) = args.get("type").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing type".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    let Some(mermaid) = args.get("mermaid").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing mermaid".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let Some(doc_type) = parse_doc_type(doc_type_str) else {
        return tool_error(id, &format!("Invalid type '{doc_type_str}'"));
    };
    let store = docs_store(state);
    if let Err(e) = store.write_diagram(app_id, doc_type, name, mermaid) {
        return tool_error(id, &format!("diagram_set failed: {e}"));
    }
    // The diagram flag is now true; re-index the entry so search reflects it.
    if let Some(idx) = state.docs_index.as_ref() {
        if let Ok(entry) = store.read_entry(app_id, doc_type, name) {
            if let Err(e) = idx.upsert(&entry).await {
                warn!(error = %e, "Docs index upsert failed after diagram set");
            }
        }
    }
    info!(app_id, doc_type = doc_type_str, name, bytes = mermaid.len(), "Docs diagram set");
    tool_success(
        id,
        json!({
            "app_id": app_id,
            "type": doc_type_str,
            "name": name,
            "ok": true,
        }),
    )
}


// (db tools removed -- now managed per-environment by env-agent)
// (db tools removed -- now managed per-environment by env-agent)

fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i32, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool_success(id: Value, data: Value) -> Value {
    success_response(
        id,
        json!({
            "content": [{ "type": "text", "text": data.to_string() }]
        }),
    )
}

fn tool_error(id: Value, message: &str) -> Value {
    success_response(
        id,
        json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }),
    )
}

// ── App* / DB* tool definitions (V3 — atelier-apps) ──────────────────────

fn tool_definitions_apps() -> Value {
    json!([
        {
            "name": "app.list",
            "description": "List all Atelier applications managed by the AppSupervisor.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "app.get",
            "description": "Get details for a single application by slug.",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "app.create",
            "description": "Create a new application (assigns port, git repo, edge route).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "name": { "type": "string" },
                    "stack": { "type": "string", "enum": ["next-js", "axum-vite", "axum"] },
                    "visibility": { "type": "string", "enum": ["public", "private"], "default": "private" },
                    "run_command": { "type": "string" },
                    "build_command": { "type": "string" },
                    "health_path": { "type": "string" },
                    "build_artefact": { "type": "string", "description": "Override artefact path(s) rsynced back after `app.build`. One per line, relative to src/." }
                },
                "required": ["slug", "name", "stack"]
            }
        },
        {
            "name": "app.update",
            "description": "Update an application's registry config (partial: only provided fields change). Does not restart the app.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "name": { "type": "string" },
                    "stack": { "type": "string", "enum": ["next-js", "axum-vite", "axum"] },
                    "visibility": { "type": "string", "enum": ["public", "private"] },
                    "run_command": { "type": "string" },
                    "build_command": { "type": "string" },
                    "health_path": { "type": "string" },
                    "env_vars": { "type": "object", "additionalProperties": { "type": "string" } },
                    "has_db": { "type": "boolean" },
                    "build_artefact": { "type": "string", "description": "Override artefact path(s) rsynced back after `app.build`. One per line, relative to src/." }
                },
                "required": ["slug"]
            }
        },
        {
            "name": "app.control",
            "description": "Control an application process: start, stop, or restart.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "action": { "type": "string", "enum": ["start", "stop", "restart"] }
                },
                "required": ["slug", "action"]
            }
        },
        {
            "name": "app.status",
            "description": "Get runtime status of an application (pid, state, port, uptime).",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "app.exec",
            "description": "Execute a shell command in the context of an application.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "integer", "default": 60 }
                },
                "required": ["slug", "command"]
            }
        },
        {
            "name": "app.build",
            "description": "Build an app remotely on the configured build host (ATELIER_BUILD_HOST; rsync src up, build, rsync artefacts down). Synchronous; bounded by `timeout_secs` (default 1800 = 30 min). Stacks: axum, axum-vite, next-js. Returns AppExecResult (stdout/stderr/exit_code/duration_ms).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "timeout_secs": { "type": "integer", "default": 1800 }
                },
                "required": ["slug"]
            }
        },
        {
            "name": "app.logs",
            "description": "Get recent logs for an application.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "limit": { "type": "integer", "default": 100 },
                    "level": { "type": "string" }
                },
                "required": ["slug"]
            }
        },
        {
            "name": "app.delete",
            "description": "Delete an application. Set keep_data=true to preserve source and DB.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "keep_data": { "type": "boolean", "default": false }
                },
                "required": ["slug"]
            }
        },
        {
            "name": "app.regenerate_context",
            "description": "Regenerate Claude context files (CLAUDE.md, .claude/) for an app.",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "db.tables",
            "description": "List user-defined tables in an app's postgres-dataverse database.",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "db.describe",
            "description": "Describe a table's schema (columns, types, row count).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "table": { "type": "string" }
                },
                "required": ["slug", "table"]
            }
        },
        {
            "name": "db.query",
            "description": "Raw SQL is not supported on postgres-dataverse — use `dv_list` or REST `/api/dv/{slug}/{table}`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "sql": { "type": "string" },
                    "params": { "type": "array", "items": {}, "default": [] }
                },
                "required": ["slug", "sql"]
            }
        },
        {
            "name": "db.execute",
            "description": "Raw SQL mutations are not supported on postgres-dataverse — use db.insert/db.update/db.delete or `dv_*` tools.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "sql": { "type": "string" },
                    "params": { "type": "array", "items": {}, "default": [] }
                },
                "required": ["slug", "sql"]
            }
        },
        {
            "name": "db.overview",
            "description": "Get an overview of an app's database (table count and list).",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "db.count_rows",
            "description": "Count rows in a specific table of an app's database.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "table": { "type": "string" }
                },
                "required": ["slug", "table"]
            }
        },
        {
            "name": "db.get_schema",
            "description": "Get the full database schema (all tables, columns, and relations).",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "db.sync_schema",
            "description": "No-op on postgres-dataverse: the `_dv_*` metadata is already the source of truth.",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "db.create_table",
            "description": "Create a new table. Columns id, created_at, updated_at are added automatically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "definition": {
                        "type": "object",
                        "description": "Table definition with name (string) and columns (array of {name, field_type, required?, unique?, default_value?, description?})"
                    }
                },
                "required": ["slug", "definition"]
            }
        },
        {
            "name": "db.drop_table",
            "description": "Drop a table from the database.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "table": { "type": "string" }
                },
                "required": ["slug", "table"]
            }
        },
        {
            "name": "db.add_column",
            "description": "Add a column to an existing table.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "table": { "type": "string" },
                    "column": {
                        "type": "object",
                        "description": "Column definition with name, field_type, required?, unique?, default_value?, description?"
                    }
                },
                "required": ["slug", "table", "column"]
            }
        },
        {
            "name": "db.remove_column",
            "description": "Remove a column from a table.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "table": { "type": "string" },
                    "column": { "type": "string", "description": "Column name to remove" }
                },
                "required": ["slug", "table", "column"]
            }
        },
        {
            "name": "db.create_relation",
            "description": "Create a foreign key relation between two tables.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "relation": {
                        "type": "object",
                        "description": "Relation with from_table, from_column, to_table, to_column, relation_type (one_to_many|many_to_many|self_referential), cascade? ({on_delete, on_update}: cascade|set_null|restrict)"
                    }
                },
                "required": ["slug", "relation"]
            }
        },
        {
            "name": "studio.refresh_context",
            "description": "Regenerate Claude Code context files (CLAUDE.md, .claude/) for a specific app.",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "studio.refresh_all",
            "description": "Regenerate Claude Code context files for all apps.",
            "inputSchema": { "type": "object", "properties": {} }
        },
    ])
}

// ── App* tool handlers ──────────────────────────────────────────────

fn require_apps_ctx<'a>(
    id: &Value,
    state: &'a McpState,
) -> Result<&'a crate::mcp::apps_ops::AppsContext, Value> {
    state
        .apps_ctx
        .as_ref()
        .ok_or_else(|| tool_error(id.clone(), "atelier-apps not initialized"))
}

async fn tool_app_list(id: Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let resp = ctx.list().await;
    ipc_resp_to_mcp(id, resp)
}

async fn tool_app_get(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.get(slug).await)
}

async fn tool_app_create(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    let Some(stack) = args.get("stack").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing stack".into());
    };
    let visibility = args
        .get("visibility")
        .and_then(|v| v.as_str())
        .unwrap_or("private");
    let run_command = args
        .get("run_command")
        .and_then(|v| v.as_str())
        .map(String::from);
    let build_command = args
        .get("build_command")
        .and_then(|v| v.as_str())
        .map(String::from);
    let health_path = args
        .get("health_path")
        .and_then(|v| v.as_str())
        .map(String::from);
    let build_artefact = args
        .get("build_artefact")
        .and_then(|v| v.as_str())
        .map(String::from);
    ipc_resp_to_mcp(
        id,
        ctx.create(
            slug.to_string(),
            name.to_string(),
            stack.to_string(),
            true,
            visibility.to_string(),
            run_command,
            build_command,
            health_path,
            build_artefact,
        )
        .await,
    )
}

async fn tool_app_update(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let opt_str = |key: &str| args.get(key).and_then(|v| v.as_str()).map(String::from);
    let env_vars = args.get("env_vars").and_then(|v| v.as_object()).map(|m| {
        m.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect()
    });
    let has_db = args.get("has_db").and_then(|v| v.as_bool());
    ipc_resp_to_mcp(
        id,
        ctx.update(
            slug.to_string(),
            opt_str("name"),
            opt_str("stack"),
            opt_str("visibility"),
            opt_str("run_command"),
            opt_str("build_command"),
            opt_str("health_path"),
            env_vars,
            has_db,
            opt_str("build_artefact"),
        )
        .await,
    )
}

async fn tool_app_build(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());
    ipc_resp_to_mcp(id, ctx.build(slug.to_string(), timeout_secs).await)
}

async fn tool_app_control(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing action".into());
    };
    ipc_resp_to_mcp(id, ctx.control(slug.to_string(), action.to_string()).await)
}

async fn tool_app_status(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.status(slug).await)
}

async fn tool_app_exec(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing command".into());
    };
    let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());
    ipc_resp_to_mcp(
        id,
        ctx.exec(slug.to_string(), command.to_string(), timeout_secs)
            .await,
    )
}

async fn tool_app_logs(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let level = args.get("level").and_then(|v| v.as_str()).map(String::from);
    ipc_resp_to_mcp(id, ctx.logs(slug.to_string(), limit, level).await)
}

async fn tool_app_delete(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let keep_data = args
        .get("keep_data")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ipc_resp_to_mcp(id, ctx.delete(slug.to_string(), keep_data).await)
}

async fn tool_app_regenerate_context(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.regenerate_context(slug.to_string()).await)
}

// ── DB tool handlers ────────────────────────────────────────────────

async fn tool_db_tables(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.db_list_tables(slug.to_string()).await)
}

async fn tool_db_describe(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(table) = args.get("table").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing table".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_describe_table(slug.to_string(), table.to_string())
            .await,
    )
}

async fn tool_db_query(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(sql) = args.get("sql").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing sql".into());
    };
    let params: Vec<Value> = args
        .get("params")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    ipc_resp_to_mcp(
        id,
        ctx.db_query(slug.to_string(), sql.to_string(), params)
            .await,
    )
}

/// Surveillance forensic read — SELECT-only against an app's DB. Lives in the
/// surveillance MCP tool set (NOT the project set), so the read-only post-mortem
/// scanner can do cross-table correlation + `_dv_audit` freeze/gap detection
/// that the gateway `dv_*` tools can't. Read-only is enforced inside `pm_query`.
async fn tool_pm_query(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(sql) = args.get("sql").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing sql".into());
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(1000) as u32;
    ipc_resp_to_mcp(
        id,
        crate::mcp::dv_ops::pm_query(ctx, slug.to_string(), sql.to_string(), limit).await,
    )
}

/// Convert an `IpcResponse` into a JSON-RPC response Value.
fn ipc_resp_to_mcp(id: Value, resp: atelier_ipc::types::IpcResponse) -> Value {
    if resp.ok {
        tool_success(id, resp.data.unwrap_or(json!({"ok": true})))
    } else {
        tool_error(id, resp.error.as_deref().unwrap_or("unknown error"))
    }
}

// ── db.execute (mutations: INSERT/UPDATE/DELETE) ──────────────────

async fn tool_db_execute(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(sql) = args.get("sql").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing sql".into());
    };
    let params: Vec<Value> = args
        .get("params")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    ipc_resp_to_mcp(id, ctx.db_execute(slug.to_string(), sql.to_string(), params).await)
}

// ── db.overview ──────────────────────────────────────────────────────

async fn tool_db_overview(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    // List tables then describe each
    let tables_resp = ctx.db_list_tables(slug.to_string()).await;
    if !tables_resp.ok {
        return ipc_resp_to_mcp(id, tables_resp);
    }
    let tables = tables_resp
        .data
        .and_then(|d| d.get("tables").cloned())
        .and_then(|t| t.as_array().cloned())
        .unwrap_or_default();
    tool_success(id, json!({
        "slug": slug,
        "tables_count": tables.len(),
        "tables": tables,
    }))
}

// ── db.count_rows ────────────────────────────────────────────────────

async fn tool_db_count_rows(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(table) = args.get("table").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing table".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_count_rows(slug.to_string(), table.to_string()).await,
    )
}

// ── db.get_schema / db.sync_schema ───────────────────────────────────

async fn tool_db_get_schema(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.db_get_schema(slug.to_string()).await)
}

async fn tool_db_sync_schema(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.db_sync_schema(slug.to_string()).await)
}

// ── db.create_table / db.drop_table ──────────────────────────────────

async fn tool_db_create_table(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(definition) = args.get("definition").cloned() else {
        return error_response(id, INVALID_PARAMS, "Missing definition".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_create_table(slug.to_string(), definition).await,
    )
}

async fn tool_db_drop_table(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(table) = args.get("table").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing table".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_drop_table(slug.to_string(), table.to_string()).await,
    )
}

// ── db.add_column / db.remove_column ─────────────────────────────────

async fn tool_db_add_column(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(table) = args.get("table").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing table".into());
    };
    let Some(column) = args.get("column").cloned() else {
        return error_response(id, INVALID_PARAMS, "Missing column".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_add_column(slug.to_string(), table.to_string(), column)
            .await,
    )
}

async fn tool_db_remove_column(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(table) = args.get("table").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing table".into());
    };
    let Some(column) = args.get("column").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing column".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_remove_column(slug.to_string(), table.to_string(), column.to_string())
            .await,
    )
}

// ── db.create_relation ───────────────────────────────────────────────

async fn tool_db_create_relation(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(relation) = args.get("relation").cloned() else {
        return error_response(id, INVALID_PARAMS, "Missing relation".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.db_create_relation(slug.to_string(), relation).await,
    )
}

// db.graphql / db.introspect / db.find / db.migrate / db.commit_migration /
// db.rollback_migration were removed once every app finished its move to
// postgres-dataverse. Agents use MCP `dv_*` tools, apps use REST
// `/api/dv/{slug}/{table}`, flows use the `dataverse` connector — there is
// no GraphQL surface anymore.

// ── studio.refresh_context ───────────────────────────────────────────

async fn tool_studio_refresh_context(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    ipc_resp_to_mcp(id, ctx.regenerate_context(slug.to_string()).await)
}

// ── studio.refresh_all ───────────────────────────────────────────────

async fn tool_studio_refresh_all(id: Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let apps = ctx.supervisor.registry.list().await;
    let mut refreshed = 0u32;
    for app in &apps {
        let _ = ctx.regenerate_context(app.slug.clone()).await;
        refreshed += 1;
    }
    tool_success(id, json!({ "refreshed": refreshed, "total": apps.len() }))
}

// ── Surveillance IA tools ───────────────────────────────────────────

async fn tool_findings_list(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.findings() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let filter = atelier_watcher::FindingFilter {
        slug: args.get("slug").and_then(|v| v.as_str()).map(String::from),
        kind: args.get("kind").and_then(|v| v.as_str()).map(String::from),
        severity: args.get("severity").and_then(|v| v.as_str()).map(String::from),
        status: args.get("status").and_then(|v| v.as_str()).map(String::from),
        category: args.get("category").and_then(|v| v.as_str()).map(String::from),
        limit: args.get("limit").and_then(|v| v.as_i64()),
    };
    match store.list(filter).await {
        Ok(items) => tool_success(id, json!({ "findings": items, "total": items.len() })),
        Err(e) => tool_error(id, &format!("findings_list failed: {e}")),
    }
}

async fn tool_findings_upsert(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.findings() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let (Some(severity), Some(title), Some(summary), Some(plan), Some(fingerprint)) = (
        args.get("severity").and_then(|v| v.as_str()),
        args.get("title").and_then(|v| v.as_str()),
        args.get("summary").and_then(|v| v.as_str()),
        args.get("plan").and_then(|v| v.as_str()),
        args.get("fingerprint").and_then(|v| v.as_str()),
    ) else {
        return error_response(
            id,
            INVALID_PARAMS,
            "Missing one of: severity, title, summary, plan, fingerprint".into(),
        );
    };
    if !matches!(severity, "critical" | "high" | "medium" | "low") {
        return tool_error(id, "severity must be critical|high|medium|low");
    }
    // Which of the three scans this finding belongs to. Defaults to `business`
    // (the agent-owned scan) so business prompts written before `kind` was
    // required keep working.
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(atelier_watcher::BIZ_KIND);
    if !atelier_watcher::is_valid_kind(kind) {
        return tool_error(id, "kind must be security|code_review|business");
    }
    // Coerce the category to the kind's declared set (unknown → "autres"). The
    // categories of the fixed scans come from their constructors; the business
    // scan's come from its `app_scan` row.
    let raw_cat = args.get("category").and_then(|v| v.as_str());
    let category = match atelier_watcher::ScanDef::fixed(kind, slug) {
        Some(scan) => scan.normalize_category(raw_cat),
        None => match state.surveillance.scan_get(slug).await {
            Some(scan) => scan.normalize_category(raw_cat),
            None => raw_cat.unwrap_or("autres").to_string(),
        },
    };
    let draft = atelier_watcher::NewFinding {
        slug: slug.to_string(),
        kind: kind.to_string(),
        severity: severity.to_string(),
        title: title.to_string(),
        summary: summary.to_string(),
        plan: plan.to_string(),
        fingerprint: fingerprint.to_string(),
        category,
        evidence: args.get("evidence").cloned(),
    };
    match store.upsert(draft).await {
        Ok(f) => {
            state.surveillance.emit("finding", &f.slug, "upsert");
            tool_success(id, json!(f))
        }
        Err(e) => tool_error(id, &format!("findings_upsert failed: {e}")),
    }
}

async fn tool_findings_dismiss(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.findings() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let Some(fid) = args.get("id").and_then(|v| v.as_i64()) else {
        return error_response(id, INVALID_PARAMS, "Missing id".into());
    };
    // Record dismissed_pattern memory so future runs skip it.
    let item = match store.get(fid).await {
        Ok(Some(f)) => Some(f),
        Ok(None) => return tool_error(id, "finding not found"),
        Err(e) => return tool_error(id, &format!("findings_dismiss get failed: {e}")),
    };
    match store.dismiss(fid).await {
        Ok(_) => {}
        Err(e) => return tool_error(id, &format!("findings_dismiss failed: {e}")),
    }
    if let (Some(item), Some(memory)) = (item.clone(), state.surveillance.memory()) {
        let value = json!({
            "fingerprint": item.fingerprint,
            "title": item.title,
            "reason": args.get("reason").and_then(|v| v.as_str()),
            "dismissed_at": chrono::Utc::now(),
        });
        let _ = memory
            .upsert(&item.slug, "dismissed_pattern", &item.fingerprint, &value, None)
            .await;
    }
    if let Some(item) = item {
        state.surveillance.emit("finding", &item.slug, "dismiss");
    }
    tool_success(id, json!({ "ok": true }))
}

async fn tool_findings_resolve(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.findings() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let Some(fid) = args.get("id").and_then(|v| v.as_i64()) else {
        return error_response(id, INVALID_PARAMS, "Missing id".into());
    };
    let commit_sha = args.get("commit_sha").and_then(|v| v.as_str());
    let item = match store.get(fid).await {
        Ok(Some(f)) => Some(f),
        Ok(None) => return tool_error(id, "finding not found"),
        Err(e) => return tool_error(id, &format!("findings_resolve get failed: {e}")),
    };
    match store.resolve(fid, commit_sha).await {
        Ok(_) => {}
        Err(e) => return tool_error(id, &format!("findings_resolve failed: {e}")),
    }
    if let (Some(item), Some(memory)) = (item.clone(), state.surveillance.memory()) {
        let value = json!({
            "finding_id": fid,
            "title": item.title,
            "commit_sha": commit_sha,
            "completed_at": chrono::Utc::now(),
        });
        let key = format!("finding:{fid}");
        let _ = memory.upsert(&item.slug, "applied_fix", &key, &value, None).await;
    }
    if let Some(item) = item {
        state.surveillance.emit("finding", &item.slug, "resolve");
    }
    tool_success(id, json!({ "ok": true }))
}

async fn tool_surveillance_run(id: Value, args: &Value, state: &McpState) -> Value {
    if state.surveillance.findings().is_none() {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    }
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(kind) = args.get("kind").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing kind".into());
    };
    if !atelier_watcher::is_valid_kind(kind) {
        return tool_error(id, "kind must be security|code_review|business");
    }
    // MCP-triggered runs are manual/debug; the scheduled cadence (with the data
    // gate watermark) goes through the REST endpoint. Pass no watermark → a
    // data-gated business scan runs unconditionally here.
    match state
        .surveillance
        .run_now(slug.to_string(), kind, "manual", None)
        .await
    {
        Ok(run_id) => tool_success(id, json!({ "ok": true, "run_id": run_id, "status": "running" })),
        Err(e) => tool_error(id, &format!("surveillance_run failed: {e}")),
    }
}

/// Read the app's business scan definition (the agent-owned scan).
async fn tool_scan_get(id: Value, args: &Value, state: &McpState) -> Value {
    if state.surveillance.findings().is_none() {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    }
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let scan = state.surveillance.scan_get(slug).await;
    let blank = scan.as_ref().map(|s| s.is_blank()).unwrap_or(true);
    tool_success(id, json!({ "scan": scan, "blank": blank }))
}

/// Create/replace the app's single scan (agent-owned, no approval).
async fn tool_scan_set(id: Value, args: &Value, state: &McpState) -> Value {
    if state.surveillance.findings().is_none() {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    }
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let f = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let (label, prompt, cadence, gate) = (f("label"), f("prompt"), f("cadence"), f("gate"));
    if label.trim().is_empty() || prompt.trim().is_empty() {
        return tool_error(id, "label and prompt are required");
    }
    let cadence = if cadence.is_empty() { "manual" } else { cadence };
    let gate = if gate.is_empty() { "code" } else { gate };
    let gate_sql = args.get("gate_sql").and_then(|v| v.as_str());
    let categories: Vec<String> = args
        .get("categories")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    match state
        .surveillance
        .scan_set(
            slug,
            label,
            prompt,
            cadence,
            gate,
            gate_sql,
            &categories,
            &format!("agent:{slug}"),
        )
        .await
    {
        Ok(()) => tool_success(id, json!({ "ok": true, "slug": slug, "label": label })),
        Err(e) => tool_error(id, &e),
    }
}

async fn tool_memory_get(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.memory() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let kind = args.get("kind").and_then(|v| v.as_str());
    match store.get(slug, kind, None).await {
        Ok(items) => tool_success(id, json!({ "memory": items, "total": items.len() })),
        Err(e) => tool_error(id, &format!("memory_get failed: {e}")),
    }
}

async fn tool_memory_remember(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.memory() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(
        kind,
        "dismissed_pattern" | "recurring_issue" | "user_preference" | "applied_fix"
    ) {
        return tool_error(
            id,
            "kind must be dismissed_pattern|recurring_issue|user_preference|applied_fix",
        );
    }
    let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing key".into());
    };
    let value = match args.get("value") {
        Some(v) => v.clone(),
        None => return error_response(id, INVALID_PARAMS, "Missing value".into()),
    };
    match store.upsert(slug, kind, key, &value, None).await {
        Ok(_) => tool_success(id, json!({ "ok": true })),
        Err(e) => tool_error(id, &format!("memory_remember failed: {e}")),
    }
}

async fn tool_runs_list(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(store) = state.surveillance.runs() else {
        return tool_error(id, "surveillance disabled (postgres unreachable)");
    };
    let slug = args.get("slug").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    match store.list(slug, limit).await {
        Ok(items) => tool_success(id, json!({ "runs": items, "total": items.len() })),
        Err(e) => tool_error(id, &format!("runs_list failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertised_project_tool_names() -> Vec<String> {
        let defs = tool_definitions_project();
        defs.as_array()
            .expect("tool_definitions_project must be an array")
            .iter()
            .map(|t| {
                t.get("name")
                    .and_then(|n| n.as_str())
                    .expect("every tool definition has a name")
                    .to_string()
            })
            .collect()
    }

    /// Guarantees parity between `tool_definitions_project()` (what clients
    /// discover) and `is_project_simplified_tool()` (what the dispatcher
    /// treats as project-scoped and injects the slug into). If these drift,
    /// a client sees a tool it cannot call (or calls one without a slug).
    #[test]
    fn project_scoped_tools_are_consistent() {
        for name in &advertised_project_tool_names() {
            assert!(
                is_project_simplified_tool(name),
                "tool `{name}` is advertised by tool_definitions_project() but \
                 is_project_simplified_tool() does not recognize it. Add it \
                 there AND add a match arm in handle_tools_call."
            );
        }
    }

    /// Guarantees every advertised project-scoped tool has a corresponding
    /// match arm in `handle_tools_call`. Catches the failure mode where a tool
    /// is added to `is_project_simplified_tool()` and `tool_definitions_project()`
    /// but the dispatcher's match falls through to the catchall, returning
    /// "Tool not found" to the client (regression seen with `db_count_rows`
    /// and `db_overview`). `is_dispatched_project_tool()` must mirror the
    /// match arms below.
    #[test]
    fn project_scoped_tools_are_dispatched() {
        for name in &advertised_project_tool_names() {
            assert!(
                is_dispatched_project_tool(name),
                "tool `{name}` is advertised by tool_definitions_project() but \
                 has no match arm in handle_tools_call (project-scope block). \
                 Add the arm AND mirror the name in is_dispatched_project_tool()."
            );
        }
    }
}
