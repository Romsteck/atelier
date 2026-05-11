//! MCP (Model Context Protocol) HTTP endpoint for Atelier.
//!
//! Ported verbatim from homeroute's `hr-orchestrator::mcp` after the Atelier
//! cutover. Adapted for Atelier's `ApiState`. The homeroute-only infra tools
//! (hosts.*, monitoring.*, reverseproxy.*, store.*) were removed — they
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
use hr_docs::{DocType, Frontmatter, Store, validate_app_id, validate_entry_name};
use serde::Deserialize;
use serde_json::{Value, json};
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
    pub git: Arc<hr_git::GitService>,
    pub apps_ctx: Option<crate::mcp::apps_ops::AppsContext>,
    /// FTS5 index for `docs.search`. None if FTS init failed at boot.
    pub docs_index: Option<Arc<hr_docs::Index>>,
}

impl McpState {
    /// Build the MCP state from the Atelier `ApiState` + the `MCP_TOKEN`
    /// env var. Returns `None` if no token is set (MCP is then disabled).
    pub fn from_api_state(state: &ApiState) -> Option<Self> {
        let token = std::env::var("MCP_TOKEN").ok()?;
        if token.is_empty() {
            return None;
        }
        // hr-edge socket vit sur Medion. Atelier (CloudMaster) ne peut pas
        // l'atteindre depuis ici → `edge: None` ; les call sites set/remove
        // route s'auto-skip avec un warn.
        let edge = None;
        let (app_build_tx, _) = tokio::sync::broadcast::channel(256);
        let apps_ctx = crate::mcp::apps_ops::AppsContext {
            supervisor: (*state.supervisor).clone(),
            db_manager: (*state.db_manager).clone(),
            dataverse_manager: state.dv.clone(),
            todos: (*state.todos_manager).clone(),
            context_generator: state.context_generator.clone(),
            edge,
            git: state.git.clone(),
            base_domain: state.context_generator.base_domain.clone(),
            build_locks: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            app_build_tx,
        };
        Some(Self {
            token: Arc::new(token),
            git: state.git.clone(),
            apps_ctx: Some(apps_ctx),
            docs_index: state.docs_index.clone(),
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
) -> impl IntoResponse {
    let project_slug = query.get("project").cloned();
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
        );
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
            );
        }
    };

    let id = request.id.clone().unwrap_or(Value::Null);

    if request.jsonrpc != "2.0" {
        return (
            StatusCode::OK,
            Json(error_response(
                id,
                INVALID_REQUEST,
                "Invalid JSON-RPC version".into(),
            )),
        );
    }

    debug!(method = %request.method, "MCP request");

    // ── Route method ──
    let response = match request.method.as_str() {
        "initialize" => handle_initialize(id),
        "tools/list" => handle_tools_list(id, &project_slug),
        "tools/call" => handle_tools_call(id, request.params, &state, project_slug).await,
        _ => error_response(
            id,
            METHOD_NOT_FOUND,
            format!("Method not found: {}", request.method),
        ),
    };

    (StatusCode::OK, Json(response))
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
    tools
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

fn handle_tools_list(id: Value, project_slug: &Option<String>) -> Value {
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
/// enforces (1); a missing arm in (2) surfaces as "Tool not found" at runtime.
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
            | "todos_list" | "todos_create" | "todos_update" | "todos_delete"
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
        { "name": "db_tables", "description": "List all tables in the application's SQLite database.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "db_schema", "description": "Describe a table's schema (columns, types, row count).", "inputSchema": { "type": "object", "properties": { "table": { "type": "string" } }, "required": ["table"] } },
        { "name": "db_query", "description": "Run a SELECT query against the database.", "inputSchema": { "type": "object", "properties": { "sql": { "type": "string" }, "params": { "type": "array", "items": {}, "default": [] } }, "required": ["sql"] } },
        { "name": "db_exec", "description": "Execute a mutation (INSERT, UPDATE, DELETE) against the database. Legacy SQLite backend only — apps on postgres-dataverse must use REST `/api/dv/{slug}/{table}` or MCP `dv_*` tools.", "inputSchema": { "type": "object", "properties": { "sql": { "type": "string" }, "params": { "type": "array", "items": {}, "default": [] } }, "required": ["sql"] } },
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
        // ── Todos (per-app, live in Studio right-panel) ──
        { "name": "todos_list", "description": "List the app's todos (optionally filtered by status). Visible live in the Studio right-side panel — consult it at session start, at every transition, and before reporting back to the user.", "inputSchema": { "type": "object", "properties": { "status": { "type": "string", "enum": ["pending", "in_progress"] } } } },
        { "name": "todos_create", "description": "Create a new todo for this app (starts as pending). Appears instantly in the Studio panel — visible to the user. Use only for items the user should see; for internal technical notes, prefer the app's CLAUDE.md.", "inputSchema": { "type": "object", "properties": { "name": { "type": "string", "description": "Short action-oriented title (≤80 chars)" }, "description": { "type": "string" } }, "required": ["name"] } },
        { "name": "todos_update", "description": "Update a todo's fields. Only two statuses exist: `pending` (note) and `in_progress` (current task — only one at a time, others are auto-demoted). To complete or abandon a task, use `todos_delete` — there is no `done` status.", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" }, "description": { "type": "string" }, "status": { "type": "string", "enum": ["pending", "in_progress"] } }, "required": ["id"] } },
        { "name": "todos_delete", "description": "Delete a todo by id. This is how todos are completed — there is no 'done' status, finished tasks must be deleted. Also use this when the user asks to drop a todo.", "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] } }
    ])
}

async fn handle_tools_call(id: Value, params: Value, state: &McpState, project_slug: Option<String>) -> Value {
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let mut arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Pre-contextualize: inject project slug into tools that need it
    if let Some(ref slug) = project_slug {
        let needs_slug = tool_name.starts_with("db.") || tool_name.starts_with("docs.") || tool_name.starts_with("flow.") || matches!(
            tool_name,
            "app.status" | "app.control" | "app.logs" | "app.exec" | "app.get" |
            "app.health" | "app.regenerate_context" | "app.delete" | "app.build" |
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
        // ── Docs (v2) ──
        "docs.overview" => tool_docs_overview(id, &arguments).await,
        "docs.list_entries" => tool_docs_list_entries(id, &arguments).await,
        "docs.get" => tool_docs_get(id, &arguments).await,
        "docs.search" => tool_docs_search(id, &arguments, state).await,
        "docs.list_apps" => tool_docs_list_apps(id).await,
        "docs.completeness" => tool_docs_completeness(id, &arguments).await,
        "docs.diagram_get" => tool_docs_diagram_get(id, &arguments).await,
        "docs.update" => tool_docs_update(id, &arguments, state).await,
        "docs.delete" => tool_docs_delete(id, &arguments, state).await,
        "docs.diagram_set" => tool_docs_diagram_set(id, &arguments, state).await,
        // ── Database ──
        // ── App* (V3 — hr-apps direct supervision) ──
        "app.list" => tool_app_list(id, state).await,
        "app.get" => tool_app_get(id, &arguments, state).await,
        "app.control" => tool_app_control(id, &arguments, state).await,
        "app.status" => tool_app_status(id, &arguments, state).await,
        "app.exec" => tool_app_exec(id, &arguments, state).await,
        "app.build" => tool_app_build(id, &arguments, state).await,
        "app.logs" => tool_app_logs(id, &arguments, state).await,
        "app.create" => tool_app_create(id, &arguments, state).await,
        "app.delete" => tool_app_delete(id, &arguments, state).await,
        "app.regenerate_context" => tool_app_regenerate_context(id, &arguments, state).await,
        // ── Flow* (hr-flow per-app flow engine) ──
        "flow.list_definitions" => tool_flow_list_definitions(id, &arguments, state).await,
        "flow.get_definition" => tool_flow_get_definition(id, &arguments, state).await,
        "flow.list_runs" => tool_flow_list_runs(id, &arguments, state).await,
        "flow.get_run" => tool_flow_get_run(id, &arguments, state).await,
        "flow.run" => tool_flow_run(id, &arguments, state).await,
        "flow.replay" => tool_flow_replay(id, &arguments, state).await,
        // ── Studio ──
        "studio.refresh_context" => tool_studio_refresh_context(id, &arguments, state).await,
        "studio.refresh_all" => tool_studio_refresh_all(id, state).await,
        // ── DB* (V3 — per-app SQLite) ──
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
        "docs_overview" => tool_docs_overview(id, &arguments).await,
        "docs_list_entries" => tool_docs_list_entries(id, &arguments).await,
        "docs_get" => tool_docs_get(id, &arguments).await,
        "docs_search" => tool_docs_search(id, &arguments, state).await,
        "docs_completeness" => tool_docs_completeness(id, &arguments).await,
        "docs_diagram_get" => tool_docs_diagram_get(id, &arguments).await,
        "docs_update" => tool_docs_update(id, &arguments, state).await,
        "docs_delete" => tool_docs_delete(id, &arguments, state).await,
        "docs_diagram_set" => tool_docs_diagram_set(id, &arguments, state).await,
        "git_log" => tool_git_log(id, &arguments, state).await,
        "git_branches" => tool_git_branches(id, &arguments, state).await,
        "todos_list" => tool_todos_list(id, &arguments, state).await,
        "todos_create" => tool_todos_create(id, &arguments, state).await,
        "todos_update" => tool_todos_update(id, &arguments, state).await,
        "todos_delete" => tool_todos_delete(id, &arguments, state).await,
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

// ── Docs tools (v2: structured by overview/screens/features/components + mermaid) ──

fn docs_store() -> Store {
    Store::new(hr_docs::DEFAULT_DOCS_DIR)
}

fn parse_doc_type(s: &str) -> Option<DocType> {
    DocType::from_str(s)
}

fn entry_to_json(entry: &hr_docs::DocEntry, diagram: Option<&str>) -> Value {
    json!({
        "app_id": entry.app_id,
        "type": entry.doc_type.as_str(),
        "name": entry.name,
        "frontmatter": entry.frontmatter,
        "body": entry.body,
        "diagram": diagram,
    })
}

async fn tool_docs_overview(id: Value, args: &Value) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    match docs_store().overview(app_id) {
        Ok(ov) => tool_success(id, serde_json::to_value(&ov).unwrap_or(json!({}))),
        Err(hr_docs::StoreError::AppNotFound(_)) => tool_error(id, &format!("No docs found for '{app_id}'")),
        Err(e) => tool_error(id, &format!("overview failed: {e}")),
    }
}

async fn tool_docs_list_entries(id: Value, args: &Value) -> Value {
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
    match docs_store().list_entries(app_id, doc_type) {
        Ok(entries) => tool_success(id, json!({ "app_id": app_id, "entries": entries })),
        Err(e) => tool_error(id, &format!("list_entries failed: {e}")),
    }
}

async fn tool_docs_get(id: Value, args: &Value) -> Value {
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
    let store = docs_store();
    match store.read_entry(app_id, doc_type, name) {
        Ok(entry) => {
            let diagram = store.read_diagram(app_id, doc_type, &entry.name).ok().flatten();
            tool_success(id, entry_to_json(&entry, diagram.as_deref()))
        }
        Err(hr_docs::StoreError::EntryNotFound { .. }) => {
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
    match idx.search(query, app_id, doc_type, limit) {
        Ok(hits) => tool_success(
            id,
            json!({ "query": query, "count": hits.len(), "results": hits }),
        ),
        Err(e) => tool_error(id, &format!("search failed: {e}")),
    }
}

async fn tool_docs_list_apps(id: Value) -> Value {
    let store = docs_store();
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

async fn tool_docs_completeness(id: Value, args: &Value) -> Value {
    let Some(app_id) = args.get("app_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing app_id".into());
    };
    if !validate_app_id(app_id) {
        return tool_error(id, "Invalid app_id");
    }
    let store = docs_store();
    let overview = match store.overview(app_id) {
        Ok(o) => o,
        Err(hr_docs::StoreError::AppNotFound(_)) => {
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

async fn tool_docs_diagram_get(id: Value, args: &Value) -> Value {
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
    match docs_store().read_diagram(app_id, doc_type, name) {
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
    let store = docs_store();
    let _ = store.ensure_layout(app_id);
    if !store.app_dir(app_id).exists() {
        let _ = std::fs::create_dir_all(store.app_dir(app_id));
    }
    if !store.app_dir(app_id).join("meta.json").exists() {
        let _ = store.write_meta(app_id, &hr_docs::Meta::new(app_id));
    }

    match store.write_entry(app_id, doc_type, name, frontmatter, body) {
        Ok(entry) => {
            // Sync FTS index.
            if let Some(idx) = state.docs_index.as_ref() {
                if let Err(e) = idx.upsert(&entry) {
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
    match docs_store().delete_entry(app_id, doc_type, name) {
        Ok(deleted) => {
            if deleted {
                if let Some(idx) = state.docs_index.as_ref() {
                    if let Err(e) = idx.remove(app_id, doc_type, name) {
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
    let store = docs_store();
    if let Err(e) = store.write_diagram(app_id, doc_type, name, mermaid) {
        return tool_error(id, &format!("diagram_set failed: {e}"));
    }
    // The diagram flag is now true; re-index the entry so search reflects it.
    if let (Some(idx), Ok(entry)) = (
        state.docs_index.as_ref(),
        store.read_entry(app_id, doc_type, name),
    ) {
        if let Err(e) = idx.upsert(&entry) {
            warn!(error = %e, "Docs index upsert failed after diagram set");
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

// ── App* / DB* tool definitions (V3 — hr-apps) ──────────────────────

fn tool_definitions_apps() -> Value {
    json!([
        {
            "name": "app.list",
            "description": "List all HomeRoute applications managed by the AppSupervisor.",
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
            "description": "Build an app remotely on CloudMaster (rsync src up, build, rsync artefacts down). Synchronous; bounded by `timeout_secs` (default 1800 = 30 min). Stacks: axum, axum-vite, next-js. Returns AppExecResult (stdout/stderr/exit_code/duration_ms).",
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
            "description": "List user-defined tables in an app's SQLite database.",
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
            "description": "Run a SELECT query against an app's SQLite database.",
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
            "name": "db.find",
            "description": "Query rows of a table with structured filters, sort, pagination and relation expand. No SQL required.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "table": { "type": "string" },
                    "filters": {
                        "type": "array",
                        "description": "List of {column, op, value?}. op ∈ eq|ne|gt|lt|gte|lte|like|in|is_null|is_not_null"
                    },
                    "limit": { "type": "integer", "default": 100, "description": "Capped at 1000" },
                    "offset": { "type": "integer", "default": 0 },
                    "order_by": { "type": "string" },
                    "order_desc": { "type": "boolean", "default": false },
                    "expand": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Foreign-key relations to hydrate inline"
                    }
                },
                "required": ["slug", "table"]
            }
        },
        {
            "name": "db.execute",
            "description": "Execute a mutation (INSERT, UPDATE, DELETE) against an app's SQLite database.",
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
            "description": "Sync existing SQLite tables into Dataverse metadata. Use after manual DDL changes.",
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
        // ── Flow tools (hr-flow per-app flow engine) ──
        {
            "name": "flow.list_definitions",
            "description": "List flow definitions declared by an app (TOML files under `src/flows/`). Returns each flow's name, description, and step count.",
            "inputSchema": {
                "type": "object",
                "properties": { "slug": { "type": "string" } },
                "required": ["slug"]
            }
        },
        {
            "name": "flow.get_definition",
            "description": "Read a single flow definition (parsed from `src/flows/{name}.toml`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["slug", "name"]
            }
        },
        {
            "name": "flow.list_runs",
            "description": "List recent runs of an app's flows (most recent first). Filter by `flow_name` if set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "flow_name": { "type": "string" },
                    "limit": { "type": "integer", "default": 50 }
                },
                "required": ["slug"]
            }
        },
        {
            "name": "flow.get_run",
            "description": "Read a complete run trace by id, including the full step tree (input/output/duration/error per step).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "run_id": { "type": "string" }
                },
                "required": ["slug", "run_id"]
            }
        },
        {
            "name": "flow.run",
            "description": "Trigger a flow manually with the given input. Returns the RunResult: run_id, status, output (or structured error pinpointing the failed step). Calls the app's internal trigger endpoint.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "name": { "type": "string", "description": "Flow name" },
                    "input": { "type": "object", "description": "Input value passed to the flow" }
                },
                "required": ["slug", "name"]
            }
        },
        {
            "name": "flow.replay",
            "description": "Replay an existing run by id — re-executes the flow under its original input. Produces a new run row with status=success/failed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "run_id": { "type": "string" }
                },
                "required": ["slug", "run_id"]
            }
        }
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
        .ok_or_else(|| tool_error(id.clone(), "hr-apps not initialized"))
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

// ── Flow tool handlers (hr-flow per-app flow engine) ────────────────

/// Resolve `slug → (flows_dir, runs_dir)` based on the app's filesystem
/// layout (`{app_dir}/src/flows`, `{app_dir}/runs`). Returns an MCP error
/// response when the app isn't registered.
async fn flow_dirs_for(
    id: &Value,
    state: &McpState,
    slug: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf, u16), Value> {
    let ctx = require_apps_ctx(id, state).map_err(|e| e)?;
    let app = ctx
        .supervisor
        .registry
        .get(slug)
        .await
        .ok_or_else(|| tool_error(id.clone(), &format!("app not found: {slug}")))?;
    let flows_dir = app.src_dir().join("flows");
    let runs_dir = app.app_dir().join("runs");
    Ok((flows_dir, runs_dir, app.port))
}

async fn tool_flow_list_definitions(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let (flows_dir, _, _) = match flow_dirs_for(&id, state, slug).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let mut out = Vec::new();
    let read = match std::fs::read_dir(&flows_dir) {
        Ok(r) => r,
        Err(_) => return tool_success(id, json!({ "flows": [] })),
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") { continue; }
        let body = match std::fs::read_to_string(&path) { Ok(b) => b, Err(_) => continue };
        let parsed = match hr_flow::parse_flow_toml(&body) { Ok(p) => p, Err(_) => continue };
        out.push(json!({
            "name": parsed.name,
            "description": parsed.description,
            "step_count": parsed.steps.len(),
            "file": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        }));
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    tool_success(id, json!({ "flows": out }))
}

async fn tool_flow_get_definition(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    let (flows_dir, _, _) = match flow_dirs_for(&id, state, slug).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let path = flows_dir.join(format!("{name}.toml"));
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) => return tool_error(id, &format!("flow `{name}` not found: {e}")),
    };
    let parsed = match hr_flow::parse_flow_toml(&body) {
        Ok(p) => p,
        Err(e) => return tool_error(id, &format!("invalid TOML: {e}")),
    };
    tool_success(id, json!({ "definition": parsed, "source": body }))
}

async fn tool_flow_list_runs(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let flow_name = args.get("flow_name").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let (_, runs_dir, _) = match flow_dirs_for(&id, state, slug).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let read = match std::fs::read_dir(&runs_dir) {
        Ok(r) => r,
        Err(_) => return tool_success(id, json!({ "runs": [] })),
    };
    let mut entries: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let modified = entry.metadata().and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((modified, path));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let mut out = Vec::new();
    for (_, path) in entries {
        if out.len() >= limit { break; }
        let body = match std::fs::read_to_string(&path) { Ok(b) => b, Err(_) => continue };
        let doc: serde_json::Value = match serde_json::from_str(&body) { Ok(d) => d, Err(_) => continue };
        if let Some(filter) = flow_name {
            if doc.get("flow_name").and_then(|v| v.as_str()) != Some(filter) { continue; }
        }
        // Strip the heavy `steps` array — caller fetches the full doc with
        // flow.get_run when they want the tree.
        let mut summary = doc.clone();
        if let Some(obj) = summary.as_object_mut() {
            obj.remove("steps");
        }
        out.push(summary);
    }
    tool_success(id, json!({ "runs": out }))
}

async fn tool_flow_get_run(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(run_id) = args.get("run_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing run_id".into());
    };
    let (_, runs_dir, _) = match flow_dirs_for(&id, state, slug).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let path = runs_dir.join(format!("{run_id}.json"));
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) => return tool_error(id, &format!("run `{run_id}` not found: {e}")),
    };
    let doc: serde_json::Value = match serde_json::from_str(&body) {
        Ok(d) => d,
        Err(e) => return tool_error(id, &format!("invalid run JSON: {e}")),
    };
    tool_success(id, doc)
}

async fn tool_flow_run(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(name) = args.get("name").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing name".into());
    };
    let input = args.get("input").cloned().unwrap_or(json!({}));
    let (_, _, port) = match flow_dirs_for(&id, state, slug).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let url = format!("http://127.0.0.1:{port}/api/_internal/flows/run");
    let body = json!({ "name": name, "input": input });
    match reqwest::Client::new().post(&url).json(&body).send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return tool_error(id, &format!("app trigger failed ({status}): {text}"));
            }
            let parsed: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|_| json!({ "raw": text }));
            tool_success(id, parsed)
        }
        Err(e) => tool_error(id, &format!("contact app: {e}")),
    }
}

async fn tool_flow_replay(id: Value, args: &Value, state: &McpState) -> Value {
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(run_id) = args.get("run_id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing run_id".into());
    };
    let (_, _, port) = match flow_dirs_for(&id, state, slug).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let url = format!("http://127.0.0.1:{port}/api/_internal/flows/replay");
    let body = json!({ "run_id": run_id });
    match reqwest::Client::new().post(&url).json(&body).send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return tool_error(id, &format!("app replay failed ({status}): {text}"));
            }
            let parsed: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|_| json!({ "raw": text }));
            tool_success(id, parsed)
        }
        Err(e) => tool_error(id, &format!("contact app: {e}")),
    }
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

/// Convert an `IpcResponse` into a JSON-RPC response Value.
fn ipc_resp_to_mcp(id: Value, resp: hr_ipc::types::IpcResponse) -> Value {
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
    let sql = format!("SELECT COUNT(*) as count FROM \"{}\"", table.replace('"', ""));
    ipc_resp_to_mcp(id, ctx.db_query(slug.to_string(), sql, vec![]).await)
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

// ── Todos tools ─────────────────────────────────────────────────────

async fn tool_todos_list(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let status = args.get("status").and_then(|v| v.as_str()).map(String::from);
    ipc_resp_to_mcp(id, ctx.todos_list(slug.to_string(), status).await)
}

async fn tool_todos_create(id: Value, args: &Value, state: &McpState) -> Value {
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
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);
    ipc_resp_to_mcp(
        id,
        ctx.todos_create(slug.to_string(), name.to_string(), description)
            .await,
    )
}

async fn tool_todos_update(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(todo_id) = args.get("id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing id".into());
    };
    let name = args.get("name").and_then(|v| v.as_str()).map(String::from);
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);
    let status = args.get("status").and_then(|v| v.as_str()).map(String::from);
    ipc_resp_to_mcp(
        id,
        ctx.todos_update(
            slug.to_string(),
            todo_id.to_string(),
            name,
            description,
            status,
        )
        .await,
    )
}

async fn tool_todos_delete(id: Value, args: &Value, state: &McpState) -> Value {
    let ctx = match require_apps_ctx(&id, state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let Some(slug) = args.get("slug").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing slug".into());
    };
    let Some(todo_id) = args.get("id").and_then(|v| v.as_str()) else {
        return error_response(id, INVALID_PARAMS, "Missing id".into());
    };
    ipc_resp_to_mcp(
        id,
        ctx.todos_delete(slug.to_string(), todo_id.to_string()).await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guarantees parity between `tool_definitions_project()` (what clients
    /// discover) and `is_project_simplified_tool()` (what the dispatcher
    /// treats as project-scoped and injects the slug into). If these drift,
    /// a client sees a tool it cannot call (or calls one without a slug).
    #[test]
    fn project_scoped_tools_are_consistent() {
        let defs = tool_definitions_project();
        let names: Vec<String> = defs
            .as_array()
            .expect("tool_definitions_project must be an array")
            .iter()
            .map(|t| {
                t.get("name")
                    .and_then(|n| n.as_str())
                    .expect("every tool definition has a name")
                    .to_string()
            })
            .collect();

        for name in &names {
            assert!(
                is_project_simplified_tool(name),
                "tool `{name}` is advertised by tool_definitions_project() but \
                 is_project_simplified_tool() does not recognize it. Add it \
                 there AND add a match arm in handle_tools_call."
            );
        }
    }
}
