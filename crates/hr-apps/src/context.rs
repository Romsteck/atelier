//! Claude Code context generation for per-app Studio awareness.
//!
//! # INVARIANT — workspace per-app
//!
//! Le workspace code-server de chaque app est `{apps_path}/{slug}/src/` (voir
//! `web/src/pages/Studio.jsx` : `?folder=.../{slug}/src`). L'agent Claude Code
//! ne lit **que** ce qui vit sous `src/`. TOUT fichier destiné à l'agent
//! (CLAUDE.md, .claude/, .mcp.json) DOIT donc être écrit sous `src/` ; les
//! fichiers au niveau `{apps_path}/{slug}/` (au-dessus de `src/`) sont
//! invisibles pour l'agent et sont activement supprimés par `generate_for_app`
//! pour éviter toute confusion avec une version stale.
//!
//! Fichiers per-app générés (tous sous `{slug}/src/`) :
//!   - `src/CLAUDE.md`                         — carnet de bord agent-owned (write-once)
//!   - `src/.mcp.json`                         — MCP server config (CLI compat)
//!   - `src/.claude/settings.json`             — MCP server + auto-approve
//!   - `src/.claude/rules/app-info.md`         — identité / stack / port / autres apps (régénéré)
//!   - `src/.claude/rules/mcp-tools.md`        — tools MCP disponibles
//!   - `src/.claude/rules/workflow.md`         — workflow dev
//!   - `src/.claude/rules/docs.md`             — usage obligatoire de `docs.*`
//!   - `src/.claude/rules/todos.md`            — usage obligatoire de `todos.*` (panneau Studio)
//!   - `src/.claude/rules/claude-md-upkeep.md` — règle de maintenance de CLAUDE.md
//!   - `src/.claude/rules/store-publishing.md` — Flutter uniquement
//!   - `src/.claude/skills/app-build/{SKILL.md,build.sh}`
//!   - `src/.claude/skills/{app-status,app-logs,app-db-info}/SKILL.md`
//!
//! Fichiers workspace-root (pour le Studio global `studio.mynetwk.biz`,
//! workspace = `/opt/homeroute/apps/`) :
//!   - `{apps_path}/CLAUDE.md`
//!   - `{apps_path}/.claude/settings.json`
//!   - `{apps_path}/.mcp.json`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::types::{AppStack, Application, Visibility};

/// Generates Claude Code context files for HomeRoute apps.
pub struct ContextGenerator {
    pub apps_path: PathBuf,
    pub base_domain: String,
    pub mcp_endpoint: String,
    pub mcp_token: Option<String>,
}

impl ContextGenerator {
    pub fn new(
        apps_path: impl Into<PathBuf>,
        base_domain: impl Into<String>,
        mcp_endpoint: impl Into<String>,
    ) -> Self {
        let mcp_token = std::env::var("MCP_TOKEN").ok();
        Self {
            apps_path: apps_path.into(),
            base_domain: base_domain.into(),
            mcp_endpoint: mcp_endpoint.into(),
            mcp_token,
        }
    }

    /// Generate all context files for a single app. Idempotent.
    ///
    /// INVARIANT : tout ce qui est destiné à l'agent est écrit sous
    /// `app.src_dir() == {apps_path}/{slug}/src/`. Le niveau parent
    /// `{apps_path}/{slug}/` est réservé aux fichiers runtime (db.sqlite, .env)
    /// et les éventuels CLAUDE.md/.claude/.mcp.json qui s'y trouvent (vestiges)
    /// sont supprimés par `cleanup_legacy_parent_context` à chaque appel.
    ///
    /// Voir le doc-comment du module (`//!`) pour la structure cible complète.
    pub fn generate_for_app(
        &self,
        app: &Application,
        all_apps: &[Application],
        db_tables: Option<Vec<String>>,
    ) -> anyhow::Result<()> {
        let src_dir = app.src_dir();
        let app_dir = self.apps_path.join(&app.slug);

        // Cleanup des fichiers au mauvais niveau, même si src_dir n'existe pas.
        cleanup_legacy_parent_context(&app_dir, &app.slug);

        // Si src_dir absent : scaffold incomplet, soft-skip (avec warn).
        if !src_dir.exists() {
            warn!(
                slug = %app.slug,
                src_dir = %src_dir.display(),
                "src_dir absent — context generation skipped (scaffold incomplete?)"
            );
            return Ok(());
        }

        self.generate_for_app_at(app, &src_dir, all_apps, db_tables, false)
    }

    /// Variante explicite de [`Self::generate_for_app`] qui prend en paramètre
    /// le `src_dir` cible (au lieu de `app.src_dir()` hardcodé). Utilisé par
    /// AppCreate pour générer le contexte dans un tmpdir local avant rsync UP
    /// vers CloudMaster.
    ///
    /// `cleanup_legacy_parent` contrôle si on supprime les vestiges au niveau
    /// `{apps_path}/{slug}/` (CLAUDE.md/.mcp.json/.claude/) — utile pour la
    /// génération in-place sur Medion (true), inutile pour un tmpdir (false).
    pub fn generate_for_app_at(
        &self,
        app: &Application,
        src_dir: &Path,
        all_apps: &[Application],
        db_tables: Option<Vec<String>>,
        cleanup_legacy_parent: bool,
    ) -> anyhow::Result<()> {
        let app_dir = self.apps_path.join(&app.slug);

        // Step 1 — Cleanup des fichiers au mauvais niveau (parent) : legacy du
        // passé où on écrivait par erreur CLAUDE.md/.claude/.mcp.json à côté
        // de src/ au lieu de dedans.
        if cleanup_legacy_parent {
            cleanup_legacy_parent_context(&app_dir, &app.slug);
        }

        // Step 2 — Si src_dir n'existe pas, créer le squelette (cas tmpdir vide).
        if !src_dir.exists() {
            fs::create_dir_all(src_dir)?;
        }

        let src_claude_dir = src_dir.join(".claude");
        let src_rules_dir = src_claude_dir.join("rules");
        let src_skills_dir = src_claude_dir.join("skills");
        fs::create_dir_all(&src_rules_dir)?;
        fs::create_dir_all(&src_skills_dir)?;

        // Step 3 — Project-scoped MCP config + settings, au seul niveau src/.
        let project_mcp = format!("{}?project={}", self.mcp_endpoint, app.slug);
        let settings = render_settings_json_with_auth(&project_mcp, self.mcp_token.as_deref());
        log_write(&app.slug, &src_claude_dir.join("settings.json"), &settings)?;
        let mcp_json = render_mcp_json_with_auth(&project_mcp, self.mcp_token.as_deref());
        log_write(&app.slug, &src_dir.join(".mcp.json"), &mcp_json)?;

        // Step 4 — Règles régénérées intégralement.
        // ORDRE : docs.md en TÊTE pour souligner que la lecture de la doc passe avant
        // toute autre chose (cf. plan « DOC-FIRST OBLIGATOIRE »).
        log_write(&app.slug, &src_rules_dir.join("docs.md"),
                  &render_docs_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("app-info.md"),
                  &render_app_info_md(app, all_apps, &db_tables))?;
        // db.md varies by `app.db_backend`:
        //  - LegacySqlite      → "MIGRATION POSTGRES EN ATTENTE" (warns to
        //    minimise schema changes until the migration runs)
        //  - PostgresDataverse → new stack rules + post-migration cleanup
        //    instructions (delete leftover SQLite refs)
        log_write(&app.slug, &src_rules_dir.join("db.md"),
                  &render_db_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("mcp-tools.md"),
                  &render_mcp_tools_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("workflow.md"),
                  &self.render_workflow_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("todos.md"),
                  &render_todos_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("claude-md-upkeep.md"),
                  &render_claude_md_upkeep_md())?;

        if matches!(app.stack, AppStack::Flutter) {
            log_write(&app.slug, &src_rules_dir.join("store-publishing.md"),
                      &render_store_publishing_md(app))?;
        } else {
            remove_if_exists(&src_rules_dir.join("store-publishing.md"), &app.slug);
        }

        // `flows-first` always-on rule — pushed to all Rust backends (axum
        // / axum-vite). Wallet was the pilot ; other Rust apps are now
        // expected to migrate. NextJS/Flutter are out of scope.
        if is_flows_eligible(app) {
            log_write(&app.slug, &src_rules_dir.join("flows-first.md"),
                      &render_flows_first_rule(app))?;
        } else {
            remove_if_exists(&src_rules_dir.join("flows-first.md"), &app.slug);
        }

        // Step 5 — Cleanup des règles obsolètes (scaffolds anciens, systèmes précédents).
        for legacy in OBSOLETE_RULE_FILES {
            remove_if_exists(&src_rules_dir.join(legacy), &app.slug);
        }

        // Step 6 — Skills.
        let app_build_dir = src_skills_dir.join("app-build");
        fs::create_dir_all(&app_build_dir)?;
        log_write(&app.slug, &app_build_dir.join("SKILL.md"), &render_app_build_skill(app))?;
        log_write(&app.slug, &app_build_dir.join("build.sh"), &render_app_build_script(app))?;

        let app_deploy_dir = src_skills_dir.join("app-deploy");
        fs::create_dir_all(&app_deploy_dir)?;
        log_write(&app.slug, &app_deploy_dir.join("SKILL.md"), &render_app_deploy_skill(app))?;
        log_write(&app.slug, &app_deploy_dir.join("deploy.sh"), &render_app_deploy_script(app))?;

        let produced: std::collections::HashSet<&'static str> = render_extra_skills(app)
            .iter()
            .map(|(name, _)| *name)
            .collect();
        for (name, content) in render_extra_skills(app) {
            let skill_dir = src_skills_dir.join(name);
            fs::create_dir_all(&skill_dir)?;
            log_write(&app.slug, &skill_dir.join("SKILL.md"), &content)?;
        }
        for legacy_name in ALL_EXTRA_SKILL_NAMES {
            if !produced.contains(legacy_name) {
                let dir = src_skills_dir.join(legacy_name);
                if dir.exists() {
                    let _ = fs::remove_dir_all(&dir);
                }
            }
        }

        // Step 7 — Cleanup des slash-commands legacy (tout est skill désormais).
        let commands_dir = src_claude_dir.join("commands");
        for legacy in OBSOLETE_SLASH_COMMANDS {
            remove_if_exists(&commands_dir.join(legacy), &app.slug);
        }
        if commands_dir.exists() {
            if let Ok(mut entries) = fs::read_dir(&commands_dir) {
                if entries.next().is_none() {
                    let _ = fs::remove_dir(&commands_dir);
                }
            }
        }

        // Step 8 — CLAUDE.md initial (skeleton), créé UNE SEULE FOIS.
        // L'agent est ensuite propriétaire du fichier : la régénération ne le touche plus.
        let claude_md_path = src_dir.join("CLAUDE.md");
        if write_if_missing(&claude_md_path, &render_initial_claude_md(app))? {
            info!(slug = %app.slug, file = %claude_md_path.display(), "CLAUDE.md skeleton created");
        }

        info!(slug = %app.slug, "context files generated");
        Ok(())
    }

    /// Generate the workspace-root context files (CLAUDE.md, settings.json, .mcp.json).
    pub fn generate_root(&self, all_apps: &[Application]) -> anyhow::Result<()> {
        let claude_dir = self.apps_path.join(".claude");
        fs::create_dir_all(&claude_dir)?;

        let claude_md = self.render_root_claude_md(all_apps);
        log_write("<root>", &self.apps_path.join("CLAUDE.md"), &claude_md)?;

        let settings = render_settings_json_with_auth(&self.mcp_endpoint, self.mcp_token.as_deref());
        log_write("<root>", &claude_dir.join("settings.json"), &settings)?;

        let mcp_json = render_mcp_json_with_auth(&self.mcp_endpoint, self.mcp_token.as_deref());
        log_write("<root>", &self.apps_path.join(".mcp.json"), &mcp_json)?;

        info!(count = all_apps.len(), "workspace-root context written");
        Ok(())
    }

    /// Refresh per-app context for every app + workspace-root context.
    pub fn refresh_all(&self, all_apps: &[Application]) -> anyhow::Result<()> {
        for app in all_apps {
            if let Err(e) = self.generate_for_app(app, all_apps, None) {
                warn!(slug = %app.slug, error = %e, "failed to generate app context");
            }
        }
        if let Err(e) = self.generate_root(all_apps) {
            warn!(error = %e, "failed to generate root context");
        }
        Ok(())
    }

    // ── Renderers ──────────────────────────────────────────────────────

    fn render_root_claude_md(&self, all_apps: &[Application]) -> String {
        let mut table_rows = String::new();
        for app in all_apps {
            let db_cell = if app.has_db {
                format!("`{}`", app.db_path().display())
            } else {
                "—".to_string()
            };
            let visibility = match app.visibility {
                Visibility::Public => "public",
                Visibility::Private => "private",
            };
            table_rows.push_str(&format!(
                "| {name} | `{slug}` | {stack} | https://{domain} | {visibility} | {db} |\n",
                name = app.name,
                slug = app.slug,
                stack = app.stack.display_name(),
                domain = app.domain,
                visibility = visibility,
                db = db_cell,
            ));
        }

        if table_rows.is_empty() {
            table_rows.push_str("| _no apps yet_ |  |  |  |  |  |\n");
        }

        format!(
            "# HomeRoute Apps Workspace\n\
             \n\
             This is the workspace root for every application managed by HomeRoute. \
             Each app lives under `{apps_path}/<slug>/` with its own sources, build \
             artifacts, `.env` and (optionally) managed SQLite DB.\n\
             \n\
             ## Documentation (DOC-FIRST OBLIGATOIRE)\n\
             \n\
             Chaque app expose une documentation structurée (overview + écrans + \
             features per-screen/global + composants + diagrammes mermaid) accessible via \
             les tools MCP `docs.*`. **Avant toute modification dans une app**, suivre \
             le workflow doc-first :\n\
             \n\
             1. `docs.overview(app_id=<slug>)` — panorama compact (overview + index)\n\
             2. `docs.search` ou `docs.get` — cibler la zone touchée\n\
             3. Modifier le code\n\
             4. `docs.update` + `docs.diagram_set` si flux changé\n\
             \n\
             La doc est la source de vérité de l'intention. **Ne jamais coder à \
             l'aveugle sans la lire d'abord.** Voir la rule `.claude/rules/docs.md` dans \
             chaque app pour le détail.\n\
             \n\
             ## Apps\n\
             | Name | Slug | Stack | URL | Visibility | DB path |\n\
             | --- | --- | --- | --- | --- | --- |\n\
             {table_rows}\
             \n\
             ## How HomeRoute runs apps\n\
             - Apps run **directly on the host** as processes supervised by HomeRoute \
             (no nspawn container, no env-agent).\n\
             - The reverse proxy `hr-edge` terminates TLS on `*.{base_domain}` and forwards to \
             each app's local port.\n\
             - The orchestrator manages the process lifecycle (start, stop, restart, logs, \
             health) and exposes everything via MCP.\n\
             \n\
             ## Working in this workspace\n\
             - Open any `<slug>/` subdirectory to focus on a single app — its `.claude/` \
             folder will scope Claude Code to that project.\n\
             - From this root, use the MCP tool `app.list` to enumerate apps, then \
             `app.status` / `app.logs` / `app.restart` to operate on them.\n\
             - Edit sources in `<slug>/src/`, then `app.restart <slug>` and verify on the \
             public URL.\n\
             \n\
             ## MCP\n\
             A single MCP server `homeroute` is configured at `{mcp_endpoint}` via \
             `.claude/settings.json` and `.mcp.json`. Read-only tools (`app.list`, \
             `app.status`, `app.logs`, `db.tables`, `db.schema`, `db.query`, \
             `docs.overview`, `docs.list_entries`, `docs.get`, `docs.search`, \
             `docs.completeness`, `docs.diagram_get`) are auto-approved. Doc \
             mutations (`docs.update`, `docs.delete`, `docs.diagram_set`) require \
             confirmation.\n\
             \n\
             ## Rules\n\
             - **Always read the app's docs (`docs.overview`) BEFORE exploring code or \
             making changes.**\n\
             - Never use `ssh`, `scp` or direct filesystem access on `*.db` files — go \
             through the MCP `db.*` tools.\n\
             - Apps must read their listening port from `PORT`, never hardcode it.\n\
             - Update each app's docs (`docs.update` / `docs.diagram_set`) after meaningful \
             changes (new screen, feature, component, or flow).\n",
            apps_path = self.apps_path.display(),
            table_rows = table_rows,
            base_domain = self.base_domain,
            mcp_endpoint = self.mcp_endpoint,
        )
    }

    fn render_workflow_md(&self, app: &Application) -> String {
        let build_cmd = app.build_command.as_deref().unwrap_or("(no build step)");
        let url = format!("https://{}", app.domain);

        format!(
            "# Workflow — {name} ({stack})\n\
             \n\
             ## Process\n\
             - **Run:** `{run_command}`\n\
             - **Build:** `{build_cmd}`\n\
             - **Health:** `{health_path}`\n\
             - **Public URL:** {url}\n\
             - Managed by HomeRoute as a host-level process. Use MCP `app.*` tools to \
             control it — **never** lancer le binaire à la main (`nohup`, `tmux`, \
             `./bin/xxx &`, `cargo run`, `systemctl`, `kill`).\n\
             \n\
             ## Interdits (et pourquoi)\n\
             - **Lancer le binaire à la main** : le superviseur vérifie que le port \
             `{port}` est libre avant de spawner. Un process manuel sur ce port bloque \
             `app.control start` avec `port not free` — l'app semble morte pour \
             l'orchestrateur alors qu'elle tourne. Pour tester un binaire : \
             `app.control restart` + `app.logs`, jamais `nohup`.\n\
             - **`kill -9` du process supervisé** : le superviseur le relance avec \
             backoff. Utilise `app.control stop`.\n\
             - **Binder un autre service sur `{port}`** : même symptôme que le nohup.\n\
             \n\
             ## Debug d'un démarrage qui échoue\n\
             1. `app.status` → state (`crashed`, `stopped`, `running`) + restart_count.\n\
             2. `app.logs` → lignes orchestrateur : `port not free`, `spawn failed`, \
             `process exited code=...`.\n\
             3. Vérifier que `{run_command}` existe et est exécutable dans `{src_dir}`.\n\
             4. Si tout semble OK mais rien ne démarre → `ss -lntp | grep {port}` via \
             `app.exec` pour voir qui squatte le port.\n\
             \n\
             ## Edit → build → restart → verify\n\
             1. Edit sources in `{src_dir}`.\n\
             2. Build on place : `{build_cmd}` (toujours en production, jamais de mode dev).\n\
             3. Restart via MCP: `app.control` (ou `POST /api/apps/{slug}/control` avec `{{\"action\":\"restart\"}}`).\n\
             4. Check the result via `app.status` and `app.logs`.\n\
             5. Open {url} to validate the change end-to-end.\n\
             \n\
             ## Regles\n\
             - **Builder sur place** : jamais de cross-compile, tout se compile sur le serveur de production.\n\
             - **Pas de mode dev** : pas de `pnpm dev` / `cargo watch`. Production only.\n\
             - **Pas de pipelines** : pas de promotion dev→acc→prod.\n\
             \n\
             ## Environment variables\n\
             The orchestrator injects:\n\
             - `PORT` — listen on this port. **Never hardcode** a port in the code.\n\
             - `DATABASE_URL` / `DATABASE_PATH` — path to the managed SQLite DB \
             (only when `has_db` is true).\n\
             - Any custom variables declared on the application (managed via the API).\n\
             \n\
             ## Database\n\
             - Use the MCP `db.*` tools for every read/write — they target the managed DB \
             for this app automatically.\n\
             - Never open the `.db` file by hand.\n\
             \n\
             ## Documentation (DOC-FIRST OBLIGATOIRE)\n\
             - **Avant toute exploration de code**, appelle `docs_overview` — c'est non \
             négociable. Voir `.claude/rules/docs.md`.\n\
             - Cible avec `docs_search` ou `docs_list_entries` selon que tu as un \
             mot-clé ou que tu explores une catégorie.\n\
             - Lis l'entrée pertinente avec `docs_get` (et son `docs_diagram_get` si flux).\n\
             - Après une feature / écran / composant ajouté ou modifié : `docs_update` \
             (et `docs_diagram_set` si le flux change).\n\
             \n\
             ## Logging\n\
             - Add structured log lines for new handlers, IPC calls, errors, and \
             unexpected branches.\n\
             - Inspect logs via `app.logs` and the HomeRoute logs page.\n",
            name = app.name,
            stack = app.stack.display_name(),
            run_command = app.run_command,
            build_cmd = build_cmd,
            health_path = app.health_path,
            url = url,
            src_dir = app.src_dir().display(),
            slug = app.slug,
            port = app.port,
        )
    }
}

// ── Standalone helpers ─────────────────────────────────────────────────

fn render_mcp_tools_md(app: &Application) -> String {
    format!(
        "# MCP tools — {name}\n\
         \n\
         A single MCP server is configured: `homeroute`. Read-only tools are \
         auto-approved via `.claude/settings.json` — mutations require explicit \
         confirmation.\n\
         \n\
         ## Documentation (`docs_*`) — DOC-FIRST OBLIGATOIRE\n\
         **Avant toute exploration de code, appelle `docs_overview`.** Voir `.claude/rules/docs.md` pour le workflow complet.\n\
         - `docs_overview` — vue d'ensemble + index compact (à lire EN PREMIER)\n\
         - `docs_list_entries` — liste les entrées par type (screen/feature/component)\n\
         - `docs_get` — lit une entrée complète (markdown + diagramme mermaid)\n\
         - `docs_search` — recherche full-text BM25 ciblée\n\
         - `docs_completeness` — diagnostic des sections manquantes\n\
         - `docs_diagram_get` — récupère un diagramme mermaid\n\
         - `docs_update` — crée/met à jour une entrée (mutation, non auto-approuvé)\n\
         - `docs_diagram_set` — attache un diagramme mermaid (mutation, non auto-approuvé)\n\
         - `docs_delete` — supprime une entrée (mutation, non auto-approuvé)\n\
         \n\
         ## Apps (`app.*`)\n\
         - `app.list` — list every application\n\
         - `app.status` — runtime status of an app (state, port, health)\n\
         - `app.create` — register a new application\n\
         - `app.control` — start / stop / restart\n\
         - `app.exec` — run a one-shot command in the app's context\n\
         - `app.logs` — tail recent logs for an app\n\
         - `app.delete` — remove an application (mutation, not auto-approved)\n\
         \n\
         ## Database (`db.*`)\n\
         - `db.tables` — list tables for `{slug}` (or any app)\n\
         - `db.schema` — describe a table\n\
         - `db.query` — read or mutate via SQL (mutating SQL is not auto-approved)\n\
         \n\
         ## Todos (`todos_*`) — visibles dans le panneau droit du Studio\n\
         - `todos_list` — lister les todos (filtre optionnel par `status` : `pending` ou `in_progress`)\n\
         - `todos_create` — créer (`name`, `description?`) — démarre en `pending`\n\
         - `todos_update` — modifier (`id`, `status?` parmi `pending`/`in_progress`)\n\
         - `todos_delete` — supprimer (`id`) — c'est ainsi qu'on clôt une tâche (pas de statut « done »)\n\
         \n\
         ## Store (`store.*`)\n\
         - Tools for the HomeRoute mobile store (uploads, listings).\n\
         \n\
         ## Build\n\
         Pour builder cette app, utilise la skill **app-build** (lazy-loaded). Elle appelle l'endpoint HTTP bloquant via Bash.\n\
         \n\
",
        name = app.name,
        slug = app.slug,
    )
}

fn render_docs_md(app: &Application) -> String {
    format!(
        r#"# Documentation — {name} (DOC-FIRST OBLIGATOIRE)

> **TL;DR** : avant TOUT travail sur cette app — avant le moindre `Read`, le moindre `grep`,
> la moindre exploration — tu **DOIS** appeler `docs_overview`. La doc est la source de
> vérité de l'intention. La lire en dernier conduit à recréer ce qui existe ou à casser
> un invariant.

## 1. Workflow obligatoire (dans cet ordre)

1. **`docs_overview`** — TOUJOURS EN PREMIER. Renvoie l'overview prose + un index compact
   de toutes les entrées (titre + résumé 1 ligne) + stats. Permet de cadrer la tâche en peu
   de tokens.
2. **`docs_search` (mot-clé)** ou **`docs_list_entries` (par catégorie)** — pour cibler la
   zone touchée par la tâche utilisateur. Préfère `docs_search` dès qu'un mot-clé est
   exploitable.
3. **`docs_get`** — lire les entrées pertinentes en détail (markdown + diagramme mermaid).
4. **`docs_diagram_get`** — si l'entrée a un diagramme et que tu modifies un flux, lis-le.
5. **Exploration code** — UNIQUEMENT après les étapes 1-4. Sinon tu travailles à l'aveugle.
6. **Modification** — applique le changement.
7. **`docs_update`** — mets à jour les entrées impactées. **Ajoute** une nouvelle entrée si
   tu introduis un nouvel écran / feature / composant.
8. **`docs_diagram_set`** — régénère le mermaid si le flux a changé.
9. **`docs_completeness`** — vérifie qu'il ne manque pas de summary / diagramme.

## 2. Tools disponibles

| Tool | Auto-approuvé | Quand l'utiliser |
|---|---|---|
| `docs_overview` | ✅ | Premier appel de chaque tâche |
| `docs_list_entries` | ✅ | Explorer une catégorie (`type` ∈ screen/feature/component) |
| `docs_get` | ✅ | Lire une entrée précise |
| `docs_search` | ✅ | Recherche FTS5 ranked, mot-clé requis |
| `docs_completeness` | ✅ | Diagnostic de complétude |
| `docs_diagram_get` | ✅ | Lire un diagramme mermaid attaché |
| `docs_update` | ❌ mutation | Créer / mettre à jour une entrée |
| `docs_delete` | ❌ mutation | Supprimer une entrée (refuse l'overview) |
| `docs_diagram_set` | ❌ mutation | Attacher / mettre à jour un diagramme |

> Tous les tools MCP de cette app sont déjà contextualisés sur `app_id = "{slug}"` —
> tu n'as pas besoin de le passer explicitement.

## 3. Taxonomie (essentielle — distingue clairement les 3 catégories)

| `type` | Quand l'utiliser | Champ `scope` |
|---|---|---|
| `overview` | UNE entrée par app : pitch utilisateur, archi, index. `name = "overview"`. | — |
| `screen` | UNE page / un écran de l'UI utilisateur (Login, Dashboard, Profile). | — |
| `feature` (`scope = "global"`) | Capacité TRANSVERSE qui touche ≥ 2 écrans (auth, notifications, i18n, theming, recherche globale). | `global` |
| `feature` (`scope = "screen:<name>"`) | Capacité propre à UN écran (ex: « éditer profil » sur l'écran Profile). | `screen:<name>` |
| `component` | Composant UI réutilisable indépendant des écrans (Button, Modal, Chart). | — |

**Règle de classification** : si une feature touche au moins 2 écrans → `scope = "global"`.
Sinon → `scope = "screen:<name>"`. Le champ `parent_screen` est dérivé automatiquement
quand `scope = "screen:<name>"`.

## 4. Templates skeleton

### Overview (`type=overview`, `name=overview`)
```markdown
# Vue d'ensemble — <App>

## Pitch utilisateur (3 phrases max)

## Architecture (1 paragraphe + diagramme global mermaid)

## Index
- Écrans : Login, Dashboard, Settings
- Features globales : Authentification, Notifications
- Composants clés : Sidebar, Card
```

### Screen (`type=screen`)
```markdown
# <Nom écran>

**Route** : `/path`
**Rôle utilisateur** : 1-2 phrases sur ce que l'utilisateur fait ici.

## Données affichées
- ...

## Features rattachées
- (références dans les `links` du frontmatter)

## États / transitions
- ...
```

### Feature (`type=feature`)
```markdown
# <Nom feature>

**Description utilisateur** : ce que l'utilisateur peut faire (orienté usage, pas implé).

## Flux
- déclencheur → action → résultat (rendu en mermaid via docs_diagram_set)

## Écrans concernés
- (depuis frontmatter `links`)

## Backend touché (synthèse user-facing)
- endpoints, règles métier visibles côté utilisateur
```

### Component (`type=component`)
```markdown
# <Nom composant>

**Rôle utilisateur** : ce qu'il rend possible.

**Props** : liste courte
**Utilisé par** : écrans / features (depuis `links`)
**Variants** : ...
```

## 5. Frontmatter (passé via le param `frontmatter` de `docs_update`)

```json
{{
  "title": "Connexion",
  "summary": "≤120 chars, affiché dans l'index compact",
  "scope": "global",                     // features uniquement
  "parent_screen": "login",              // si scope=screen:<name>
  "code_refs": ["apps/{slug}/src/routes/auth.rs:1-80"],
  "links": ["screen:login", "component:auth-form"]
}}
```

`title` et `summary` sont essentiels — ils alimentent l'index compact retourné par
`docs_overview`. Un agent qui ouvre l'app pour la première fois LIT cet index avant tout
le reste : si `summary` est vide, il est aveugle.

## 6. Bonnes pratiques mermaid

- Header : `flowchart LR` (lecture gauche-droite) ou `flowchart TD` (top-down). **Pas** le
  vieux `graph`.
- **Boîtes carrées uniquement** : nœuds en `[Texte lisible]` (rectangles). Pas de cercles
  ni de losanges sauf décision explicite.
- Flèches simples : `-->` avec label optionnel `-->|label|`.
- IDs en kebab-case (`user-input`), labels humains.
- **Max 12 nœuds par diagramme**. Si dépassé, découper en plusieurs diagrammes (overview =
  vue large ; feature = zoom).
- Pas d'icônes, pas de couleurs custom (le rendu utilise le thème dark global).
- Un `subgraph` pour grouper si > 6 nœuds, sinon flat.

Exemple cible (à coller via `docs_diagram_set`) :
```mermaid
flowchart LR
  user[Utilisateur] --> form[Formulaire login]
  form --> api[POST /api/auth]
  api --> session[Session créée]
  session --> dash[Dashboard]
```

## 7. Règles de mise à jour

- **Nouvel écran** → créer entrée `screen` + ajouter le lien depuis l'overview.
- **Nouvelle feature** → créer entrée `feature` avec scope correct + lier depuis le(s)
  écran(s) concerné(s) via `links`.
- **Modification d'un flux** → régénérer le diagramme via `docs_diagram_set`.
- **Doc incohérente avec le code que tu lis** → corriger la doc dans le même PR / commit.
- **Style** : descriptions orientées utilisateur (« ce qu'il peut faire »), JAMAIS
  l'implémentation (« composant React useState fetch... »).

## 8. Cycle d'exemple

> Tâche utilisateur : « Ajoute un bouton "Mot de passe oublié" sur l'écran login. »

1. `docs_overview` → je vois qu'il y a un écran `login` et une feature globale `auth`.
2. `docs_get(type=screen, name=login)` → je lis le rôle, les états, les liens.
3. `docs_get(type=feature, name=auth-login)` → je vois le flux actuel.
4. `docs_diagram_get(type=feature, name=auth-login)` → je lis le mermaid.
5. Exploration code, modif.
6. `docs_update(type=feature, name=auth-password-reset, scope="screen:login", ...)` →
   je crée la nouvelle feature.
7. `docs_diagram_set(type=feature, name=auth-password-reset, mermaid="flowchart LR\n...")` →
   nouveau flux.
8. `docs_update(type=screen, name=login, ...)` → j'ajoute le lien vers la nouvelle feature
   dans `links`.
9. `docs_completeness` → je vérifie que mon nouveau summary est rempli.
"#,
        name = app.name,
        slug = app.slug,
    )
}

/// Self-contained bash script that runs the build LOCALLY on CloudMaster
/// (where the agent + sources live) and emits status events to the Studio's
/// per-app live panel via the homeroute API on Medion.
///
/// The agent sees the cargo/pnpm output streamed in its terminal; the Studio
/// sees only the start/end milestones (no log forwarding).
fn render_app_build_script(app: &Application) -> String {
    let build_command = app
        .build_command
        .as_deref()
        .unwrap_or("echo 'no build_command configured'; exit 1");
    let template = r#"#!/usr/bin/env bash
# Build local de l'app `__SLUG__` (sources et toolchain sur CloudMaster).
# Émet des events au Studio (panel per-app live) via /api/apps/__SLUG__/build-event.
# Géré par Atelier — ne pas éditer (régénéré à chaque AppUpdate).
set -euo pipefail
API_BASE="${API_BASE:-http://10.0.0.254:4100}"
SLUG="__SLUG__"
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

emit() {
  curl -sS --max-time 5 -X POST "$API_BASE/api/apps/$SLUG/build-event" \
    -H 'content-type: application/json' \
    -d "$1" >/dev/null 2>&1 || true
}

on_err() {
  ec=$?
  ELAPSED_MS=$(( ($(date +%s) - START) * 1000 ))
  emit "{\"status\":\"error\",\"phase\":\"compile\",\"duration_ms\":$ELAPSED_MS,\"error\":\"build exited $ec\"}"
  exit $ec
}
trap on_err ERR

emit '{"status":"started","phase":"compile","message":"local build on cloudmaster"}'
START=$(date +%s)
echo "=== Build local: $SLUG ==="
echo "Cwd: $SRC_DIR"
cd "$SRC_DIR"
export CI=true NPM_CONFIG_FUND=false
__BUILD_COMMAND__
ELAPSED_MS=$(( ($(date +%s) - START) * 1000 ))
emit "{\"status\":\"finished\",\"phase\":\"compile\",\"duration_ms\":$ELAPSED_MS,\"message\":\"build OK (local)\"}"
echo "=== Build OK ($ELAPSED_MS ms) ==="
echo "Pour livrer en prod : bash .claude/skills/app-deploy/deploy.sh"
"#;
    template
        .replace("__SLUG__", &app.slug)
        .replace("__BUILD_COMMAND__", build_command)
}

/// Skill `app-deploy` : pousse les artefacts pre-buildés vers Medion + restart.
fn render_app_deploy_script(app: &Application) -> String {
    let template = r#"#!/usr/bin/env bash
# Deploy de l'app `__SLUG__` : envoie les artefacts pre-buildés vers Medion + restart.
# Pré-requis : avoir lancé `bash .claude/skills/app-build/build.sh` au préalable.
# Géré par Atelier — ne pas éditer.
set -euo pipefail
API_BASE="${API_BASE:-http://10.0.0.254:4100}"
TIMEOUT_SECS="${1:-900}"
curl -sS --max-time "$TIMEOUT_SECS" -X POST \
  "$API_BASE/api/apps/__SLUG__/ship" \
  -H 'content-type: application/json' \
  -d "{\"timeout_secs\":${TIMEOUT_SECS}}"
"#;
    template.replace("__SLUG__", &app.slug)
}

fn render_app_deploy_skill(app: &Application) -> String {
    format!(
        "---\n\
         name: app-deploy\n\
         description: Livre les artefacts pre-buildés de l'app `{slug}` vers Medion (rsync + restart). Utilise cette skill QUAND l'utilisateur demande de déployer/livrer/ship cette app — APRÈS un build local réussi.\n\
         allowed-tools: Bash(bash .claude/skills/app-deploy/deploy.sh*)\n\
         ---\n\
         \n\
         # Deploy de l'app `{slug}`\n\
         \n\
         Cette skill **livre** les artefacts (déjà compilés localement par `app-build`) vers Medion (10.0.0.254), puis redémarre le process supervisé. Pas de compile ici.\n\
         \n\
         ## Pré-requis\n\
         \n\
         - L'agent DOIT avoir lancé `bash .claude/skills/app-build/build.sh` avec succès dans cette session ou une précédente.\n\
         - Les artefacts (`build_artefact` de l'app) doivent exister sous `src/`.\n\
         \n\
         ## Commande\n\
         \n\
         ```bash\n\
         bash .claude/skills/app-deploy/deploy.sh\n\
         ```\n\
         \n\
         Timeout optionnel en secondes (défaut 900) :\n\
         \n\
         ```bash\n\
         bash .claude/skills/app-deploy/deploy.sh 1200\n\
         ```\n\
         \n\
         ## Retour\n\
         \n\
         JSON `{{ ok, stages, summary, duration_ms }}`. Étapes émises au Studio : `stop` → `rsync-back` → `restart`.\n\
         \n\
         ## Workflow type\n\
         \n\
         1. `bash .claude/skills/app-build/build.sh`  (build local sur CloudMaster, voir output cargo)\n\
         2. `bash .claude/skills/app-deploy/deploy.sh`  (livre + restart sur Medion)\n\
         3. Vérifier dans le panel Studio que l'app est `running`.\n\
         \n\
         ## Erreur HTTP 409 — BUILD_BUSY\n\
         \n\
         Un autre build/ship pour `{slug}` est déjà en cours. NE PAS RETRY automatiquement — informer l'utilisateur et attendre.\n",
        slug = app.slug,
    )
}

fn render_app_build_skill(app: &Application) -> String {
    use crate::types::AppStack;

    let stack_label = app.stack.display_name();
    let build_cmd = app
        .build_command
        .as_deref()
        .unwrap_or("(no build command configured)");

    let stack_section = match app.stack {
        AppStack::Axum => format!(
            "## Stack: Rust (Axum)\n\n\
             - Artefact attendu sous `src/` après build : `target/release/{slug}` (ou `build_artefact` si défini).\n\
             - Commande de build : `{cmd}`.\n",
            slug = app.slug,
            cmd = build_cmd,
        ),
        AppStack::AxumVite => format!(
            "## Stack: Rust (Axum) + Vite\n\n\
             - Artefacts attendus : `target/release/{slug}` + `web/dist/` (ou `build_artefact` si défini).\n\
             - Commande de build : `{cmd}`.\n",
            slug = app.slug,
            cmd = build_cmd,
        ),
        AppStack::NextJs => format!(
            "## Stack: Next.js\n\n\
             - Artefacts attendus : `.next/`, `public/`, `node_modules/` (ou `build_artefact` si défini).\n\
             - Commande de build : `{cmd}`.\n",
            cmd = build_cmd,
        ),
        AppStack::Flutter => format!(
            "## Stack: Flutter (mobile Android)\n\n\
             - Commande de build : `{cmd}` (publication via la règle `store-publishing.md`).\n",
            cmd = build_cmd,
        ),
    };

    format!(
        "---\n\
         name: app-build\n\
         description: Build local de l'application {slug} ({stack}) sur CloudMaster (toolchain locale, output cargo/pnpm visible en live). Utilise cette skill QUAND l'utilisateur demande de builder/compiler/rebuild cette app.\n\
         allowed-tools: Bash(bash .claude/skills/app-build/build.sh*)\n\
         ---\n\
         \n\
         # Build de l'app `{slug}` (local sur CloudMaster)\n\
         \n\
         Cette skill compile l'app **directement** sur CloudMaster (où vivent sources et toolchain). \
         L'output (cargo, pnpm, etc.) est visible en live dans ton terminal. Le Studio est notifié en parallèle via `/api/apps/{slug}/build-event` pour afficher l'état dans le panel per-app.\n\
         \n\
         **Important** : ce build NE LIVRE PAS l'artefact à Medion. Pour livrer + restart en prod, enchaîne ensuite avec :\n\
         \n\
         ```bash\n\
         bash .claude/skills/app-deploy/deploy.sh\n\
         ```\n\
         \n\
         ## Commande\n\
         \n\
         ```bash\n\
         bash .claude/skills/app-build/build.sh\n\
         ```\n\
         \n\
         Tu peux itérer sans deploy : edit → build → re-edit → re-build. Tant que tu n'as pas fait `app-deploy`, le runtime Medion ne change pas (c'est volontaire).\n\
         \n\
         ## Retour\n\
         \n\
         Le script affiche l'output cargo/pnpm en stream + un événement `started` puis `finished` (ou `error`) émis au Studio. Exit code 0 si le build passe.\n\
         \n\
         ## Workflow type\n\
         \n\
         1. `bash .claude/skills/app-build/build.sh`  (compile local, voir l'output)\n\
         2. Itérer si besoin (fix erreurs, re-build)\n\
         3. `bash .claude/skills/app-deploy/deploy.sh`  (livre + restart prod)\n\
         \n\
         ## Interdits\n\
         \n\
         - **JAMAIS** appeler les anciens endpoints `/api/apps/{slug}/build` ou `/api/apps/{slug}/deploy` à la main : ils refont l'aller-retour SSH inutile.\n\
         \n\
         {stack_section}",
        slug = app.slug,
        stack = stack_label,
        stack_section = stack_section,
    )
}

/// Remove a stale file silently. Logs at info if the file existed.
fn remove_if_exists(path: &Path, slug: &str) {
    match fs::remove_file(path) {
        Ok(()) => {
            info!(slug = %slug, file = %path.display(), "obsolete context file removed");
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            warn!(slug = %slug, file = %path.display(), error = %e, "failed to remove obsolete context file");
        }
    }
}

/// Rule toujours-active (`.claude/rules/app-info.md`) qui centralise l'identité
/// et les infos dynamiques de l'app. C'est l'ancien corps de CLAUDE.md, déplacé
/// hors du CLAUDE.md pour que celui-ci puisse devenir agent-owned (write-once).
fn render_app_info_md(
    app: &Application,
    all_apps: &[Application],
    db_tables: &Option<Vec<String>>,
) -> String {
    let url = format!("https://{}", app.domain);
    let visibility_label = match app.visibility {
        Visibility::Public => "Public (no auth required)",
        Visibility::Private => "Private (HomeRoute auth required)",
    };

    let db_section = render_db_section(app, db_tables);

    let env_var_section = if app.env_vars.is_empty() {
        "Aucune variable d'environnement custom déclarée. `PORT` est injecté automatiquement.".to_string()
    } else {
        let mut s = String::from("Variables d'environnement déclarées (injectées par le superviseur) :\n\n");
        for (k, _) in app.env_vars.iter() {
            s.push_str(&format!("- `{}`\n", k));
        }
        s.push_str("\n`PORT` est toujours injecté en plus.");
        s
    };

    let mut other_apps = String::from("## Autres apps du workspace\n\n");
    let mut has_others = false;
    for other in all_apps {
        if other.slug == app.slug {
            continue;
        }
        has_others = true;
        other_apps.push_str(&format!(
            "- **{name}** (`{slug}`) — {stack}, https://{domain}\n",
            name = other.name,
            slug = other.slug,
            stack = other.stack.display_name(),
            domain = other.domain,
        ));
    }
    if !has_others {
        other_apps.push_str("_(aucune autre app enregistrée pour l'instant)_\n");
    }

    let build_cmd = app.build_command.as_deref().unwrap_or("(no build step)");

    format!(
        "# {name} — informations\n\
         \n\
         > Ce fichier est **régénéré** à chaque `AppUpdate`/`AppRegenerateContext`/boot.\n\
         > Ne le modifie pas à la main — tes changements seraient écrasés.\n\
         > Pour tes propres notes, utilise `CLAUDE.md` (agent-owned).\n\
         \n\
         ## Identité\n\
         - **Nom :** {name}\n\
         - **Slug :** `{slug}`\n\
         - **Stack :** {stack}\n\
         - **URL publique :** {url} ({visibility})\n\
         - **Port interne :** {port}\n\
         - **Health check :** `{health}`\n\
         - **Commande de run :** `{run}`\n\
         - **Commande de build (CloudMaster) :** `{build}`\n\
         - **Dossier source (workspace) :** `{src_dir}`\n\
         \n\
         ## Base de données\n\
         {db}\n\
         \n\
         ## Environnement\n\
         {env}\n\
         \n\
         {others}",
        name = app.name,
        slug = app.slug,
        stack = app.stack.display_name(),
        url = url,
        visibility = visibility_label,
        port = app.port,
        health = app.health_path,
        run = app.run_command,
        build = build_cmd,
        src_dir = app.src_dir().display(),
        db = db_section,
        env = env_var_section,
        others = other_apps,
    )
}

/// Skeleton initial écrit dans `src/CLAUDE.md` **une seule fois** à la création
/// de l'app. Ensuite il appartient à l'agent qui l'enrichit au fil du temps.
/// Les règles comportementales vivent dans `.claude/rules/`, voir aussi la
/// rule `claude-md-upkeep.md` qui détaille ce qu'il faut (et ne faut pas) y
/// écrire.
fn render_initial_claude_md(app: &Application) -> String {
    format!(
        "# {name} — Carnet de bord\n\
         \n\
         > **DOC-FIRST** : avant toute tâche, appelle `docs_overview` pour avoir le \
         contexte business + l'index des écrans / features / composants. La doc est la \
         source de vérité de l'intention. Voir \
         [`.claude/rules/docs.md`](.claude/rules/docs.md).\n\
         \n\
         Ce fichier est **le tien** : architecture, décisions, apprentissages, \
         TODOs, pièges rencontrés. Lis d'abord \
         [`.claude/rules/claude-md-upkeep.md`](.claude/rules/claude-md-upkeep.md) \
         avant d'y ajouter du contenu.\n\
         \n\
         Les informations techniques dynamiques (stack, port, autres apps, env \
         vars, DB) sont dans [`.claude/rules/app-info.md`](.claude/rules/app-info.md) \
         — ne les recopie pas ici.\n\
         \n\
         ---\n\
         \n\
         _Ajoute tes notes sous cette ligne. HomeRoute ne réécrira jamais ce \
         fichier (sauf demande explicite via `AppRegenerateContext` avec un \
         futur flag `force_claude_md`)._\n",
        name = app.name,
    )
}

/// Rule obligatoire pour l'usage des tools MCP `todos_*` — les todos sont
/// visibles en live dans le panneau latéral droit du Studio de l'app.
fn render_todos_md(app: &Application) -> String {
    format!(
        "# Todos — {name} (OBLIGATOIRE)\n\
         \n\
         Cette app possède une todolist **vivante** scopée à `{slug}`, exposée via les \
         tools MCP `todos_*`. Elle s'affiche **en temps réel** dans le panneau latéral \
         droit du Studio ({slug}.mynetwk.biz via studio.mynetwk.biz). C'est un \
         compagnon de travail **visible par l'utilisateur**, pas ton bloc-notes interne.\n\
         \n\
         ## Réflexes obligatoires\n\
         \n\
         Ces trois moments ne sont pas négociables :\n\
         \n\
         1. **Au début de chaque session** — `todos_list` immédiatement. Toujours. Tu \
         prends connaissance de ce qui traîne avant d'attaquer.\n\
         2. **À chaque transition naturelle** — fin d'une étape, blocage, changement \
         de focus, après avoir résolu un truc : reconsulte la liste. Une tâche que tu \
         viens de finir est probablement **dedans** — `todos_delete` avant de poursuivre. \
         Ne laisse jamais un todo livré derrière toi entre deux étapes.\n\
         3. **Avant de rendre ton rapport final à l'utilisateur** — ouvre `todos_list` \
         une dernière fois. Tout ce qui est livré : `delete`. Tout ce qui n'a plus de \
         raison d'être : `delete`. La liste finale ne doit contenir QUE ce qui reste \
         réellement à faire ou en cours.\n\
         \n\
         Si tu termines un tour sans avoir fait ce dernier balayage, l'utilisateur \
         voit des fantômes dans son panneau. C'est un échec.\n\
         \n\
         ## Anti-patterns à proscrire\n\
         \n\
         - ❌ **Créé-puis-oublié** : tu crées un todo pour tracer un diagnostic, tu \
         fixes le problème dans la même session, tu rends le rapport sans toucher au \
         todo. → `todos_delete` **immédiatement** après la résolution, pas après le \
         rapport.\n\
         - ❌ **Hors scope** : tu bosses dans `{slug}`, tu découvres un follow-up qui \
         concerne `hr-apps`, `homeroute`, ou une autre app. Ne crée PAS un todo ici. \
         Ce panneau est dédié à `{slug}`. Mentionne le follow-up **dans la conversation \
         avec l'utilisateur** — c'est lui qui décidera où le tracer.\n\
         - ❌ **Surutilisation** : un todo s'adresse à **l'utilisateur qui regarde le \
         Studio**. Pour des notes purement techniques destinées au prochain agent \
         (chemins de code, hypothèses internes, conventions locales), `CLAUDE.md` à la \
         racine de l'app est mieux. Critère : « Est-ce que l'utilisateur veut savoir \
         que je laisse ça pour plus tard ? » Si non → pas un todo.\n\
         - ❌ **Statut décoratif** : pas de « done », « completed », « archived ». \
         Terminé = supprimé.\n\
         \n\
         ## Sémantique des statuts\n\
         \n\
         Seuls deux statuts existent :\n\
         \n\
         - **`pending`** — note « à penser plus tard ». Pas encore en cours.\n\
         - **`in_progress`** — la tâche que tu fais **maintenant**. **Une seule à la \
         fois**. Si tu démarres un nouveau todo sans clore le précédent, le backend \
         demote automatiquement l'ancien à `pending` — évite-toi la surprise et fais-le \
         consciemment.\n\
         \n\
         Une tâche **terminée** est **supprimée** (`todos_delete`). Pas de status \
         « done ». Une tâche dont la suppression est demandée par l'utilisateur est \
         aussi supprimée — pas marquée d'une autre façon.\n\
         \n\
         ## Mapping action → tool (référence)\n\
         \n\
         - **Démarrer une tâche** → `todos_update(id, status: \"in_progress\")` ou \
         `todos_create(name, description)` puis update.\n\
         - **Tâche terminée** → `todos_delete(id)` ⚠ supprimer, ne pas « compléter ».\n\
         - **Note / follow-up à garder en tête** → `todos_create(name, description)` \
         (le statut par défaut est `pending`).\n\
         - **Progrès partiel sur in_progress** → `todos_update(id, description)` avec \
         des notes.\n\
         - **Suppression demandée par l'utilisateur** → `todos_delete(id)`.\n\
         \n\
         ## Tools MCP\n\
         \n\
         | Tool | Usage |\n\
         |---|---|\n\
         | `todos_list` | Lister (filtre optionnel par `status`) |\n\
         | `todos_create` | Créer (`name`, `description?`) — démarre en `pending` |\n\
         | `todos_update` | Modifier (`id`, puis les champs à changer) |\n\
         | `todos_delete` | Supprimer (`id`) — c'est ainsi qu'on clôt |\n\
         \n\
         Le `slug` de l'app (`{slug}`) est injecté automatiquement par le MCP projet — \
         ne le passe pas dans les arguments.\n",
        name = app.name,
        slug = app.slug,
    )
}

/// Rule statique (même contenu pour toutes les apps) qui documente le rôle de
/// `CLAUDE.md` et sa relation aux règles de `.claude/rules/`.
fn render_claude_md_upkeep_md() -> String {
    "# Maintenance de CLAUDE.md — règle obligatoire\n\
     \n\
     `CLAUDE.md` (à la racine du workspace) est le **carnet de bord du projet** : \
     décisions d'architecture, apprentissages, TODOs non-bloquants, pièges connus, \
     conventions locales spécifiques à cette app. Il t'appartient — tu dois le \
     tenir à jour.\n\
     \n\
     ## Quand mettre à jour CLAUDE.md\n\
     \n\
     - Nouvelle décision d'architecture ou refactor significatif.\n\
     - Piège ou edge-case non évident rencontré (« pourquoi cette ligne bizarre »).\n\
     - Convention locale établie (nommage, structure d'un dossier, pattern récurrent).\n\
     - TODO technique que tu ne traites pas maintenant.\n\
     - Lien utile (issue, PR, doc externe) qu'un futur agent devra connaître.\n\
     \n\
     Ajoute une section datée `## YYYY-MM-DD — titre court`. Préfère condenser \
     ou supprimer une vieille section plutôt que laisser s'accumuler du bruit.\n\
     \n\
     ## Ce qui ne doit PAS aller dans CLAUDE.md\n\
     \n\
     Les règles opérationnelles de l'app vivent dans `.claude/rules/` (source de \
     vérité). **Ne les recopie jamais dans CLAUDE.md** — tu créerais des \
     divergences. Si tu dois les citer, référence leur chemin :\n\
     \n\
     - Identité, stack, port, domaine, autres apps → [`app-info.md`](app-info.md) (**régénéré automatiquement**)\n\
     - Tools MCP disponibles → [`mcp-tools.md`](mcp-tools.md)\n\
     - Workflow (tests, lint, deploy, interdits) → [`workflow.md`](workflow.md)\n\
     - Documentation partagée (`docs.*`) → [`docs.md`](docs.md)\n\
     - Build → skill `app-build`\n\
     \n\
     ## Style\n\
     \n\
     - Sections courtes, datées, orientées « futur toi-même qui ouvre le projet \
       dans 3 mois ».\n\
     - Pas de duplication avec les rules ci-dessus.\n\
     - Le ton est libre mais précis : privilégie les faits et les décisions aux \
       narrations.\n"
        .to_string()
}

fn render_flows_first_rule(app: &Application) -> String {
    // Stack-aware : NextJS and Rust apps share the same flow doctrine but
    // wire actions/connectors via different toolchains. We keep both bodies
    // close in shape so reading two is cheap, and avoid `format!` so the
    // literal `{{ ... }}` TOML placeholders don't need escaping.
    if matches!(app.stack, AppStack::NextJs) {
        render_flows_first_rule_next(&app.slug)
    } else {
        render_flows_first_rule_rust(&app.slug)
    }
}

fn render_flows_first_rule_rust(slug: &str) -> String {
    r#"# Flows-first — orchestration via daemon `hr-flowd`

Cette app utilise le moteur **`hr-flow`** comme **daemon partagé** (`hr-flowd`, Medion `127.0.0.1:4002`, loopback only) — équivalent Power Automate, natif Rust. Toute logique métier qui chaîne plusieurs étapes doit s'exprimer comme un **flux**, pas comme du Rust écrit à la main dans un handler.

## État de la migration

Le pilote **Wallet** a validé la plateforme (engine, persistence, MCP, Studio, stats) et a basculé en **mode callback** (Phase 4 livrée, 2026-05-09). Cette app (`{slug}`) est **éligible** au mode callback dès maintenant — on saute la phase embedded, on attaque directement le daemon partagé.

Selon où en est cette app :

- **Pas encore intégrée à hr-flow** : `Cargo.toml` du serveur n'a ni `hr-flow` ni `hr-flow-callback`, pas de `flows/` ni de `flows::register_callbacks()`. → Quand l'utilisateur te donnera le **prompt de migration** (commence par "Migration vers hr-flow"), suis-le. Le prompt complet vit dans `/nvme/atelier/docs/migration-prompts/rust.md`.
- **Déjà intégrée et au moins une route migrée** : tu peux ajouter de nouveaux flux et migrer le reste des routes existantes. Réfère-toi au skill `flow-build` (lazy-loaded, demande-le au besoin).

**Tant que l'utilisateur n'a pas explicitement lancé la migration**, ne crée pas de flux spontanément — finis ce qu'il te demande. Mais quand tu écris du nouveau code orchestrant ≥ 2 étapes, **mentionne** que ça pourrait être un flux et propose-le.

## Architecture callback (le modèle actuel)

Le daemon orchestre, l'app héberge ses actions/connecteurs custom. Contrat :

```
hr-flowd (4002) ──HTTP POST──► <app>:<port>/_flow/action/{name}
                                Authorization: Bearer <HR_FLOW_TOKEN>
                                X-HomeRoute-Flow: 1
                                Body: { run_id, step_id, input, params }

                              ► <app>:<port>/_flow/connector/{name}/{op}
                                (idem, params au lieu de input)

                              ◄ 200 { "output": ... }
                                ou { "error": "...", "kind": "..." }
```

Côté app Rust, le sous-router est créé par `hr_flow_callback::router()` et merged dans le router axum principal. Les actions `#[flow_action]` génèrent automatiquement deux fonctions sœurs :
- `register_<fn>(&mut FlowEngineBuilder)` — legacy embedded (sera retiré Phase 6)
- `mount_<fn>(CallbackRouter) -> CallbackRouter` — wiring callback (utilisation actuelle)

## ⛔ Escalade plateforme — règle stricte

Si tu rencontres un **bug** ou une **limitation** dans `hr-flow` / `hr-flow-callback` / `hr-flowd` (engine, primitives, expressions, connecteurs managés, persistence, MCP, callback) :

1. **STOP. NE CONTOURNE PAS.** Pas d'action Rust custom pour faire le travail d'une primitive manquante. Pas de hack qui maquille le bug.
2. **Produis un rapport structuré** dans la conversation, en français, avec :
   - **Sévérité** : `P0` (bloque la migration), `P1` (force un workaround moche), `P2` (ergonomie).
   - **Contexte** : quel flux, quel step, quelle valeur d'input réelle.
   - **Repro minimale** : extrait TOML ou expression qui déclenche le problème, ce que tu attends, ce qui se passe.
   - **Hypothèse** sur la cause si tu en as une.
3. **Attends le correctif**. La plateforme `hr-flow` est maintenue dans le repo Atelier (`/nvme/atelier/crates/hr-flow*`) par un agent dédié. Le user transmet le rapport, l'agent plateforme corrige et redéploie, puis te dit de reprendre.
4. **Pas de TODO bidouille pendant l'attente** : passe à un autre lot de routes ou à une autre tâche. Tu reprendras le flux problématique quand le correctif sera là.

Cette règle existe parce que les workarounds polluent durablement le code de l'app et masquent les vrais besoins de la plateforme. La plateforme s'améliore *parce qu'on lui remonte les manques* — pas en les évitant.

## Quand créer un flux

Crée un flux dès que tu as :

- une logique avec ≥ 2 étapes (ex : appel HTTP + transformation + écriture en base) ;
- une orchestration que tu voudras pouvoir **rejouer** ou **inspecter step-by-step** ;
- un branchement, une boucle ou une accumulation portant sur des données métier.

Le code Rust de l'app reste l'**entrée du monde réel** (route HTTP, cron, webhook). Il invoque ensuite le flux via :
- `RemoteEngine::from_env(slug)?.run(name, input).await` (côté serveur de l'app, en interne), ou
- `POST http://127.0.0.1:4100/api/apps/<slug>/flows/<name>/run` (route Atelier API, depuis n'importe où loopback).

## Hiérarchie de choix

Pour chaque étape d'un flux, choisis dans cet ordre :

1. **Connecteur managé daemon-side** (`http`) — l'option par défaut pour parler à n'importe quelle API HTTP.
2. **Connecteur custom de l'app** (déclaré via `with_connector` dans `register_callbacks`, ex : `openrouter`, `dataverse`-app-specific) — quand un service tiers est utilisé plus d'une fois OU quand l'app porte des credentials/contexte de connexion non-portables au daemon.
3. **Primitive** (`if`, `for_each`, `set_var`, `compose`, `increment_var`, etc.) — toute la logique de contrôle et les transformations légères.
4. **Action Rust custom** (`#[flow_action]`) — **dernier recours**, uniquement pour :
   - un calcul pur, isolé, type-safe (algorithme, scoring, parsing) ;
   - un wrapper autour d'un service éphémère qui ne mérite pas un connecteur dédié.

Une action Rust est **opaque** dans la timeline du run : tu vois son input et son output, mais pas ce qui se passe dedans. C'est pour ça qu'elle doit rester strictement une **fonction pure**.

## Anti-patterns à proscrire

- ❌ Une boucle, une condition ou une accumulation **dans** une action Rust → sors-la en `for_each`/`if`/`set_var` ; sinon le run trace n'est plus auditable.
- ❌ Un handler HTTP qui fait `dataverse_query → openrouter_call → dataverse_update` en code direct au lieu de déléguer à un flux → tu perds traçabilité, replay, drilldown Studio.
- ❌ Une action Rust nommée `process_all`/`do_everything` → c'est un signal qu'il y a du contrôle de flux caché dedans.
- ❌ Réintroduire `FlowEngineBuilder` dans cette app pour faire de l'embedded — Phase 6 retire ce chemin, on n'y retourne pas.

## Nommage des steps — règle stricte

Les `id` de steps apparaissent tels quels dans la timeline du run. Ils doivent **se lire** : un humain qui ouvre un run sans contexte doit comprendre ce que fait chaque ligne sans cliquer.

- ✅ **Verbe en snake_case** qui décrit l'intention métier :
  - `fetch_uncategorized_transactions`
  - `classify_with_ai`
  - `apply_categories_to_db`
  - `compute_monthly_summary`
  - `partition_by_recurrence`
- ❌ **Génériques numérotés** (vieille logique Power Automate) :
  - `lookup1`, `if_exists1`, `do_insert1`, `step_3`, `action`, `loop`, `branch`
- ❌ **Le kind comme nom** : un step `kind = "if"` ne s'appelle pas `if`, il s'appelle `if_above_threshold` ou `if_already_exists`.

Cas particulier des steps répétés : si tu boucles avec `for_each` et qu'un step à l'intérieur est invoqué N fois, garde un nom singulier descriptif (`upsert_recommendation`, pas `upsert1`/`upsert2`/…). L'`iteration_index` distingue déjà les exécutions dans la trace.

Pour les variables (`set_var.name`, `append_to_var.name`) : même règle. `total`, `flag`, `tmp` sont à proscrire — préfère `total_high_risk_eur`, `has_pending_imports`.

## Format des flux

Les flux vivent en TOML **plat** sous `flows/*.toml`. Une étape par bloc `[[steps]]` ; le nesting passe par `parent = "id_parent"` et `parent_branch = "then" | "else" | "case:..."` (jamais d'imbrication TOML profonde).

```toml
name = "categorize_transaction"

[[steps]]
id = "fetch"
kind = "connector"
connector = "dataverse"
op = "find"
table = "transactions"
filter = "id == @input.transaction_id"

[[steps]]
id = "branch"
kind = "if"
cond = "{{ steps.fetch.output.amount }} > 1000"

[[steps]]
id = "ai"
parent = "branch"
parent_branch = "then"
kind = "connector"
connector = "openrouter"
op = "chat"
# ...
```

## Outils

- `mcp__homeroute__flow.list_definitions(slug=...)` / `flow.get_definition` — lecture des flux
- `mcp__homeroute__flow.run(slug, name, input)` — déclencher un flux à la main
- `mcp__homeroute__flow.list_runs` / `flow.get_run` — lecture de l'historique avec arbre des steps
- `mcp__homeroute__flow.replay(slug, run_id)` — rejouer un run en l'état
- `POST http://127.0.0.1:4100/api/apps/<slug>/flows/<name>/run` — Atelier API write endpoint (depuis le code Rust de l'app)
- Studio onglet **Flows** — visualisation lecture seule pour l'utilisateur

Le skill `flow-build` (lazy-loaded) couvre le scaffold, le lint et le debug en détail.

## Variables d'environnement (côté app)

```
HR_FLOW_TOKEN=<32-bytes-hex>   # bearer partagé entre daemon et app pour les callbacks
```

À provisionner dans le `.env` canonique de l'app sur CloudMaster, avec le même token côté `apps.json::flow_callback_token` sur Medion.
"#.replace("{slug}", slug)
}

fn render_flows_first_rule_next(slug: &str) -> String {
    r#"# Flows-first — orchestration via daemon `hr-flowd` (NextJS)

Cette app utilise le moteur **`hr-flow`** comme **daemon partagé** (`hr-flowd`, Medion `127.0.0.1:4002`, loopback only) — équivalent Power Automate. Toute logique métier qui chaîne plusieurs étapes doit s'exprimer comme un **flux**, pas comme du TS écrit à la main dans un handler Next.

Pas de Rust dans ta toolchain : les actions custom sont des fonctions TS pures, exposées via une route catchall Next vers le daemon.

## État de la migration

Le pilote **Wallet** (Rust) a validé la plateforme et le daemon `hr-flowd` tourne en prod (Phase 4 livrée, 2026-05-09). Cette app (`{slug}`) est **éligible** au mode callback dès qu'on lance la migration. C'est la **première fois** qu'elle touche aux flux.

Selon où en est cette app :

- **Pas encore intégrée à hr-flow** : pas de `flows/`, pas de route `app/api/_flow/[type]/[name]/route.ts`, pas de dep `@homeroute/flow-action` dans `package.json`. → Quand l'utilisateur te donnera le **prompt de migration** (commence par "Migration vers hr-flow"), suis-le. Le prompt complet vit dans `/nvme/atelier/docs/migration-prompts/next.md`.
- **Déjà intégrée et au moins une route migrée** : tu peux ajouter de nouveaux flux et migrer le reste des routes existantes. Réfère-toi au skill `flow-build` (lazy-loaded, demande-le au besoin).

**Tant que l'utilisateur n'a pas explicitement lancé la migration**, ne crée pas de flux spontanément — finis ce qu'il te demande. Mais quand tu écris du nouveau code orchestrant ≥ 2 étapes, **mentionne** que ça pourrait être un flux et propose-le.

## Architecture callback (le modèle)

Le daemon orchestre, l'app héberge ses actions / connecteurs custom en TS. Contrat :

```
hr-flowd (4002) ──HTTP POST──► <app>:<port>/api/_flow/action/{name}
                                Authorization: Bearer <HR_FLOW_TOKEN>
                                X-HomeRoute-Flow: 1
                                Body: { run_id, step_id, input, params }

                              ► <app>:<port>/api/_flow/connector/{name}/{op}
                                (idem, params au lieu de input)

                              ◄ 200 { "output": ... }
                                ou { "error": "...", "kind": "..." }
```

Côté app NextJS, la route catchall `app/api/_flow/[type]/[name]/route.ts` (et éventuellement `[op]/route.ts` plus profond pour les connecteurs) est servie par `handleFlowCallback({ actions, connectors })` du package `@homeroute/flow-action`. Validation bearer + isolement panique + sérialisation d'erreur sont gérées par le helper.

**Important** : la route DOIT spécifier `export const runtime = 'nodejs';` — Edge runtime ne fournit pas `crypto.timingSafeEqual`.

## ⛔ Escalade plateforme — règle stricte

Si tu rencontres un **bug** ou une **limitation** dans `hr-flow` / `@homeroute/flow-action` / `hr-flowd` (engine, primitive, expression, callback contract, persistence) :

1. **STOP. NE CONTOURNE PAS.** Pas d'action TS qui contourne une primitive manquante. Pas de hack qui maquille le bug.
2. **Produis un rapport structuré** dans la conversation, en français, avec :
   - **Sévérité** : `P0` (bloque la migration), `P1` (force un workaround moche), `P2` (ergonomie).
   - **Contexte** : quel flux, quel step, quelle valeur d'input réelle.
   - **Repro minimale** : extrait TOML ou expression qui déclenche le problème, ce que tu attends, ce qui se passe.
   - **Hypothèse** sur la cause si tu en as une.
3. **Attends le correctif**. La plateforme `hr-flow` est dans `/nvme/atelier/crates/hr-flow*` + `/nvme/atelier/web/packages/flow-action/`, maintenue par un agent dédié. Le user transmet le rapport, l'agent plateforme corrige et redéploie, puis te dit de reprendre.
4. **Pas de TODO bidouille pendant l'attente** : passe à un autre lot de routes ou à une autre tâche. Tu reprendras le flux problématique quand le correctif sera là.

Cette règle existe parce que les workarounds polluent durablement le code de l'app et masquent les vrais besoins de la plateforme. La plateforme s'améliore *parce qu'on lui remonte les manques* — pas en les évitant.

## Quand créer un flux

Crée un flux dès que tu as :

- une logique avec ≥ 2 étapes (ex : appel HTTP + transformation + écriture en base) ;
- une orchestration que tu voudras pouvoir **rejouer** ou **inspecter step-by-step** ;
- un branchement, une boucle ou une accumulation portant sur des données métier.

Le code TS de l'app reste l'**entrée du monde réel** (route HTTP, cron, webhook). Il invoque ensuite le flux via :
- `POST http://127.0.0.1:4100/api/apps/<slug>/flows/<name>/run` (Atelier API write endpoint, recommandé)
- ou `POST http://127.0.0.1:4002/v1/runs` directement (avec bearer `HR_FLOWD_TOKEN`)

## Hiérarchie de choix

Pour chaque étape d'un flux, choisis dans cet ordre :

1. **Connecteur managé daemon-side** (`http`) — l'option par défaut pour parler à n'importe quelle API HTTP.
2. **Connecteur custom de l'app** (object map dans `handleFlowCallback({ connectors })`) — quand un service tiers est utilisé plus d'une fois OU porte des credentials propres à l'app.
3. **Primitive** (`if`, `for_each`, `set_var`, `compose`, `increment_var`, etc.) — toute la logique de contrôle et les transformations légères.
4. **Action TS custom** (`FlowAction` dans `lib/flow-actions/<name>.ts`) — **dernier recours**, uniquement pour :
   - un calcul pur, isolé (algorithme, parsing, scoring) ;
   - un wrapper autour d'un service éphémère qui ne mérite pas un connecteur dédié.

Une action TS est **opaque** dans la timeline du run : tu vois son input et son output, mais pas ce qui se passe dedans. C'est pour ça qu'elle doit rester strictement **pure** (pas d'I/O caché, pas d'effet de bord global).

## Anti-patterns à proscrire

- ❌ Une boucle, une condition ou une accumulation **dans** une action TS → sors-la en `for_each`/`if`/`set_var` ; sinon le run trace n'est plus auditable.
- ❌ Un route Next qui fait `db_query → openai_call → db_update` en code direct au lieu de déléguer à un flux → tu perds traçabilité, replay, drilldown Studio.
- ❌ Une action TS nommée `process_all`/`do_everything` → c'est un signal qu'il y a du contrôle de flux caché dedans.
- ❌ Oublier `export const runtime = 'nodejs'` — la route va 500 silencieusement en Edge runtime.

## Nommage des steps — règle stricte

Les `id` de steps apparaissent tels quels dans la timeline du run. Ils doivent **se lire** : un humain qui ouvre un run sans contexte doit comprendre ce que fait chaque ligne sans cliquer.

- ✅ **Verbe en snake_case** qui décrit l'intention métier :
  - `fetch_uncategorized_transactions`
  - `classify_with_ai`
  - `apply_categories_to_db`
  - `compute_monthly_summary`
  - `partition_by_recurrence`
- ❌ **Génériques numérotés** : `lookup1`, `if_exists1`, `do_insert1`, `step_3`, `action`, `loop`, `branch`
- ❌ **Le kind comme nom** : un step `kind = "if"` ne s'appelle pas `if`, il s'appelle `if_above_threshold`.

Cas particulier des steps répétés (`for_each`) : nom singulier descriptif (`upsert_recommendation`, pas `upsert1`/`upsert2`/…). L'`iteration_index` distingue déjà les exécutions dans la trace.

Pour les variables (`set_var.name`, `append_to_var.name`) : même règle. `total`, `flag`, `tmp` à proscrire — préfère `total_high_risk_eur`, `has_pending_imports`.

## Format des flux

Les flux vivent en TOML **plat** sous `flows/*.toml` à la racine de l'app (pas sous `app/`). Une étape par bloc `[[steps]]` ; le nesting passe par `parent = "id_parent"` et `parent_branch = "then" | "else" | "case:..."` (jamais d'imbrication TOML profonde).

```toml
name = "categorize_transaction"

[[steps]]
id = "fetch"
kind = "connector"
connector = "http"
op = "get"
url = "https://api.example.com/transactions/{{ input.id }}"

[[steps]]
id = "branch"
kind = "if"
cond = "{{ steps.fetch.output.amount }} > 1000"

[[steps]]
id = "classify"
parent = "branch"
parent_branch = "then"
kind = "action"
action = "compute_score"
params = { amount = "{{ steps.fetch.output.amount }}" }
```

## Outils

- `mcp__homeroute__flow.list_definitions(slug=...)` / `flow.get_definition` — lecture des flux
- `mcp__homeroute__flow.run(slug, name, input)` — déclencher un flux à la main
- `mcp__homeroute__flow.list_runs` / `flow.get_run` — lecture de l'historique avec arbre des steps
- `mcp__homeroute__flow.replay(slug, run_id)` — rejouer un run en l'état
- `POST http://127.0.0.1:4100/api/apps/<slug>/flows/<name>/run` — Atelier API write endpoint (depuis le code TS de l'app, en interne)
- Studio onglet **Flows** — visualisation lecture seule pour l'utilisateur

Le skill `flow-build` (lazy-loaded) couvre le scaffold, le lint et le debug en détail.

## Variables d'environnement (côté app)

```
HR_FLOW_TOKEN=<32-bytes-hex>   # bearer partagé entre daemon et app pour les callbacks
```

À provisionner dans `.env.local` (ou `.env`, selon la convention de l'app) sur CloudMaster, avec le même token côté `apps.json::flow_callback_token` sur Medion.
"#.replace("{slug}", slug)
}

fn render_store_publishing_md(app: &Application) -> String {
    format!(
        "# Publication Store — Règles obligatoires\n\
         \n\
         Pour publier une nouvelle version de cette app Flutter dans le store mobile HomeRoute, \
         **toujours** utiliser le tool MCP `store.upload`. Jamais un `curl` manuel vers \
         `/api/store/apps/{slug}/releases`.\n\
         \n\
         ## Workflow\n\
         \n\
         1. Build APK sur CloudMaster :\n\
         ```\n\
         export PATH=/ssd_pool/flutter/bin:$PATH\n\
         flutter build apk --release\n\
         ```\n\
         2. Encoder en base64 :\n\
         ```\n\
         base64 -w0 build/app/outputs/flutter-apk/app-release.apk > /tmp/app.b64\n\
         ```\n\
         3. Appeler `store.upload` (tool MCP) avec :\n\
         - `slug` : `{slug}`\n\
         - `version` : la version à publier (ex: `1.2.3`)\n\
         - `apk_base64` : contenu du fichier `.b64`\n\
         \n\
         4. Vérifier via `store.get` (slug `{slug}`) que la nouvelle version apparaît avec \
         `sha256` et `size_bytes` renseignés.\n\
         \n\
         ## Arguments optionnels (store.upload)\n\
         \n\
         | Argument | Header HTTP | Usage |\n\
         |---|---|---|\n\
         | `app_name` | `X-App-Name` | **Requis au premier upload** de ce slug |\n\
         | `description` | `X-App-Description` | Description affichée dans le store |\n\
         | `category` | `X-App-Category` | Défaut : `other` |\n\
         | `changelog` | `X-Changelog` | Notes de version |\n\
         | `publisher_app_id` | `X-Publisher-App-Id` | Lien vers une app publisher |\n\
         \n\
         Package Android, SHA256, taille et icône sont extraits automatiquement de l'APK côté API.\n\
         \n\
         ## Limites\n\
         \n\
         - Payload max : 500 MB.\n\
         - Pas de streaming ni de upload par chunks.\n\
         - Suppression de release : via API REST directement, pas via MCP.\n",
        slug = app.slug,
    )
}

fn mcp_server_entry(endpoint: &str, token: Option<&str>) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "type": "http",
        "url": endpoint,
    });
    if let Some(t) = token {
        entry["headers"] = serde_json::json!({
            "Authorization": format!("Bearer {t}")
        });
    }
    entry
}

fn render_settings_json_with_auth(mcp_endpoint: &str, token: Option<&str>) -> String {
    let settings = serde_json::json!({
        "mcpServers": {
            "homeroute": mcp_server_entry(mcp_endpoint, token),
        },
        "enabledMcpjsonServers": ["homeroute"],
        "permissions": {
            "allow": [
                "mcp__homeroute__app_list",
                "mcp__homeroute__app_status",
                "mcp__homeroute__app_logs",
                "mcp__homeroute__db_tables",
                "mcp__homeroute__db_schema",
                "mcp__homeroute__db_query",
                "mcp__homeroute__db_get_schema",
                "mcp__homeroute__db_sync_schema",
                "mcp__homeroute__db_overview",
                "mcp__homeroute__db_count_rows",
                "mcp__homeroute__db_find",
                "mcp__homeroute__db_introspect",
                "mcp__homeroute__db_graphql",
                // Schema mutations (db_create_table / db_add_column / etc.)
                // are intentionally NOT in the auto-approve list — agents
                // get a confirmation prompt before each call. They are
                // discoverable via tools/list (see mcp.rs).
                "mcp__homeroute__docs_overview",
                "mcp__homeroute__docs_list_entries",
                "mcp__homeroute__docs_get",
                "mcp__homeroute__docs_search",
                "mcp__homeroute__docs_completeness",
                "mcp__homeroute__docs_diagram_get",
                "mcp__homeroute__todos_list",
                "mcp__homeroute__todos_create",
                "mcp__homeroute__todos_update",
                "mcp__homeroute__todos_delete",
            ],
            "deny": [],
        }
    });
    serde_json::to_string_pretty(&settings).expect("settings JSON serializes")
}

fn render_mcp_json_with_auth(mcp_endpoint: &str, token: Option<&str>) -> String {
    let mcp = serde_json::json!({
        "mcpServers": {
            "homeroute": mcp_server_entry(mcp_endpoint, token),
        }
    });
    serde_json::to_string_pretty(&mcp).expect("mcp JSON serializes")
}

/// Write `content` to `path` only if the existing content differs.
/// Returns `true` if the file was actually written.
fn write_if_changed(path: &Path, content: &str) -> io::Result<bool> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(true)
}

/// Write `content` to `path` only if the file does not already exist. Returns
/// `true` if the file was created. Utilisé pour les fichiers « agent-owned »
/// (typiquement `CLAUDE.md`) qu'on initialise avec un skeleton mais qu'on ne
/// doit jamais écraser ensuite — sinon l'agent perdrait ses notes.
fn write_if_missing(path: &Path, content: &str) -> io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(true)
}

fn log_write(slug: &str, path: &Path, content: &str) -> io::Result<()> {
    let changed = write_if_changed(path, content)?;
    if changed {
        info!(slug = %slug, file = %path.display(), "context written");
    } else {
        info!(slug = %slug, file = %path.display(), "context unchanged");
    }
    Ok(())
}

/// Skills additionnelles (read-only) en plus de `app-build`. Chaque entrée est
/// (nom_skill, contenu_complet_avec_frontmatter). Le nom devient le dossier
/// `src/.claude/skills/<nom>/SKILL.md`.
fn render_extra_skills(app: &Application) -> Vec<(&'static str, String)> {
    let mut skills = vec![
        ("app-status", format!(
            "---\n\
             name: app-status\n\
             description: Affiche l'état courant du process de l'app {slug} (state, PID, port, uptime, restart count). Utilise-moi quand l'utilisateur demande le statut, l'état, si l'app tourne, son PID ou son uptime.\n\
             allowed-tools: \n\
             ---\n\
             \n\
             # Statut de l'app `{slug}`\n\
             \n\
             Appelle le tool MCP `status` et affiche le résultat de manière concise : \
             state, PID, port, uptime, restart count.\n",
            slug = app.slug,
        )),
        ("app-logs", format!(
            "---\n\
             name: app-logs\n\
             description: Récupère et analyse les logs récents de l'app {slug}. Utilise-moi quand l'utilisateur demande les logs, des erreurs récentes, pourquoi l'app crash, ou un diagnostic runtime.\n\
             allowed-tools: \n\
             ---\n\
             \n\
             # Logs de l'app `{slug}`\n\
             \n\
             Appelle le tool MCP `logs` (paramètres : `limit` optionnel, `level` optionnel). \
             Identifie toute erreur ou warning et suggère des actions si pertinent.\n",
            slug = app.slug,
        )),
    ];

    if app.has_db {
        skills.push(("app-db-info", format!(
            "---\n\
             name: app-db-info\n\
             description: Donne un résumé de la base SQLite de l'app {slug} (tables, colonnes, row counts). Utilise-moi quand l'utilisateur demande ce qu'il y a en base, le schéma, ou un aperçu des données.\n\
             allowed-tools: \n\
             ---\n\
             \n\
             # Résumé base `{slug}`\n\
             \n\
             1. Appelle `db_tables` pour lister toutes les tables.\n\
             2. Pour chaque table, appelle `db_schema` pour obtenir les colonnes et le row count.\n\
             3. Affiche un résumé concis : nom de la table, nombre de colonnes, nombre de lignes.\n",
            slug = app.slug,
        )));
    }

    if is_flows_eligible(app) {
        skills.push(("flow-build", render_flow_build_skill(app)));
    }

    skills
}

/// Apps eligible for the hr-flow rollout. Phase 4 livrée le 2026-05-09 :
/// le daemon `hr-flowd` est partagé, donc les apps NextJS sont éligibles
/// au même titre que les apps Rust — leurs actions/connecteurs custom
/// sont exposés en TS via `@homeroute/flow-action`. Flutter reste hors
/// scope (pas d'app NodeJS/Rust hostable côté mobile).
fn is_flows_eligible(app: &Application) -> bool {
    matches!(
        app.stack,
        AppStack::AxumVite | AppStack::Axum | AppStack::NextJs
    )
}

fn render_flow_build_skill(app: &Application) -> String {
    let is_next = matches!(app.stack, AppStack::NextJs);
    let stack_block_check = if is_next {
        r#"1. **Dep `@homeroute/flow-action` dans `package.json`** (path-dep `file:/nvme/atelier/web/packages/flow-action` le temps de la transition).
2. **Catchall route** `app/api/_flow/[type]/[name]/route.ts` qui appelle `handleFlowCallback({ actions, connectors })`. La route DOIT déclarer `export const runtime = 'nodejs';`.
3. **`HR_FLOW_TOKEN` dans `.env.local`** (ou `.env`) — bearer partagé avec le daemon, miroir de `apps.json::flow_callback_token`.
4. **Dossier `flows/`** à la racine de l'app (au même niveau que `app/`, `lib/`, `package.json`).
5. **Inscription côté daemon** : `apps.json` côté Medion porte `flow_callback_url=http://127.0.0.1:<port>` + `flow_callback_token` pour ce slug, et le daemon a fait `POST /v1/_admin/reload` après ajout."#
    } else {
        r#"1. **Deps `hr-flow` + `hr-flow-callback` dans `server/Cargo.toml`** — path-dep relatif vers `/nvme/atelier/crates/hr-flow*`.
2. **Module `flows/`** dans `server/src/` avec `pub fn register_callbacks() -> Router<Arc<AppState>>` qui crée un `hr_flow_callback::router()` et y attache les `mount_<fn>` (actions custom, générés par `#[flow_action]`) + les `with_connector(name, Arc::new(...))` (connecteurs custom).
3. **`register_callbacks()` mergé dans le router axum principal de `main.rs`** — AVANT le premier `with_state`.
4. **`HR_FLOW_TOKEN` dans `.env`** — bearer partagé avec le daemon, miroir de `apps.json::flow_callback_token`.
5. **Dossier `flows/`** au niveau de `server/src/` (relatif au cwd du process), ajouté au `build_artefact` de l'app.
6. **Inscription côté daemon** : `apps.json` côté Medion porte `flow_callback_url=http://127.0.0.1:<port>` + `flow_callback_token` pour ce slug, et le daemon a fait `POST /v1/_admin/reload` après ajout."#
    };

    let stack_block_connector = if is_next {
        r#"Si tu as besoin d'appeler un service tiers (et qu'`http` daemon-side ne suffit pas pour des raisons d'auth, de format, etc.) :

1. Dans la route catchall, ajoute un sous-objet à `connectors` :
   ```ts
   import { handleFlowCallback, type FlowAction } from '@homeroute/flow-action';
   import { OpenRouterClient } from '@/lib/openrouter';

   const openrouter = new OpenRouterClient(process.env.OPENROUTER_API_KEY!);

   export const runtime = 'nodejs';
   export const POST = handleFlowCallback({
     actions: { /* ... */ },
     connectors: {
       openrouter: {
         chat: async (_input, params, _ctx) => openrouter.chat(params),
       },
     },
   });
   ```
2. Si plusieurs op (`chat`, `embed`, …), expose-les dans le map.
3. Les credentials viennent de `.env` (par app, jamais hardcodées)."#
    } else {
        r#"Si tu as besoin d'appeler un service tiers (et qu'`http` daemon-side ne suffit pas pour des raisons d'auth, de format, etc.) :

1. Crée un module Rust `server/src/flows/connectors/<nom>.rs` qui implémente le trait `hr_flow::Connector` (méthode `async fn call(&self, op: &str, params: Value) -> FlowResult<Value>`).
2. Dans `flows/mod.rs::register_callbacks()`, ajoute `router = router.with_connector("<nom>", Arc::new(MyConnector::from_env()?));`.
3. Les credentials viennent de `.env` (par app, jamais hardcodées)."#
    };

    let stack_block_action = if is_next {
        r#"1. Dans `lib/flow-actions/<nom>.ts` :
   ```ts
   import type { FlowAction } from '@homeroute/flow-action';

   export const compute_score: FlowAction = async (input, params, _ctx) => {
     // calcul pur — pas de boucle visible, pas de side-effect
     return { score: 0.42 };
   };
   ```
2. Dans la route catchall, importe et ajoute au map `actions` :
   ```ts
   import { compute_score } from '@/lib/flow-actions/compute_score';
   export const POST = handleFlowCallback({ actions: { compute_score }, ... });
   ```
3. Référence dans le flux :
   ```toml
   [[steps]]
   id = "score"
   kind = "action"
   action = "compute_score"
   params = { tx = "{{ steps.fetch.output }}" }
   ```"#
    } else {
        r#"1. Dans `server/src/flows/actions/<nom>.rs` (ou similaire) :
   ```rust
   use hr_flow::flow_action;

   #[flow_action(name = "compute_score")]
   async fn compute_score(input: serde_json::Value) -> hr_flow::FlowResult<serde_json::Value> {
       // calcul pur — pas de boucle visible, pas de side-effect
       Ok(serde_json::json!({ "score": 0.42 }))
   }
   ```
2. Dans `flows/mod.rs::register_callbacks()`, ajoute `router = router.with_action(actions::scoring::mount_compute_score);` (le `mount_<fn>` est généré par la macro à côté de `register_<fn>`).
3. Référence dans le flux :
   ```toml
   [[steps]]
   id = "score"
   kind = "action"
   action = "compute_score"
   params = { tx = "{{ steps.fetch.output }}" }
   ```"#
    };

    format!(r#"---
name: flow-build
description: Crée, teste, débugge un flux hr-flow pour cette app. Utilise-moi quand l'utilisateur demande un nouveau flux, veut modifier un flux existant, ajoute un connecteur custom ou une action, ou cherche pourquoi un run a échoué. Lis aussi `.claude/rules/flows-first.md` qui pose la doctrine.
allowed-tools:
---

# `flow-build` — guide complet

## Doctrine (rappel — voir `flows-first.md` pour la version longue)

Pour toute logique chaînée : **flux > primitive > action custom**. Une action custom = une fonction pure ; toute boucle/condition/accumulation visible doit être en primitive. Les flux sont orchestrés par le daemon partagé `hr-flowd` (Medion 127.0.0.1:4002, loopback) ; cette app expose ses actions/connecteurs custom via callback HTTP.

## Prérequis — l'app est-elle intégrée à `hr-flow` ?

Vérifie d'abord :

{stack_check}

Si **un seul** de ces points manque, l'app n'est pas intégrée. Réfère-toi au prompt de migration que t'a passé l'utilisateur ; **n'invente pas** la procédure d'intégration depuis le skill — c'est un prompt explicite et structuré (voir `/nvme/atelier/docs/migration-prompts/{stack}.md`).

## Workflow

### 1. Créer un flux

1. **Définition TOML** : dans `flows/<nom>.toml`, format plat avec `parent`/`parent_branch`. Schéma minimal :
   ```toml
   name = "<nom>"
   description = "Une phrase pour l'utilisateur"

   [[steps]]
   id = "<id_unique>"
   kind = "connector" | "action" | "compose" | "set_var" | "if" | "for_each" | …
   # … champs spécifiques selon kind
   ```
2. **Inscription daemon** : après ajout d'un nouveau TOML, `POST http://127.0.0.1:4100/api/flows/_admin/reload` (Atelier API → daemon). Ou `make deploy-app SLUG=<slug>` qui rsync les TOML + relance — le scan filesystem du daemon les voit après reload.
3. **Test rapide** : `mcp__homeroute__flow.run(slug="<app-slug>", name="<nom>", input={{...}})` → vérifie status + output. Le `slug` est auto-injecté par le MCP per-app — tu peux l'omettre la plupart du temps.
4. **Inspection** : `flow.get_run(slug, run_id)` pour voir l'arbre des steps avec input/output/durée par step.

### 2. Ajouter un connecteur custom

{stack_connector}

### 3. Ajouter une action custom (dernier recours)

{stack_action}

### 4. Debug d'un run échoué

1. `flow.list_runs(slug, flow_name="<nom>", limit=10)` → repère le run failed.
2. `flow.get_run(slug, run_id="<id>")` → trouve le step qui a `status=failed` ; lis `error` et `input`.
3. Reproduis localement avec `flow.run(slug, name, input=<input du run failed>)`.
4. Une fois corrigé, `flow.replay(slug, run_id)` re-tourne le run d'origine pour confirmer le fix.

## Règles de forme

- IDs de step en `snake_case`, descriptifs (`fetch_user`, pas `step1`).
- Une expression `{{{{ ... }}}}` qui occupe toute la string préserve le type natif (array, number, etc.). Mixée à du texte, elle est coercée en string.
- Variables d'itération : référencées par `@<nom>` (par défaut `@iter`). Dispo seulement à l'intérieur d'un `for_each`.
- Conditions (`if.cond`, `while.cond`) : sous-langage `==`, `!=`, `<`, `<=`, `>`, `>=`, `&&`, `||`, `!`, parens. Les `{{{{ ... }}}}` y sont substituées d'abord.

## Tools MCP disponibles

| Tool | Usage |
|------|-------|
| `flow.list_definitions` | Liste les flux de l'app |
| `flow.get_definition` | Lit un flux complet (parsé + source TOML) |
| `flow.run` | Trigger manuel d'un flux |
| `flow.list_runs` | Historique des runs (filtrable par `flow_name`) |
| `flow.get_run` | Détail d'un run avec arbre des steps |
| `flow.replay` | Rejoue un run par id |
"#,
        stack_check = stack_block_check,
        stack = if is_next { "next" } else { "rust" },
        stack_connector = stack_block_connector,
        stack_action = stack_block_action,
    )
}

/// Slash-commands & fichiers legacy à nettoyer à chaque régénération.
/// Les builds sont désormais la skill `app-build` ; les raccourcis status/logs/db-info
/// sont devenus des skills — plus rien ne vit dans `src/.claude/commands/`.
const OBSOLETE_SLASH_COMMANDS: &[&str] = &[
    "build.md",
    "build-client.md",
    "build-server.md",
    "build-api.md",
    "build-apk.md",
    "publish-apk.md",
    "install.md",
    "deploy.md",
    "status.md",
    "logs.md",
    "db-info.md",
];

/// Noms de skills auxiliaires potentiellement obsolètes à nettoyer si la stack
/// de l'app change (ex: app passe de `has_db=true` à `false` → retirer app-db-info).
const ALL_EXTRA_SKILL_NAMES: &[&str] = &["app-status", "app-logs", "app-db-info", "flow-build"];

/// Fichiers `rules/*.md` obsolètes à nettoyer à chaque génération. Certains
/// étaient produits par un système antérieur (bootstrap env-agent) et sont
/// encore présents sous `src/.claude/rules/` dans les apps existantes ; d'autres
/// ont été renommés ou fusionnés au fil du temps.
const OBSOLETE_RULE_FILES: &[&str] = &[
    "env-rules.md",
    "env-context.md",
    "git.md",
    "app-build.md",
    "deploy.md",
    "project.md",
    "homeroute-deploy.md",
    "homeroute-dev.md",
    "homeroute-docs.md",
    "homeroute-dataverse.md",
    "homeroute-store.md",
];

/// Nettoie les fichiers de contexte agent qui traînent au niveau `app_dir`
/// (au-dessus de `src/`). Aucun fichier utile ne doit vivre là — voir
/// l'INVARIANT du module. Silencieux si rien à supprimer.
fn cleanup_legacy_parent_context(app_dir: &Path, slug: &str) {
    remove_if_exists(&app_dir.join("CLAUDE.md"), slug);
    remove_if_exists(&app_dir.join(".mcp.json"), slug);
    let parent_claude = app_dir.join(".claude");
    if parent_claude.exists() {
        match fs::remove_dir_all(&parent_claude) {
            Ok(()) => info!(slug, path = %parent_claude.display(), "legacy parent .claude/ removed"),
            Err(e) => warn!(slug, path = %parent_claude.display(), error = %e, "failed to remove legacy parent .claude/"),
        }
    }
}

/// Always-on rule that documents the DB stack the app is on. The content
/// is selected at generation time from `app.db_backend`:
/// - `LegacySqlite` → "MIGRATION POSTGRES PENDING" (RULE PRINCIPALE that
///   asks the agent to minimise schema churn until migration runs)
/// - `PostgresDataverse` → new-stack documentation + post-migration
///   cleanup instruction (delete leftover SQLite references)
///
/// The rule is regenerated at every boot / `AppUpdate` /
/// `AppRegenerateContext`. Switching `db_backend` flips the rule
/// automatically — agents inside the app pick up the new instructions on
/// the next context refresh.
/// Backend-aware DB section in `app-info.md`. Tracks the same 3-state
/// model as [`render_db_md`] so the headline description in the
/// agent's "what is this app" rule stays consistent with the
/// dedicated `db.md` rule.
fn render_db_section(app: &crate::types::Application, db_tables: &Option<Vec<String>>) -> String {
    use crate::types::DbBackend;
    if !app.has_db {
        return "Pas de base de données configurée pour cette app.".to_string();
    }

    let tables_block = match db_tables {
        Some(tables) if !tables.is_empty() => {
            let mut s = String::from("\n**Tables :**\n");
            for t in tables {
                s.push_str(&format!("- `{}`\n", t));
            }
            s
        }
        _ => String::new(),
    };

    match app.db_backend {
        DbBackend::PostgresDataverse => format!(
            "PostgreSQL Dataverse (`app_{slug}`).\n\
             {tables}\n\
             - Connexion : `DATABASE_URL` injecté dans ton env runtime\n\
             - Tools MCP : `db_graphql` (queries/mutations), `db_introspect` (SDL)\n\
             - Voir `.claude/rules/db.md` pour les règles d'usage.\n",
            slug = app.slug,
            tables = tables_block,
        ),
    }
}

fn render_db_md(app: &crate::types::Application) -> String {
    render_db_md_dataverse(app)
}

fn render_db_md_dataverse(app: &crate::types::Application) -> String {
    format!(
        "# Base de données — PostgreSQL + Dataverse-like + GraphQL\n\
         \n\
         Cette app (`{slug}`) utilise la stack **HomeRoute Dataverse** :\n\
         \n\
         - **PostgreSQL 18** sur Medion :5432, base dédiée `app_{slug}` (rôle\n\
           `app_{slug}` aux droits limités à cette base)\n\
         - **Connexion runtime** : `DATABASE_URL` est injectée dans l'env du\n\
           process par `hr-apps`. Tu peux utiliser sqlx, tokio-postgres, prisma,\n\
           etc. selon la stack de l'app.\n\
         - **API GraphQL managée** : endpoint `/api/apps/{slug}/db/graphql` (POST)\n\
           pour les opérations Dataverse — schéma généré dynamiquement depuis\n\
           les métadonnées `_dv_*`. Endpoint introspection : `GET /api/apps/{slug}/db/introspect`.\n\
         - **Schema-ops** : tools MCP `db_create_table`, `db_add_column`,\n\
           `db_create_relation`, `db_drop_table`, `db_remove_column` (inchangés depuis le legacy) — \n\
           ils créent les tables avec trigger `updated_at`, FK natives, types Dataverse riches.\n\
         \n\
         ## Comment requêter (côté app et côté agent)\n\
         \n\
         - **Lecture** : query GraphQL avec filtres style Hasura\n\
           ({{ email: {{ _ilike: \"%@%\" }} }})\n\
         - **Mutations** : `insert<Table>`, `update<Table>`, `delete<Table>`\n\
         - **Découverte du schéma** : appelle `db_introspect` (renvoie le SDL en\n\
           un seul shot) — préférable à un `__schema` query manuel.\n\
         \n\
         ## Tools MCP `db_*` à privilégier\n\
         \n\
         - `db_introspect` — voir le schéma GraphQL en SDL\n\
         - `db_graphql` — exécuter une query/mutation GraphQL arbitraire\n\
         - `db_find` — sucre syntaxique (where/orderBy/limit/offset/expand → query GraphQL)\n\
         - `db_tables`, `db_schema`, `db_get_schema` — métadonnées Dataverse\n\
         - `db_create_table`, `db_add_column`, `db_create_relation` — schema-ops\n\
         \n\
         ## ❌ Ne PAS faire\n\
         \n\
         - Pas de `db_query` / `db_exec` (SQL brut) — ils renvoient une erreur sur\n\
           ce backend. Utilise `db_graphql`.\n\
         - Pas d'ouverture directe d'un fichier `.db` — il n'y en a plus.\n\
         \n\
         ## 🧹 Nettoyage post-migration\n\
         \n\
         Si tu trouves dans le code de l'app **des restes de l'ancienne stack SQLite**\n\
         (chemins `db.sqlite`, dépendances `rusqlite`, calls vers `db_query`/`db_exec`\n\
         SQL brut, mentions de `hr-db` legacy, fixtures de migration `_dv_*`\n\
         SQLite-flavored), **supprime-les** : la migration est faite, ces restes ne\n\
         servent à rien.\n\
         \n\
         Si tu trouves dans la doc / `CLAUDE.md` de l'app des phrases du type\n\
         « migration Postgres en attente », **supprime-les aussi** — la migration a eu\n\
         lieu et la règle « post-migration » que tu lis maintenant le confirme.\n",
        slug = app.slug,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AppStack, Application};
    use std::collections::BTreeMap;

    fn make_app(slug: &str, name: &str, has_db: bool) -> Application {
        let mut app = Application::new(slug.to_string(), name.to_string(), AppStack::AxumVite);
        app.has_db = has_db;
        app.port = 3001;
        app.run_command = format!("./bin/{}", slug);
        app.build_command = Some("cargo build --release".to_string());
        app.health_path = "/api/health".to_string();
        let mut env_vars = BTreeMap::new();
        env_vars.insert("API_KEY".to_string(), "secret".to_string());
        app.env_vars = env_vars;
        app
    }

    fn test_generator(tmp: &Path) -> ContextGenerator {
        ContextGenerator::new(
            tmp.to_path_buf(),
            "mynetwk.biz".to_string(),
            "http://127.0.0.1:4001/mcp".to_string(),
        )
    }

    #[test]
    fn generate_for_app_creates_expected_files() {
        let tmp = std::env::temp_dir().join("hr-apps-context-test-1");
        let _ = fs::remove_dir_all(&tmp);
        let ctx = test_generator(&tmp);
        let trader = make_app("trader", "Trader", true);
        let wallet = make_app("wallet", "Wallet", false);
        let all = vec![trader.clone(), wallet.clone()];

        // Scaffold src/ pour que `app.src_dir().exists()` soit vrai dans le test.
        // ⚠ Application::src_dir() est hardcodé en /opt/homeroute/apps/... donc on
        // construit le path "relatif au tmp" manuellement pour les assertions,
        // sachant que le code réel utilise le hardcoded. On valide donc le contenu
        // renderer par renderer ici plutôt que leur écriture physique sous tmp.
        fs::create_dir_all(tmp.join("trader/src")).unwrap();

        // Pré-créer des vestiges au niveau parent (app_dir) : CLAUDE.md, .mcp.json, .claude/
        // → doivent tous disparaître après generate_for_app (cleanup legacy).
        let parent_dir = tmp.join("trader");
        fs::write(parent_dir.join("CLAUDE.md"), "stale parent CLAUDE").unwrap();
        fs::write(parent_dir.join(".mcp.json"), "{}").unwrap();
        fs::create_dir_all(parent_dir.join(".claude/rules")).unwrap();
        fs::write(parent_dir.join(".claude/settings.json"), "{}").unwrap();
        fs::write(parent_dir.join(".claude/rules/app-build.md"), "stale rule").unwrap();
        assert!(parent_dir.join("CLAUDE.md").exists());
        assert!(parent_dir.join(".claude").exists());

        ctx.generate_for_app(&trader, &all, Some(vec!["users".into(), "trades".into()]))
            .unwrap();

        // Cleanup legacy parent-level : tout a disparu.
        assert!(!parent_dir.join("CLAUDE.md").exists(),
                "trader/CLAUDE.md parent-level doit être supprimé");
        assert!(!parent_dir.join(".mcp.json").exists(),
                "trader/.mcp.json parent-level doit être supprimé");
        assert!(!parent_dir.join(".claude").exists(),
                "trader/.claude/ parent-level doit être supprimé intégralement");

        // Les renderers produisent le bon contenu (vérif directe, indépendante du
        // path d'écriture physique qui dépend du hardcoded src_dir).
        let settings = render_settings_json_with_auth(
            "http://127.0.0.1:4001/mcp?project=trader",
            None,
        );
        let parsed: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(
            parsed["mcpServers"]["homeroute"]["url"].as_str().unwrap(),
            "http://127.0.0.1:4001/mcp?project=trader"
        );
        assert!(
            parsed["permissions"]["allow"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some("mcp__homeroute__app_list"))
        );

        // app-info.md contient l'identité + autres apps + DB tables.
        let app_info = render_app_info_md(&trader, &all, &Some(vec!["users".into(), "trades".into()]));
        assert!(app_info.contains("`trader`"));
        assert!(app_info.contains("Vite+Rust"));
        assert!(app_info.contains("`users`"));
        assert!(app_info.contains("`trades`"));
        assert!(app_info.contains("Wallet"));
        assert!(app_info.contains("`API_KEY`"));

        // Skeleton CLAUDE.md minimal, n'inclut PAS les infos dynamiques.
        let initial_md = render_initial_claude_md(&trader);
        assert!(initial_md.contains("# Trader — Carnet de bord"));
        assert!(!initial_md.contains("Vite+Rust"),
                "skeleton CLAUDE.md ne doit pas dupliquer app-info.md");

        // Skill app-build + script.
        let skill_content = render_app_build_skill(&trader);
        assert!(skill_content.contains("name: app-build"));
        assert!(skill_content.contains("allowed-tools: Bash(bash .claude/skills/app-build/build.sh"));
        assert!(skill_content.contains("bash .claude/skills/app-build/build.sh"));
        let script = render_app_build_script(&trader);
        assert!(script.contains("/api/apps/trader/build"));
        assert!(script.starts_with("#!/usr/bin/env bash"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn generate_for_app_skips_when_src_missing() {
        let tmp = std::env::temp_dir().join("hr-apps-context-test-no-src");
        let _ = fs::remove_dir_all(&tmp);
        let ctx = test_generator(&tmp);
        let app = make_app("ghost", "Ghost", false);
        // src_dir absent → generate_for_app doit retourner Ok sans crash, avec warn.
        let result = ctx.generate_for_app(&app, &[app.clone()], None);
        assert!(result.is_ok(), "no-src should be a soft skip, not an error");
        // Rien n'a été créé sous tmp/ghost/
        assert!(!tmp.join("ghost/.claude").exists());
        assert!(!tmp.join("ghost/CLAUDE.md").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn generate_root_lists_apps_and_writes_settings() {
        let tmp = std::env::temp_dir().join("hr-apps-context-test-2");
        let _ = fs::remove_dir_all(&tmp);
        let ctx = test_generator(&tmp);
        let trader = make_app("trader", "Trader", true);
        let wallet = make_app("wallet", "Wallet", false);

        ctx.generate_root(&[trader, wallet]).unwrap();

        let root = fs::read_to_string(tmp.join("CLAUDE.md")).unwrap();
        assert!(root.contains("HomeRoute Apps Workspace"));
        assert!(root.contains("| Trader | `trader`"));
        assert!(root.contains("| Wallet | `wallet`"));
        assert!(tmp.join(".claude/settings.json").exists());
        assert!(tmp.join(".mcp.json").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_if_changed_skips_when_identical() {
        let tmp = std::env::temp_dir().join("hr-apps-context-test-3");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("file.txt");

        assert!(write_if_changed(&path, "hello").unwrap());
        assert!(!write_if_changed(&path, "hello").unwrap());
        assert!(write_if_changed(&path, "world").unwrap());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn app_info_md_no_db_renders_no_database_section() {
        let app = make_app("static", "Static", false);
        let md = render_app_info_md(&app, &[app.clone()], &None);
        assert!(
            md.contains("Pas de base de données"),
            "app-info.md should say no DB when has_db=false: {md}"
        );
    }

    #[test]
    fn app_info_md_has_identity_fields() {
        let mut trader = make_app("trader", "Trader", true);
        trader.port = 3008;
        let calendar = make_app("calendar", "Calendar", false);
        let md = render_app_info_md(&trader, &[trader.clone(), calendar.clone()], &Some(vec!["users".into()]));
        assert!(md.contains("**Slug :** `trader`"));
        assert!(md.contains("**Port interne :** 3008"));
        assert!(md.contains("Calendar"), "liste des autres apps: {md}");
        assert!(md.contains("`users`"));
    }

    #[test]
    fn claude_md_upkeep_rule_is_static_and_mentions_rules() {
        let md = render_claude_md_upkeep_md();
        assert!(md.contains("# Maintenance de CLAUDE.md"));
        assert!(md.contains("app-info.md"));
        assert!(md.contains("mcp-tools.md"));
        assert!(md.contains("workflow.md"));
    }

    #[test]
    fn initial_claude_md_is_a_skeleton() {
        let trader = make_app("trader", "Trader", true);
        let md = render_initial_claude_md(&trader);
        assert!(md.contains("# Trader — Carnet de bord"));
        assert!(md.contains("claude-md-upkeep.md"));
        assert!(md.contains("app-info.md"));
    }

    #[test]
    fn write_if_missing_only_writes_once() {
        let tmp = std::env::temp_dir().join("hr-apps-context-write-if-missing");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("claude.md");
        assert!(write_if_missing(&path, "v1").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");
        // Deuxième appel : le fichier existe, pas de write.
        assert!(!write_if_missing(&path, "v2").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");
        let _ = fs::remove_dir_all(&tmp);
    }
}
