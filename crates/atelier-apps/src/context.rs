//! Claude Code context generation for per-app Studio awareness.
//!
//! # INVARIANT — workspace per-app
//!
//! Le workspace de chaque app est `{apps_path}/{slug}/src/` (ouvert par le
//! Studio / l'agent Claude Code). L'agent Claude Code
//! ne lit **que** ce qui vit sous `src/`. TOUT fichier destiné à l'agent
//! (CLAUDE.md, .claude/, .mcp.json) DOIT donc être écrit sous `src/` ; les
//! fichiers au niveau `{apps_path}/{slug}/` (au-dessus de `src/`) sont
//! invisibles pour l'agent et sont activement supprimés par `generate_for_app`
//! pour éviter toute confusion avec une version stale.
//!
//! Fichiers per-app générés (tous sous `{slug}/src/`) :
//!   - `src/CLAUDE.md`                         — carnet de bord agent-owned (write-once)
//!   - `src/.mcp.json`                         — MCP server config (CLI compat)
//!   - `src/.claude/settings.json`             — enabledMcpjsonServers only (def serveur dans .mcp.json ; sans permissions — cf. render_settings_json)
//!   - `src/.claude/rules/app-info.md`         — identité / stack / port / autres apps (régénéré)
//!   - `src/.claude/rules/mcp-tools.md`        — tools MCP disponibles
//!   - `src/.claude/rules/workflow.md`         — workflow dev
//!   - `src/.claude/rules/docs.md`             — usage obligatoire de `docs.*`
//!   - `src/.claude/rules/claude-md-upkeep.md` — règle de maintenance de CLAUDE.md
//!   - `src/.claude/rules/report-issues.md`    — quand/comment remonter une friction plateforme
//!   - `src/.claude/skills/0-build/{SKILL.md,build.sh}` — seul script bash restant (stream cargo/pnpm)
//!   - `src/.claude/skills/{0-deploy,0-report-issue}/SKILL.md` — pointent vers les tools MCP `ship`/`issue_report` (scripts curl supprimés 2026-07-03)
//!   - `src/.claude/skills/{0-status,0-logs,0-db-info,0-surveillance}/SKILL.md`
//!
//! Fichiers workspace-root (pour le Studio global `studio.mynetwk.biz`,
//! workspace = `/var/lib/atelier/apps/`) :
//!   - `{apps_path}/CLAUDE.md`
//!   - `{apps_path}/.claude/settings.json`
//!   - `{apps_path}/.mcp.json`

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::types::{Application, Visibility};

/// Generates Claude Code context files for Atelier apps.
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
    /// `{apps_path}/{slug}/` est réservé aux fichiers runtime (`.env`, etc.)
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
    /// (héritage hôte de build séparé ; désormais build en place sur Medion).
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
        apply_rules_dir_perms(&src_claude_dir);
        apply_rules_dir_perms(&src_rules_dir);
        apply_rules_dir_perms(&src_skills_dir);

        // Step 3 — Project-scoped MCP config + settings, au seul niveau src/.
        let project_mcp = format!("{}?project={}", self.mcp_endpoint, app.slug);
        let settings = render_settings_json();
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
        // db.md documents the postgres-dataverse stack + post-migration
        // cleanup instructions (delete any leftover SQLite refs in app code).
        log_write(&app.slug, &src_rules_dir.join("db.md"),
                  &render_db_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("mcp-tools.md"),
                  &render_mcp_tools_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("workflow.md"),
                  &self.render_workflow_md(app))?;
        // Conventions de code génériques : la plateforme étant stack-agnostique
        // (aucun scaffold, stack = label libre), la cohérence inter-apps passe
        // par ces règles-là, pas par des squelettes identiques.
        log_write(&app.slug, &src_rules_dir.join("conventions.md"),
                  &render_conventions_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("surveillance.md"),
                  &render_surveillance_rule_md(app))?;
        log_write(&app.slug, &src_rules_dir.join("claude-md-upkeep.md"),
                  &render_claude_md_upkeep_md())?;
        // Canal de remontée des frictions PLATEFORME (vers CLAUDE_ISSUES.json,
        // relu en session dev Atelier). Pilote la skill `0-report-issue`.
        log_write(&app.slug, &src_rules_dir.join("report-issues.md"),
                  &render_report_issues_rule_md(app))?;

        // Le système de flux a été éradiqué (2026-05-26) — nettoyer les
        // anciennes règles `flows-first.md` si encore présentes.
        remove_if_exists(&src_rules_dir.join("flows-first.md"), &app.slug);

        // Step 5 — Cleanup des règles obsolètes (scaffolds anciens, systèmes précédents).
        for legacy in OBSOLETE_RULE_FILES {
            remove_if_exists(&src_rules_dir.join(legacy), &app.slug);
        }

        // Step 6 — Skills.
        let app_build_dir = src_skills_dir.join("0-build");
        fs::create_dir_all(&app_build_dir)?;
        apply_rules_dir_perms(&app_build_dir);
        log_write(&app.slug, &app_build_dir.join("SKILL.md"), &render_app_build_skill(app))?;
        log_write(&app.slug, &app_build_dir.join("build.sh"), &render_app_build_script(app))?;

        // 0-deploy et 0-report-issue sont passés aux tools MCP `ship` et
        // `issue_report` (2026-07-03) : SKILL.md seul, plus de script curl.
        // WHY : le path MCP profite du journal d'actions + du plan-gate, et
        // supprime les dépendances shell (jq). Les scripts résiduels des
        // workspaces existants sont purgés ici (ils ne passent pas par
        // OBSOLETE_RULE_FILES, qui ne couvre que rules/*.md).
        let app_deploy_dir = src_skills_dir.join("0-deploy");
        fs::create_dir_all(&app_deploy_dir)?;
        apply_rules_dir_perms(&app_deploy_dir);
        log_write(&app.slug, &app_deploy_dir.join("SKILL.md"), &render_app_deploy_skill(app))?;
        remove_if_exists(&app_deploy_dir.join("deploy.sh"), &app.slug);

        let app_report_dir = src_skills_dir.join("0-report-issue");
        fs::create_dir_all(&app_report_dir)?;
        apply_rules_dir_perms(&app_report_dir);
        log_write(&app.slug, &app_report_dir.join("SKILL.md"), &render_report_issue_skill(app))?;
        remove_if_exists(&app_report_dir.join("report-issue.sh"), &app.slug);

        let produced: std::collections::HashSet<&'static str> = render_extra_skills(app)
            .iter()
            .map(|(name, _)| *name)
            .collect();
        for (name, content) in render_extra_skills(app) {
            let skill_dir = src_skills_dir.join(name);
            fs::create_dir_all(&skill_dir)?;
            apply_rules_dir_perms(&skill_dir);
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

        let settings = render_settings_json();
        log_write("<root>", &claude_dir.join("settings.json"), &settings)?;

        let mcp_json = render_mcp_json_with_auth(&self.mcp_endpoint, self.mcp_token.as_deref());
        log_write("<root>", &self.apps_path.join(".mcp.json"), &mcp_json)?;

        info!(count = all_apps.len(), "workspace-root context written");
        Ok(())
    }

    // NB : le refresh « toutes les apps + root » vit dans
    // `AppsContext::regenerate_all_contexts` (atelier-api), qui sait résoudre
    // les `db_tables` par app — un helper ici ne le pourrait pas (perte de la
    // section DB d'app-info.md, bug de l'ancien `refresh_all` supprimé).

    // ── Renderers ──────────────────────────────────────────────────────

    fn render_root_claude_md(&self, all_apps: &[Application]) -> String {
        let mut table_rows = String::new();
        for app in all_apps {
            let db_cell = if app.has_db {
                "postgres-dataverse".to_string()
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
                stack = app.stack,
                domain = app.domain,
                visibility = visibility,
                db = db_cell,
            ));
        }

        if table_rows.is_empty() {
            table_rows.push_str("| _no apps yet_ |  |  |  |  |  |\n");
        }

        format!(
            "# Atelier Apps Workspace\n\
             \n\
             This is the workspace root for every application managed by Atelier. \
             Each app lives under `{apps_path}/<slug>/` with its own sources, build \
             artifacts, `.env` and (optionally) a postgres-dataverse database.\n\
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
             ## How Atelier runs apps\n\
             - Apps run **directly on the host** as processes supervised by Atelier \
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
             A single MCP server `studio` is configured at `{mcp_endpoint}` via \
             `.claude/settings.json` and `.mcp.json`. Read-only tools (`app.list`, \
             `app.status`, `app.logs`, `db.tables`, `db.schema`, `db.query`, \
             `docs.overview`, `docs.list_entries`, `docs.get`, `docs.search`, \
             `docs.completeness`, `docs.diagram_get`) are auto-approved for the \
             interactive Studio user (user-level settings). Doc mutations \
             (`docs.update`, `docs.delete`, `docs.diagram_set`) require \
             confirmation.\n\
             \n\
             ## Pattes plateforme (scope per-app)\n\
             Dans un workspace d'app, l'agent dispose aussi de : `ship` (livraison \
             prod), `env_list`/`env_set`/`env_delete` (variables d'env), \
             `notify_user` (notifier l'utilisateur — les actions plateforme sont \
             déjà journalisées automatiquement, ne pas notifier pour ça) et \
             `issue_report` (remonter une friction plateforme).\n\
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
             - Managed by Atelier as a host-level process. Use MCP `app.*` tools to \
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
             > Ce cycle (edit → build → `app.control restart`) sert à **itérer en \
             session** sur une feature en cours. La **livraison en prod** (0-build → \
             0-deploy) se fait en **fin de feature validée** — voir ci-dessous.\n\
             \n\
             ## Fin de feature : livrer PUIS proposer le commit\n\
             Quand une feature / un correctif cohérent est **terminé et validé** \
             (vérifié end-to-end sur {url}), applique **systématiquement** et **dans \
             cet ordre** :\n\
             1. **Livre en prod, sans demander** : `0-build` (compile en place) puis, \
             s'il passe, le tool MCP `ship` (stop + restart — la skill `0-deploy` \
             documente le détail). La livraison est **libre** — tu ne demandes PAS \
             l'autorisation de déployer, comme un `make deploy-app`. Vérifie ensuite \
             `app.status` (`running`) et {url}.\n\
             2. **PROPOSE ensuite le commit** : le commit reste **décidé par \
             l'utilisateur**. Ne commit/push jamais en silence ni du travail à moitié \
             fait. S'il accepte, dans `{src_dir}` via Bash : `git add -A && git commit \
             -m \"<résumé clair, impératif>\"` (**aucune attribution Claude / \
             Co-Authored-By**), puis `git push origin` (origin = dépôt Atelier local, \
             miroir GitHub via post-receive).\n\
             \n\
             **Règle de livraison** : déployer est libre et systématique en fin de \
             feature ; committer se demande. (Symétrique de la doctrine Atelier : \
             déployer librement, demander avant de committer.)\n\
             \n\
             **Si ça échoue** : `0-build` KO → ne déploie PAS, corrige, re-build sur \
             vert. `0-deploy` KO → l'app peut être down : diagnostique (`app.status` / \
             `app.logs`), répare, re-livre ; ne propose le commit qu'une fois \
             `running` et sain.\n\
             \n\
             **Pendant l'itération** (feature pas finie) : tu build librement pour \
             compiler/tester mais tu ne déploies PAS à chaque petit edit — 0-deploy se \
             déclenche une fois la feature validée.\n\
             \n\
             ## Regles\n\
             - **Builder sur place** : jamais de cross-compile, tout se compile sur le serveur de production.\n\
             - **Pas de mode dev** : pas de `pnpm dev` / `cargo watch`. Production only.\n\
             - **Pas de pipelines, pas d'environnements** : un seul runtime, la prod ; \
             pas de promotion dev→acc→prod. « Pas de pipeline » = pas d'étages, PAS \
             « ne pas déployer » : la livraison en fin de feature est justement \
             systématique (voir ci-dessus).\n\
             \n\
             ## Environment variables\n\
             Atelier injects a single `.env` (rendered automatically — **ne pas \
             l'éditer à la main**, il est régénéré). Tiers :\n\
             - **Plateforme (calculé)** : `PORT` (jamais hardcoder un port), et — \
             quand `has_db` — la passerelle dataverse `HR_DV_BASE_URL` / `HR_DV_TOKEN` \
             / `HR_APP_UUID`, plus `ATELIER_INGEST_URL` / `ATELIER_LOGS_TOKEN` (logs). \
             **Pas de `DATABASE_URL`** : l'accès DB est gateway-only depuis 2026-05-30.\n\
             - **Variables applicatives** : config + secrets gérés via les tools MCP \
             `env_list` / `env_set` / `env_delete` (ou l'onglet *Variables* du Studio). \
             Un `scope` `build` les expose aussi au build (`VITE_*` / `NEXT_PUBLIC_*`). \
             Restart requis pour qu'une modif soit vue par le process.\n\
             \n\
             ## Notifications & journal\n\
             - Toutes tes **actions plateforme** via MCP (restart, ship, env, schéma, \
             scan) sont **journalisées automatiquement** côté Atelier — tu n'as RIEN à \
             faire, l'utilisateur les voit dans sa cloche de notifications.\n\
             - `notify_user(title, body?, level?)` est réservé à ce qui mérite \
             **l'attention de Romain** : décision à prendre, anomalie détectée, \
             résultat inattendu d'une tâche longue. JAMAIS pour dire « j'ai redémarré \
             l'app » (le journal le dit déjà).\n\
             \n\
             ## Database\n\
             - Use the MCP `db.*` / `dv_*` tools or the REST gateway \
             `/api/dv/{slug}/<table>` for every read/write.\n\
             - Raw SQL is not supported on the postgres-dataverse backend.\n\
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
             - Inspect logs via `app.logs` and the Atelier logs page.\n",
            name = app.name,
            stack = app.stack,
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
         A single MCP server is configured: `studio`. Read-only tools are \
         auto-approved for the interactive Studio user (user-level settings) — \
         mutations require explicit confirmation.\n\
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
         ## Database (`db.*` schema-ops + `dv.*` runtime)\n\
         - `db.tables` — list tables for `{slug}` (or any app)\n\
         - `db.schema` — describe a table\n\
         - `dv.schema` — full Dataverse schema (tables, columns, relations)\n\
         - `dv.list` — read rows ($filter/$select/$expand/$orderby/$top/$skip/$count)\n\
         - `dv.get` — single row by id\n\
         - `dv.insert`, `dv.update`, `dv.soft_delete`, `dv.restore` — writes (not auto-approved, audit logged)\n\
         - `dv.audit_list` — who changed what/when\n\
         - Schema mutations (`db.create_table`, `db.add_column`, `db.create_relation`, `db.drop_table`, `db.remove_column`) — not auto-approved\n\
         - **No GraphQL, no raw SQL.** See `.claude/rules/db.md`.\n\
         \n\
         ## Build\n\
         Pour builder cette app, utilise la skill **0-build** (lazy-loaded). Elle compile **en local sur Medion** (toolchain locale) via Bash et notifie le badge de build du Studio.\n\
         \n\
         ## Plateforme (tes « pattes » vers Atelier et l'utilisateur)\n\
         - `ship` — livraison prod : stop + restart pour reprendre les artefacts compilés par 0-build (aucune compilation). `BUILD_BUSY` = un build/ship est en cours, ne PAS retry.\n\
         - `notify_user` — notifie Romain (cloche + appareils). Réservé à ce qui mérite VRAIMENT son attention (décision, anomalie, résultat inattendu). Tes actions plateforme (restart, ship, env, schéma) sont déjà **journalisées automatiquement** — ne notifie pas pour ça.\n\
         - `issue_report` — remonte une friction PLATEFORME (tool MCP qui bug/manque, doc trompeuse, build/deploy/dataverse qui déraille côté Atelier) ou une suggestion d'amélioration (`kind: error|limitation|suggestion`). Voir `.claude/rules/report-issues.md`.\n\
         \n\
         ## Environment (`env_*`)\n\
         Le `.env` est un **artefact généré** — ne JAMAIS l'éditer à la main. Une variable modifiée n'est vue par le process qu'au prochain restart.\n\
         - `env_list` — toutes les variables (tier plateforme calculé + tier user) ; les valeurs secrètes sont TOUJOURS masquées ici\n\
         - `env_set(key, value, secret?, scope?)` — crée/remplace une variable user puis régénère le `.env`. `scope: runtime|build|both` (build = exposée à la commande de build, canal `VITE_*`/`NEXT_PUBLIC_*`). Clés plateforme (PORT, HR_DV_*, ATELIER_*) refusées.\n\
         - `env_delete(key)` — supprime une variable user\n\
         - `app.update env_vars` est **deprecated** (merge legacy) — utilise `env_set`.\n\
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

/// Self-contained bash script that runs the build LOCALLY on Medion
/// (where the agent + sources live) and emits status events to the Studio's
/// per-app live panel via the Atelier API.
///
/// The agent sees the cargo/pnpm output streamed in its terminal; the Studio
/// sees only the start/end milestones (no log forwarding).
fn render_app_build_script(app: &Application) -> String {
    let build_command = app
        .build_command
        .as_deref()
        .unwrap_or("echo 'no build_command configured'; exit 1");
    let template = r#"#!/usr/bin/env bash
# Build local de l'app `__SLUG__` : compile en place sur Medion (sources + toolchain locales).
# Émet des events au Studio (badge per-app live) via /api/apps/__SLUG__/build-event.
# Géré par Atelier — ne pas éditer (régénéré à chaque AppUpdate).
set -euo pipefail
# Toolchain sur PATH : l'agent tourne sous `sudo -u hr-studio` qui réinitialise l'env
# vers son secure_path, donc `cargo` (~/.cargo/bin) en est absent. On le rajoute ici
# pour que `cargo build` aboutisse (idempotent ; sans effet si déjà présent).
export PATH="${HOME:-/var/lib/hr-studio}/.cargo/bin:${HOME:-/var/lib/hr-studio}/.local/bin:$PATH"
API_BASE="${API_BASE:-http://127.0.0.1:4100}"
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

emit '{"status":"started","phase":"compile","message":"local build"}'
START=$(date +%s)
echo "=== Build local: $SLUG ==="
echo "Cwd: $SRC_DIR"
cd "$SRC_DIR"
export CI=true NPM_CONFIG_FUND=false
# Variables build-scoped (VITE_*/NEXT_PUBLIC_*) injectées par Atelier (vide sinon).
eval "$(curl -sS --max-time 5 "$API_BASE/api/apps/$SLUG/build-env" 2>/dev/null || true)"
__BUILD_COMMAND__
ELAPSED_MS=$(( ($(date +%s) - START) * 1000 ))
emit "{\"status\":\"finished\",\"phase\":\"compile\",\"duration_ms\":$ELAPSED_MS,\"message\":\"build OK (local)\"}"
echo "=== Build OK ($ELAPSED_MS ms) ==="
echo "En fin de feature validée, livre en prod via le tool MCP ship (cf. skill 0-deploy)"
"#;
    template
        .replace("__SLUG__", &app.slug)
        .replace("__BUILD_COMMAND__", build_command)
}

/// Skill `0-deploy` : pousse les artefacts pre-buildés vers Medion + restart.
fn render_app_deploy_skill(app: &Application) -> String {
    format!(
        "---\n\
         name: 0-deploy\n\
         description: Livre en prod l'app `{slug}` via le tool MCP `ship` (stop + restart pour reprendre l'artefact buildé). Utilise cette skill EN FIN DE FEATURE VALIDÉE (livraison systématique, sans demander l'autorisation), et aussi quand l'utilisateur demande de déployer/livrer/ship. Toujours APRÈS un `0-build` réussi.\n\
         ---\n\
         \n\
         # Deploy de l'app `{slug}`\n\
         \n\
         Appelle le tool MCP **`ship`** (serveur `studio`). Il **recharge** le process supervisé (stop → restart) pour qu'il reprenne les artefacts déjà compilés en place par `0-build`. **Pas de compile, pas de copie distante** : tout est local sur Medion (un rsync depuis un build host n'a lieu que si `ATELIER_BUILD_HOST` est défini, ce qui n'est pas le cas par défaut). La livraison est journalisée automatiquement côté Atelier.\n\
         \n\
         ## Pré-requis\n\
         \n\
         - Avoir lancé `bash .claude/skills/0-build/build.sh` avec succès dans cette session ou une précédente.\n\
         - Les artefacts (`build_artefact` de l'app) doivent exister sous `src/`.\n\
         \n\
         ## Appel\n\
         \n\
         Tool MCP `ship` — sans argument (timeout optionnel `timeout_secs`, défaut 900, clampé 60..=7200).\n\
         \n\
         ## Retour\n\
         \n\
         JSON `{{ ok, stages, summary, duration_ms }}`. Étapes émises au badge Studio : `stop` → `restart` (un `rsync-back` ne s'intercale que si un build host distant est configuré). Une erreur du pipeline est renvoyée comme erreur du tool (l'app peut être down → `app.status`/`app.logs`).\n\
         \n\
         ## Workflow type\n\
         \n\
         1. `bash .claude/skills/0-build/build.sh`  (build local sur Medion, voir output cargo)\n\
         2. Tool MCP `ship`  (stop + restart sur Medion)\n\
         3. Vérifier `app.status` = `running`.\n\
         \n\
         ## Erreur BUILD_BUSY\n\
         \n\
         Un autre build/ship pour `{slug}` est déjà en cours. NE PAS RETRY automatiquement — informer l'utilisateur et attendre.\n",
        slug = app.slug,
    )
}

/// Conventions de code génériques, identiques pour toutes les apps quelle que
/// soit la stack. Elles portent le contrat plateforme (ce que l'app DOIT
/// respecter pour être servie/supervisée) + les invariants de cohérence
/// inter-projets. La structure interne du projet reste libre.
fn render_conventions_md(app: &Application) -> String {
    format!(
        "# Conventions — génériques, toutes stacks\n\
         \n\
         La plateforme est **stack-agnostique** : aucune structure de projet imposée, \
         aucun scaffold. En échange, chaque app respecte le contrat plateforme et ces \
         conventions de cohérence.\n\
         \n\
         ## Contrat plateforme (non négociable)\n\
         \n\
         - Le process écoute sur **`$PORT`** (livré par le `.env` rendu) — jamais de \
         port hardcodé.\n\
         - L'app est servie en même-origine sous **`/apps/{slug}/`** (path-proxy \
         Atelier). Pour un frontend : base path / assets relatifs à cette base \
         (`base` Vite, `basePath` Next, scope service-worker/PWA) — jamais de chemin \
         absolu `/...` qui suppose la racine du domaine.\n\
         - Un endpoint de santé répond 200 sur **`{health_path}`**.\n\
         - Configuration **exclusivement** via variables d'environnement (le `.env` \
         est rendu par Atelier — tools `env_set`/`env_list`, jamais d'édition à la \
         main). Aucun secret committé ou en dur dans le code.\n\
         - Logs sur **stdout/stderr** (captés par le superviseur).\n\
         - Données : passerelle dataverse (`HR_DV_*`) uniquement — jamais de \
         connexion Postgres directe, jamais de SQLite.\n\
         \n\
         ## Cohérence inter-projets\n\
         \n\
         - `README.md` à la racine de `src/` : ce que fait l'app, comment elle se \
         build, comment elle tourne.\n\
         - **Lockfile committé** (Cargo.lock, package-lock.json, uv.lock, …) : build \
         reproductible.\n\
         - Le build tient en **une commande** = le `build_command` du registre \
         (exécuté par la skill `0-build`). Production only, pas de mode dev.\n\
         - `run_command` / `build_command` / `health_path` / label `stack` **à jour \
         dans le registre** (tool MCP `app.update`) dès qu'ils changent — c'est le \
         registre qui pilote supervision et build, pas la doc.\n\
         - Structure de projet **idiomatique de l'écosystème choisi** : lisible pour \
         un dev de cette stack, sans chercher l'uniformité entre apps.\n",
        slug = app.slug,
        health_path = app.health_path,
    )
}

fn render_app_build_skill(app: &Application) -> String {
    let stack_label = if app.stack.trim().is_empty() {
        "stack non renseignée"
    } else {
        app.stack.as_str()
    };
    let build_cmd = app
        .build_command
        .as_deref()
        .filter(|c| !c.trim().is_empty());

    // Section rendue depuis les champs RÉELS de l'app (la plateforme est
    // stack-agnostique : aucun texte par-stack, le registre fait autorité).
    let stack_section = match build_cmd {
        Some(cmd) => {
            let artefacts = app
                .build_artefact
                .as_deref()
                .map(|a| {
                    a.lines()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty());
            let artefact_line = match artefacts {
                Some(list) => format!("- Artefacts déclarés (`build_artefact`) : {list}.\n"),
                None => "- Aucun artefact déclaré (`build_artefact` vide) : build en place, \
                         le restart reprend directement le résultat.\n"
                    .to_string(),
            };
            format!(
                "## Configuration de build de cette app\n\n\
                 - Commande de build (`build_command` du registre) : `{cmd}`.\n\
                 {artefact_line}\
                 - Pour changer commande/artefacts : tool MCP `app.update` \
                 (cette skill est régénérée automatiquement).\n",
            )
        }
        None => "## Configuration de build de cette app\n\n\
                 ⚠️ **Aucun `build_command` configuré** : `build.sh` échouera volontairement. \
                 Configure d'abord le build via le tool MCP `app.update` \
                 (`build_command`, et `run_command`/`health_path` si pas encore posés) — \
                 c'est toi qui possèdes la définition du build de cette app.\n"
            .to_string(),
    };

    format!(
        "---\n\
         name: 0-build\n\
         description: Build local de l'app {slug} ({stack}) sur Medion (toolchain locale, output cargo/pnpm en live). Utilise cette skill pour itérer/compiler pendant le dev ET en fin de feature validée (build puis 0-deploy, livraison systématique), ou quand l'utilisateur demande de builder/compiler/rebuild.\n\
         allowed-tools: Bash(bash .claude/skills/0-build/build.sh*)\n\
         ---\n\
         \n\
         # Build de l'app `{slug}` (local sur Medion)\n\
         \n\
         Cette skill compile l'app **directement** sur Medion (où vivent sources et toolchain). \
         L'output (cargo, pnpm, etc.) est visible en live dans ton terminal. Le Studio est notifié en parallèle via `/api/apps/{slug}/build-event` pour afficher l'état dans le panel per-app.\n\
         \n\
         **Important** : ce build compile **en place** mais ne RECHARGE PAS le process en cours (qui tourne encore sur l'ancien artefact). Pour reprendre le nouvel artefact en prod (stop + restart), enchaîne ensuite avec le tool MCP **`ship`** (cf. skill `0-deploy`).\n\
         \n\
         ## Commande\n\
         \n\
         ```bash\n\
         bash .claude/skills/0-build/build.sh\n\
         ```\n\
         \n\
         Tu peux itérer sans deploy : edit → build → re-edit → re-build. Tant que tu \
         n'as pas fait `0-deploy`, le runtime Medion ne change pas — **volontaire \
         pendant l'itération**. Mais **en fin de feature validée, 0-deploy est \
         systématique** (livraison libre, sans demander) : le build seul ne suffit pas \
         à livrer.\n\
         \n\
         ## Retour\n\
         \n\
         Le script affiche l'output cargo/pnpm en stream + un événement `started` puis `finished` (ou `error`) émis au Studio. Exit code 0 si le build passe.\n\
         \n\
         ## Workflow type\n\
         \n\
         1. `bash .claude/skills/0-build/build.sh`  (compile en place, voir l'output cargo/pnpm)\n\
         2. Itérer si besoin (fix erreurs, re-build)\n\
         3. Tool MCP `ship`  (stop + restart pour reprendre l'artefact — cf. skill `0-deploy`)\n\
         \n\
         ## Interdits\n\
         \n\
         - **JAMAIS** rebuilder à la main hors de cette skill : le PATH toolchain et les events Studio sont déjà gérés par `build.sh`.\n\
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
        Visibility::Private => "Private (Atelier auth required)",
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
            stack = other.stack,
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
         - **Commande de build (Medion) :** `{build}`\n\
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
        stack = app.stack,
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
         _Ajoute tes notes sous cette ligne. Atelier ne réécrira jamais ce \
         fichier (sauf demande explicite via `AppRegenerateContext` avec un \
         futur flag `force_claude_md`)._\n",
        name = app.name,
    )
}

/// Rule pour la surveillance IA : le modèle 3 scans + le rituel de maintenance
/// du scan Business + pointeurs MCP + convention de commit. Le détail du workflow
/// vit dans la skill `0-surveillance`.
fn render_surveillance_rule_md(app: &Application) -> String {
    format!(
        "# Surveillance IA — {slug}\n\
         \n\
         Cette app a **trois scans** (tous tournent en lecture seule via le \
         scan-agent — Claude Agent SDK — et écrivent des findings, visibles dans le \
         tab **Surveillance** du Studio) :\n\
         \n\
         - **Sécurité** et **Qualité** (bugs/qualité code/perf) — scans **plateforme \
         FIXES**. Tu ne les configures PAS ; tu **tries** leurs findings (résous / \
         dismiss). Tu peux les lancer depuis l'UI ou via `surveillance_run`.\n\
         - **Business** — le **SEUL scan que TU possèdes** et fais évoluer (aucune \
         validation humaine). Il porte sur les **données et le comportement métier \
         réels** de l'app. **Vide par défaut** : tant que tu ne l'as pas défini, il \
         est en veille (aucun run).\n\
         \n\
         ## Rituel à CHAQUE session (scan Business)\n\
         \n\
         1. **`scan_get`** — relis la définition actuelle de ton scan Business.\n\
         2. Au vu de l'évolution du projet, mets-la à jour avec **`scan_set`** : \
         `label`, `prompt`, `cadence` (manual|daily|weekly), `gate` (code|data|manual), \
         `gate_sql` (un SELECT scalaire de fraîcheur **adapté au schéma de CETTE app** \
         si gate=data), `categories` (tes axes). Le `prompt` doit contenir les slots \
         `{{{{SLUG}}}}`, `{{{{CATEGORIES}}}}`, `{{{{DIFF}}}}`, `{{{{MEMORY}}}}`, \
         `{{{{OPEN_COUNT}}}}`, `{{{{REMAINING}}}}`, `{{{{MAX_OPEN}}}}`. Conçois-le pour \
         CETTE app — pas de template générique.\n\
         3. Maintiens le contexte support dans **`.claude/rules/`** (invariants \
         métier, pièges, schéma data) — ce que ton scan doit savoir pour être pertinent.\n\
         \n\
         > Un bon scan Business répond à une question utile et RÉCURRENTE propre à ce \
         projet. Le `prompt` doit demander au scan-agent : (1) **d'abord** lire les \
         findings existantes via `findings_list(kind=\"business\", status=\"open\")` et \
         de les **trier** — garder / mettre à jour (`findings_upsert` même `fingerprint`) \
         / **supprimer** (`findings_delete` si la cause n'existe plus, vérifiée) ; puis \
         (2) émettre les nouvelles findings via `findings_upsert(kind=\"business\", \
         category=<une de tes categories>, severity, title, summary, plan, fingerprint)`, \
         en fingerprintant par CAUSE et en restant dans le budget `{{{{REMAINING}}}}`.\n\
         \n\
         ## Forme d'une finding (les 3 scans)\n\
         \n\
         - `title` + `summary` = la **présentation** de l'issue (c'est tout ce qui \
         s'affiche dans la liste). Garde le `summary` court.\n\
         - `plan` = un **document de résolution complet** (annexe, ouverte à la \
         demande) : `## Contexte` / `## Cause racine` / `## Fichiers impactés` / \
         `## Étapes de correction` / `## Validation`. Pas 2-3 steps condensés.\n\
         \n\
         ## Tools MCP\n\
         \n\
         - `scan_get`, `scan_set` — lire / définir ton scan **Business** (les scans \
         Sécurité/Qualité ne se règlent pas ici).\n\
         - `surveillance_run(kind=security|code_review|business)` — lancer un scan.\n\
         - `findings_list` (filtre kind/severity/status) — **à lire en premier** \
         pour trier l'existant.\n\
         - `findings_dismiss` (faux positif à mémoriser), `findings_resolve` (fix \
         committé), `findings_delete` (cause **disparue** — fichier/fonction supprimé, \
         refactoré, faux positif que le code ne déclenche plus ; **définitif**, \
         vérifie avant).\n\
         - `memory_get`, `memory_remember` (préférences/mémoire durables)\n\
         - `runs_list` (historique des runs), `pm_query` (SELECT read-only sur la base de l'app)\n\
         \n\
         ## Convention de commit\n\
         \n\
         Quand tu corriges une finding, commit avec le message \
         `fix(surveillance:<id>): <résumé>`. Atelier détecte ce pattern et marque la \
         finding `resolved` automatiquement. Appelle aussi `findings_resolve(id, commit_sha)`.\n",
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
     - Build → skill `0-build`\n\
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

fn render_settings_json() -> String {
    // settings.json ne porte QUE `enabledMcpjsonServers` : la *définition* du serveur
    // `studio` (type/url/headers) est déclarée une seule fois dans `.mcp.json` (source de
    // vérité), settings.json ne fait que la PRÉ-APPROUVER (sinon prompt de trust à la 1re
    // session, bloquant pour le runner non-interactif). Recopier le bloc `mcpServers` ici
    // serait redondant (.mcp.json étant déjà la déclaration active) et imposerait de
    // maintenir url+token à deux endroits.
    //
    // PAS de bloc `permissions` ici : le runner agent (runner.js) charge ce fichier via
    // settingSources:['project'], et une allow rule court-circuiterait son canUseTool
    // (vérifié : `mcp__studio` en allow exécute `exec` EN ROOT même en mode plan).
    // L'auto-approve `mcp__studio` des sessions interactives Studio vit dans les settings
    // USER de hr-studio (/var/lib/hr-studio/.claude/settings.json), source que le runner
    // ne charge jamais.
    let settings = serde_json::json!({
        "enabledMcpjsonServers": ["studio"],
    });
    serde_json::to_string_pretty(&settings).expect("settings JSON serializes")
}

fn render_mcp_json_with_auth(mcp_endpoint: &str, token: Option<&str>) -> String {
    let mcp = serde_json::json!({
        "mcpServers": {
            "studio": mcp_server_entry(mcp_endpoint, token),
        }
    });
    serde_json::to_string_pretty(&mcp).expect("mcp JSON serializes")
}

/// Resolve the rules-files group GID (default `hr-studio`, overridable via
/// `ATELIER_RULES_GROUP`). The agent runner runs as `hr-studio`; making
/// regenerated rules group-writable for that gid lets agents edit them
/// without sudo. Returns `None` if the group doesn't exist on the host
/// or `getent` is unavailable — callers degrade to a `warn!` and continue.
fn resolve_rules_group_gid() -> Option<u32> {
    use std::sync::OnceLock;
    static CACHED: OnceLock<Option<u32>> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let group = std::env::var("ATELIER_RULES_GROUP")
            .unwrap_or_else(|_| "hr-studio".to_string());
        let output = std::process::Command::new("getent")
            .arg("group")
            .arg(&group)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let line = String::from_utf8(output.stdout).ok()?;
        // Format `hr-studio:x:1001:...` — 3rd colon-field is the gid.
        line.split(':').nth(2)?.trim().parse::<u32>().ok()
    })
}

/// Resolve the rules-files group NAME (default `hr-studio`, overridable via
/// `ATELIER_RULES_GROUP`). Used by `setfacl` calls — we pass the name so
/// `getfacl` output is readable. If the group doesn't exist locally,
/// `setfacl` will fail silently (handled as warn) which is fine for the
/// fail-soft contract.
fn resolve_rules_group_name() -> String {
    std::env::var("ATELIER_RULES_GROUP").unwrap_or_else(|_| "hr-studio".to_string())
}

/// Force an ACL on a freshly-written rules file so the agent group keeps
/// `rwx` (via mask) even when a chmod-set group bit would otherwise clamp
/// the mask. Best-effort: if `setfacl` is missing or the group doesn't
/// exist, log and continue.
fn apply_rules_acl_file(path: &Path) {
    let group = resolve_rules_group_name();
    let spec = format!("u::rw-,g::rw-,g:{group}:rw-,o::r--,m::rwx");
    let output = std::process::Command::new("setfacl")
        .arg("-m")
        .arg(&spec)
        .arg(path)
        .output();
    match output {
        Ok(o) if !o.status.success() => {
            warn!(
                path = %path.display(),
                stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                "setfacl file failed (non-fatal)"
            );
        }
        Err(e) => {
            warn!(path = %path.display(), err = %e, "setfacl file spawn failed (non-fatal)");
        }
        _ => {}
    }
}

/// Force ACL + default ACL on a rules dir so files created inside inherit
/// `g:<rules-group>:rwx` with `mask::rwx`. Same fail-soft contract.
fn apply_rules_acl_dir(path: &Path) {
    let group = resolve_rules_group_name();
    let access = format!("u::rwx,g::rwx,g:{group}:rwx,o::rx,m::rwx");
    let default = format!("u::rwx,g::rwx,g:{group}:rwx,o::rx,m::rwx");
    let output = std::process::Command::new("setfacl")
        .args(["-m", &access, "-d", "-m", &default])
        .arg(path)
        .output();
    match output {
        Ok(o) if !o.status.success() => {
            warn!(
                path = %path.display(),
                stderr = %String::from_utf8_lossy(&o.stderr).trim(),
                "setfacl dir failed (non-fatal)"
            );
        }
        Err(e) => {
            warn!(path = %path.display(), err = %e, "setfacl dir spawn failed (non-fatal)");
        }
        _ => {}
    }
}

/// Apply ACL-friendly perms to a freshly-created rules dir: setgid +
/// group-writable so children inherit the group and are editable. Best-
/// effort `chgrp` to the resolved rules group + explicit `setfacl` to
/// guarantee the mask doesn't clamp the group entry. All failures degrade
/// to a `warn!` — the regeneration must not fail because of permission tweaks.
fn apply_rules_dir_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o2775)) {
        warn!(path = %path.display(), err = %e, "set_permissions dir failed");
    }
    if let Some(gid) = resolve_rules_group_gid() {
        if let Err(e) = std::os::unix::fs::chown(path, None, Some(gid)) {
            warn!(path = %path.display(), gid, err = %e, "chown dir failed");
        }
    }
    apply_rules_acl_dir(path);
}

/// Apply group-writable perms (0o664) + best-effort `chgrp` to a freshly-
/// written rules file. Same fail-soft contract as `apply_rules_dir_perms`.
fn apply_rules_file_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o664)) {
        warn!(path = %path.display(), err = %e, "set_permissions file failed");
    }
    if let Some(gid) = resolve_rules_group_gid() {
        if let Err(e) = std::os::unix::fs::chown(path, None, Some(gid)) {
            warn!(path = %path.display(), gid, err = %e, "chown file failed");
        }
    }
    apply_rules_acl_file(path);
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
    apply_rules_file_perms(path);
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

/// Règle `report-issues.md` : QUAND/COMMENT remonter une friction PLATEFORME à
/// Atelier (vs un bug interne de l'app). Toujours chargée par l'agent via
/// `settingSources=['project']`. Pilote la skill `0-report-issue`.
fn render_report_issues_rule_md(app: &Application) -> String {
    format!(
        "# Remonter les soucis plateforme à Atelier (`report-issues`)\n\
         \n\
         > Quand un souci que tu rencontres relève de **la plateforme Atelier** (et non du code de ton app `{slug}`), tu DOIS le remonter via la skill `0-report-issue`. Ces remontées sont **centralisées côté Atelier** (dans son control-plane, **hors de ton dépôt**) et relues par le développeur d'Atelier. Sans ça le souci se perd : tu contournes en silence et personne ne le corrige jamais.\n\
         \n\
         ## QUAND remonter\n\
         \n\
         Un souci **plateforme** = quelque chose qui te bloque ou te ralentit et qui ne se corrige PAS dans le code de `{slug}` :\n\
         \n\
         - un **tool MCP** qui échoue, renvoie une erreur opaque, ou qui **manque** pour faire ton travail ;\n\
         - une **doc Atelier** (rules, descriptions de skills, contexte généré) fausse, périmée ou trompeuse ;\n\
         - un **build / deploy** (`0-build`/`0-deploy`) qui casse pour une raison **plateforme** (toolchain absente du PATH, endpoint, permission), pas pour une erreur de ton code ;\n\
         - la **passerelle dataverse** (`dv_*` / `db_*` / REST `/api/dv`) qui se comporte mal ;\n\
         - l'**agent / le Studio** lui-même (comportement inattendu, capacité absente).\n\
         \n\
         Une **suggestion d'amélioration plateforme** est aussi bienvenue (`kind: suggestion`) : un tool/une capacité qui te manquerait, une doc à enrichir, un workflow qui pourrait être plus fluide — même sans blocage.\n\
         \n\
         ## QUAND NE PAS remonter\n\
         \n\
         - Les **bugs internes de ton app** `{slug}` → corrige-les, ou note-les dans `CLAUDE.md`.\n\
         - Les **findings de surveillance** → ils ont leur propre canal (`findings_*`, cf. `surveillance.md`).\n\
         - Une incompréhension que la doc résout déjà → relis la doc avant de remonter.\n\
         \n\
         ## COMMENT remonter\n\
         \n\
         1. Appelle le tool MCP **`issue_report(title, kind?, area?, severity?, context?, tried?)`** (la skill `0-report-issue` documente les champs) : Atelier enregistre la remontée côté plateforme.\n\
         2. **Ne stocke rien toi-même** (pas de fichier dans ton dépôt) : Atelier est l'unique writer, il estampe id + horodatage + statut côté serveur, dans son control-plane centralisé.\n\
         3. **Dis-le à l'utilisateur** en une phrase (« j'ai remonté un souci plateforme : … ») — pas de remontée silencieuse.\n\
         \n\
         ## Barème\n\
         \n\
         - `kind` : `error` (un truc plateforme est cassé) · `limitation` (ça marche mais ça te bride) · `suggestion` (idée d'amélioration, rien de cassé). Défaut `error`.\n\
         - `severity` : `low` (gênant mais contournable) · `medium` (ralentit nettement) · `high` (bloque le travail). Pour une `suggestion` : l'impact qu'aurait l'amélioration.\n\
         - `area` : `mcp` · `docs` · `build` · `deploy` · `dataverse` · `agent` · `studio-ui` · `platform` · `other`.\n\
         \n\
         Donne un `title` court et actionnable, un `context` (ce que tu faisais + le symptôme exact) et `tried` (ce que tu as tenté / le contournement en place).\n",
        slug = app.slug,
    )
}

/// Skill `0-report-issue` (SKILL.md seul — le tool MCP `issue_report` a
/// remplacé l'ancien script curl+jq).
fn render_report_issue_skill(app: &Application) -> String {
    format!(
        "---\n\
         name: 0-report-issue\n\
         description: Remonte un souci PLATEFORME (Atelier) rencontré en travaillant sur l'app {slug} — erreur, limitation ou suggestion d'amélioration : tool MCP, doc, build/deploy, dataverse, agent. Utilise cette skill QUAND tu butes sur une friction qui ne relève PAS du code de {slug} mais de la plateforme, OU quand tu as une idée d'amélioration plateforme. NE concerne PAS les bugs internes de l'app.\n\
         ---\n\
         \n\
         # Remonter un souci plateforme — `{slug}`\n\
         \n\
         Appelle le tool MCP **`issue_report`** (serveur `studio`) : Atelier enregistre la remontée dans son control-plane (centralisé, **hors de ton dépôt**). Tu ne stockes rien toi-même. Voir `.claude/rules/report-issues.md` pour QUAND remonter (et quand NE PAS).\n\
         \n\
         ## Champs\n\
         \n\
         - `title` (**requis**) : court et actionnable. Ex. « docs_search renvoie 500 sur requête vide ».\n\
         - `kind` : `error|limitation|suggestion` (défaut `error`) — `error` = cassé, `limitation` = ça bride, `suggestion` = idée d'amélioration.\n\
         - `area` : `mcp|docs|build|deploy|dataverse|agent|studio-ui|platform|other` (défaut `other`).\n\
         - `severity` : `low|medium|high` (défaut `medium`).\n\
         - `context` / `tried` : optionnels mais utiles (symptôme exact + contournement en place).\n\
         \n\
         ## Retour\n\
         \n\
         Le JSON de l'entrée stockée (`id`, `ts`, `status:\"open\"`, …). Mentionne ensuite à l'utilisateur, en une phrase, que tu as remonté le souci.\n\
         \n\
         ## Interdits\n\
         \n\
         - **Ne stocke pas la remontée toi-même** (aucun fichier dans le dépôt) — Atelier la centralise côté plateforme.\n\
         - Ne remonte pas un bug interne de l'app ici (corrige-le ou note-le dans `CLAUDE.md`).\n",
        slug = app.slug,
    )
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

/// Skills additionnelles (read-only) en plus de `0-build`. Chaque entrée est
/// (nom_skill, contenu_complet_avec_frontmatter). Le nom devient le dossier
/// `src/.claude/skills/<nom>/SKILL.md`.
fn render_extra_skills(app: &Application) -> Vec<(&'static str, String)> {
    let mut skills = vec![
        ("0-status", format!(
            "---\n\
             name: 0-status\n\
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
        ("0-logs", format!(
            "---\n\
             name: 0-logs\n\
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

    // ── Surveillance : 3 scans (security/code_review fixes + business possédé
    // par l'agent). Une skill consolidée couvre la MAINTENANCE du scan Business
    // (scan_get/scan_set) et le TRAITEMENT des findings des 3 scans. ──
    skills.push(("0-surveillance", format!(
        "---\n\
         name: 0-surveillance\n\
         description: Surveillance de l'app {slug} — maintenir le scan BUSINESS (scan_set) et TRAITER les findings des 3 scans (Sécurité, Qualité, Business). Utilise-moi quand l'utilisateur dit \"surveillance\", \"définis/mets à jour le scan\", \"/scan\", \"traite les findings\", ou en début de session.\n\
         allowed-tools: \n\
         ---\n\
         \n\
         # Surveillance — `{slug}`\n\
         \n\
         Trois scans : **Sécurité** + **Qualité** (plateforme, fixes — tu ne les configures pas) et **Business** (tu le possèdes, aucune validation humaine). Vois `.claude/rules/surveillance.md`.\n\
         \n\
         ## Maintenir le scan Business (début de session / quand le projet évolue)\n\
         1. `scan_get` — lis la définition actuelle (`blank=true` = pas encore défini).\n\
         2. Décide ce que ce projet a besoin de surveiller de façon RÉCURRENTE dans ses **données/comportement métier** (une question utile qu'un build ne couvre pas). Puis `scan_set` : `label`, `prompt` (slots `{{{{SLUG}}}}`/`{{{{CATEGORIES}}}}`/`{{{{DIFF}}}}`/`{{{{MEMORY}}}}`/`{{{{OPEN_COUNT}}}}`/`{{{{REMAINING}}}}`/`{{{{MAX_OPEN}}}}`, et `findings_upsert(kind=\"business\", …)`), `cadence`, `gate` (+ `gate_sql` adapté au schéma de l'app si `data`), `categories`. Conçois-le pour CETTE app.\n\
         3. Maintiens aussi le contexte support dans `.claude/rules/` (ce que le scan doit savoir).\n\
         \n\
         ## Traiter les findings (les 3 scans)\n\
         1. `findings_list` (`status=open`, filtre `kind` au besoin), trie par sévérité décroissante.\n\
         2. Pour chaque finding : la liste montre titre+summary ; ouvre le `plan` (document de résolution) ; demande confirmation ; implémente ; vérifie (build/typecheck) ; commit `fix(surveillance:<id>): <résumé>` puis `findings_resolve(id, commit_sha)`. Faux positif → `findings_dismiss(id, reason)`.\n\
         3. Récap : N résolues, M dismiss, K ouvertes.\n\
         \n\
         **Ne lance JAMAIS `make deploy` / `make deploy-app` toi-même** (commandes \
         d'Atelier, hors périmètre d'une app). Pour livrer un fix, passe par `0-build` \
         puis `0-deploy` (comme toute fin de feature) : la livraison est libre. Le \
         commit `fix(surveillance:<id>)` reste, lui, décidé par l'utilisateur.\n",
        slug = app.slug,
    )));

    if app.has_db {
        skills.push(("0-db-info", format!(
            "---\n\
             name: 0-db-info\n\
             description: Donne un résumé de la base postgres-dataverse de l'app {slug} (tables, colonnes, row counts). Utilise-moi quand l'utilisateur demande ce qu'il y a en base, le schéma, ou un aperçu des données.\n\
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

    skills
}

/// Slash-commands & fichiers legacy à nettoyer à chaque régénération.
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
/// de l'app change.
const ALL_EXTRA_SKILL_NAMES: &[&str] = &[
    // Anciens noms (modèles précédents) à supprimer à la régénération. Les noms
    // courants sont préfixés `0-` (voir render_extra_skills + app-build/deploy).
    "app-build",
    "app-deploy",
    "app-status",
    "app-logs",
    "app-db-info",
    "flow-build",
    "surveillance",
    "surveillance-bugs",
    "surveillance-improvements",
    "surveillance-security",
];

/// Fichiers `rules/*.md` obsolètes à nettoyer à chaque génération.
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
    "store-publishing.md",
    "flows-first.md",
    "todos.md",
];

/// Nettoie les fichiers de contexte agent qui traînent au niveau `app_dir`
/// (au-dessus de `src/`).
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
             - Connexion : **gateway-only** via `HR_DV_BASE_URL` + `HR_DV_TOKEN` (PAS de `DATABASE_URL`, pas de SQL direct)\n\
             - Surfaces : REST OData `/api/dv/{slug}/<table>` (app), tools MCP `dv_*` (agent)\n\
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
        "# Base de données — PostgreSQL + Dataverse\n\
         \n\
         Cette app (`{slug}`) utilise la stack **Atelier Dataverse** :\n\
         \n\
         - **PostgreSQL 18** sur Medion :5432, base dédiée `app_{slug}` (rôle\n\
           `app_{slug}` aux droits limités à cette base)\n\
         - **Connexion runtime** : **gateway-only**. L'app n'a PAS de `DATABASE_URL`\n\
           ni d'accès SQL direct (pas de sqlx/prisma/tokio-postgres) — toute lecture/\n\
           écriture passe par la passerelle REST `/api/dv/{slug}` avec\n\
           `HR_DV_BASE_URL` + `HR_DV_TOKEN` (injectés dans l'env).\n\
         \n\
         Deux surfaces officielles pour parler à la base, selon le contexte :\n\
         **REST OData-style** (app runtime) et **MCP `dv_*`** (agent / debug).\n\
         Pas de GraphQL, pas de flows TOML (système éradiqué 2026-05-26).\n\
         \n\
         ## Côté app runtime — REST OData-style\n\
         \n\
         Routes exposées sous `/api/dv/{slug}/{{table}}` (authentifié par bearer\n\
         token de l'app) :\n\
         \n\
         | Verbe | Endpoint | Headers | Effet |\n\
         |---|---|---|---|\n\
         | `GET` | `/api/dv/{slug}/{{table}}` | `Authorization: Bearer $HR_DV_TOKEN` | list (200, `{{rows, count?}}`) |\n\
         | `GET` | `/api/dv/{slug}/{{table}}/{{id}}` | idem | single row (200, `ETag` = version) |\n\
         | `POST` | `/api/dv/{slug}/{{table}}` | idem + body JSON | insert (201, row) |\n\
         | `PATCH` | `/api/dv/{slug}/{{table}}/{{id}}` | `+ If-Match: <version>` | update (200, row) |\n\
         | `DELETE` | `/api/dv/{slug}/{{table}}/{{id}}` | `+ If-Match: <version>` | soft-delete (200) |\n\
         | `POST` | `/api/dv/{slug}/{{table}}/$restore/{{id}}` | `+ If-Match: <version>` | restore (200, row) |\n\
         \n\
         **`If-Match` obligatoire** pour update/delete/restore — sinon 400 (optimistic lock).\n\
         \n\
         Query params (sur GET) : `$filter`, `$select`, `$orderby`, `$top`, `$skip`,\n\
         `$count`, `$includeDeleted`, `$expand`. Le `$filter` est parsé en\n\
         **dvexpr** (dialect propriétaire) : `==`, `!=`, `<`, `>`, `<=`, `>=`,\n\
         `&&`, `||`, `contains(...)`, `startswith(...)`, `endswith(...)`.\n\
         Exemple : `?$filter=active == true && contains(email, \"@example.com\")`.\n\
         \n\
         ## Côté agent — MCP tools `dv_*`\n\
         \n\
         Pour explorer/debug depuis l'IDE :\n\
         \n\
         - `dv_schema` — introspect tables, colonnes, relations\n\
         - `dv_list` — read avec `$filter`/`$select`/`$expand`/`$orderby`/`$top`/`$skip`/`$count`\n\
         - `dv_get` — single row read\n\
         - `dv_insert` — insert + audit\n\
         - `dv_update` — patch + optimistic lock + audit\n\
         - `dv_soft_delete` — soft delete + audit\n\
         - `dv_restore` — restore soft-deleted row + audit\n\
         - `dv_audit_list` — qui a changé quoi/quand sur la table\n\
         \n\
         ## Schema-ops (création/évolution de tables)\n\
         \n\
         - `db_create_table`, `db_add_column`, `db_create_relation`,\n\
           `db_drop_table`, `db_remove_column`, `db_set_display_column` — outils\n\
           MCP pour faire évoluer le schéma. Créent tables avec trigger\n\
           `updated_at`, FK natives, types Dataverse riches.\n\
         \n\
         ## 🔤 Colonne d'affichage primaire (lookups)\n\
         \n\
         Quand une table est **référencée par un Lookup**, l'UI (explorateur,\n\
         sélecteurs) et `$expand` affichent sa **colonne d'affichage primaire** à\n\
         la place de l'id brut. Invariant : **chaque table** a une colonne\n\
         d'affichage primaire **explicitement définie** (jamais implicite) —\n\
         y compris `id` si aucune colonne texte ne convient. La plateforme la\n\
         renseigne d'office à la création (et backfille les tables existantes).\n\
         \n\
         - **Par défaut**, elle est déduite par heuristique : 1re colonne texte\n\
           nommée `name`, puis `title`, puis `label`, puis la 1re colonne texte ;\n\
           à défaut (aucune colonne texte) → `id`. Cette valeur est **épinglée**\n\
           explicitement, pas recalculée à chaque lecture.\n\
         - Nommer cette colonne `name` est **recommandé quand c'est naturel** —\n\
           mais **non obligatoire**.\n\
         - Pour la changer : `db_set_display_column` (`table` + `column` texte,\n\
           ou `id`, ou `null` pour recalculer+épingler le défaut).\n\
         - Côté app : `GET /api/dv/{slug}/<table>?$expand=<colonne_lookup>` renvoie\n\
           `<colonne_lookup>_display` (le libellé de la cible) **à côté** de l'id\n\
           brut — plus besoin de charger une table de correspondance en mémoire.\n\
         \n\
         ## ❌ Ne PAS faire\n\
         \n\
         - **Pas de GraphQL.** L'ancienne surface `db_graphql` / `db_introspect`\n\
           a été supprimée. Utilise REST (app) ou MCP `dv_*` (agent).\n\
         - **Pas d'écriture SQL brute.** `db_exec` est refusé (utilise\n\
           `dv_insert`/`dv_update`/`dv_soft_delete`). En revanche `db_query` **fonctionne**\n\
           pour la **lecture** : c'est un SELECT-only (JOIN/agrégats/`_dv_audit` OK,\n\
           toute mutation rejetée) — idéal pour les diagnostics.\n\
         - **Pas d'ouverture directe d'un fichier `.db`** — il n'y en a plus.\n\
         \n\
         ## 🧹 Nettoyage post-migration\n\
         \n\
         Si tu trouves dans le code de l'app **des restes de l'ancienne stack SQLite**\n\
         (chemins `db.sqlite`, dépendances `rusqlite`, calls vers `db_query`/`db_exec`\n\
         SQL brut, mentions de `atelier-db` legacy, fixtures de migration `_dv_*`\n\
         SQLite-flavored), **supprime-les** : la migration est faite, ces restes ne\n\
         servent à rien.\n\
         \n\
         Si tu trouves dans la doc / `CLAUDE.md` de l'app des phrases du type\n\
         « migration Postgres en attente », **supprime-les aussi** — la migration a eu\n\
         lieu et la règle « post-migration » que tu lis maintenant le confirme.\n\
         \n\
         Idem pour les références à GraphQL (`db_graphql`, `db_introspect`,\n\
         schéma SDL généré, queries Hasura-style) : ils n'existent plus.\n",
        slug = app.slug,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Application;
    use std::collections::BTreeMap;

    fn make_app(slug: &str, name: &str, has_db: bool) -> Application {
        let mut app =
            Application::new(slug.to_string(), name.to_string(), "axum-vite".to_string());
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
        let tmp = std::env::temp_dir().join("atelier-apps-context-test-1");
        let _ = fs::remove_dir_all(&tmp);
        let ctx = test_generator(&tmp);
        let trader = make_app("trader", "Trader", true);
        let wallet = make_app("wallet", "Wallet", false);
        let all = vec![trader.clone(), wallet.clone()];

        // ⚠️ On passe par `generate_for_app_at` avec un src_dir SOUS TMP, jamais par
        // `generate_for_app` : ce dernier résout `app.src_dir()` via
        // ATELIER_APPS_RUNTIME_ROOT (défaut /var/lib/atelier/apps) — sur Medion, un
        // `cargo test` écrirait alors dans le VRAI workspace trader (constaté le
        // 2026-06-11 : settings.json/.mcp.json de prod écrasés par la config fixture).
        let src_dir = tmp.join("trader/src");
        fs::create_dir_all(&src_dir).unwrap();

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

        ctx.generate_for_app_at(
            &trader,
            &src_dir,
            &all,
            Some(vec!["users".into(), "trades".into()]),
            true,
        )
        .unwrap();

        // Les écritures sont bien confinées sous tmp.
        assert!(src_dir.join(".claude/settings.json").exists());
        assert!(src_dir.join(".mcp.json").exists());
        assert!(src_dir.join(".claude/rules/docs.md").exists());

        // Cleanup legacy parent-level : tout a disparu.
        assert!(!parent_dir.join("CLAUDE.md").exists(),
                "trader/CLAUDE.md parent-level doit être supprimé");
        assert!(!parent_dir.join(".mcp.json").exists(),
                "trader/.mcp.json parent-level doit être supprimé");
        assert!(!parent_dir.join(".claude").exists(),
                "trader/.claude/ parent-level doit être supprimé intégralement");

        // Les renderers produisent le bon contenu (vérif directe).
        // settings.json ne porte QUE l'activation — la déclaration du serveur vit dans .mcp.json.
        let settings = render_settings_json();
        let parsed: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(
            parsed["enabledMcpjsonServers"],
            serde_json::json!(["studio"]),
        );
        assert!(parsed.get("mcpServers").is_none(),
                "settings.json ne doit plus recopier la définition du serveur (source de vérité = .mcp.json)");
        // INVARIANT runner : aucune allow rule dans le settings.json projet (une allow
        // court-circuiterait le canUseTool du runner agent — exec root même en plan).
        assert!(parsed.get("permissions").is_none(),
                "settings.json projet ne doit plus porter de bloc permissions");

        // La définition du serveur (url + Bearer) est dans .mcp.json — source de vérité unique.
        let mcp = render_mcp_json_with_auth(
            "http://127.0.0.1:4001/mcp?project=trader",
            Some("tok"),
        );
        let mcp_parsed: serde_json::Value = serde_json::from_str(&mcp).unwrap();
        assert_eq!(
            mcp_parsed["mcpServers"]["studio"]["url"].as_str().unwrap(),
            "http://127.0.0.1:4001/mcp?project=trader"
        );
        assert_eq!(
            mcp_parsed["mcpServers"]["studio"]["headers"]["Authorization"].as_str().unwrap(),
            "Bearer tok"
        );

        // app-info.md contient l'identité + autres apps + DB tables.
        let app_info = render_app_info_md(&trader, &all, &Some(vec!["users".into(), "trades".into()]));
        assert!(app_info.contains("`trader`"));
        assert!(app_info.contains("axum-vite"));
        assert!(app_info.contains("`users`"));
        assert!(app_info.contains("`trades`"));
        assert!(app_info.contains("Wallet"));
        assert!(app_info.contains("`API_KEY`"));

        // Skeleton CLAUDE.md minimal, n'inclut PAS les infos dynamiques.
        let initial_md = render_initial_claude_md(&trader);
        assert!(initial_md.contains("# Trader — Carnet de bord"));
        assert!(!initial_md.contains("axum-vite"),
                "skeleton CLAUDE.md ne doit pas dupliquer app-info.md");

        // Skill 0-build + script.
        let skill_content = render_app_build_skill(&trader);
        assert!(skill_content.contains("name: 0-build"));
        assert!(skill_content.contains("allowed-tools: Bash(bash .claude/skills/0-build/build.sh"));
        assert!(skill_content.contains("bash .claude/skills/0-build/build.sh"));
        let script = render_app_build_script(&trader);
        assert!(script.contains("/api/apps/trader/build"));
        assert!(script.starts_with("#!/usr/bin/env bash"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn generate_for_app_skips_when_src_missing() {
        let tmp = std::env::temp_dir().join("atelier-apps-context-test-no-src");
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
        let tmp = std::env::temp_dir().join("atelier-apps-context-test-2");
        let _ = fs::remove_dir_all(&tmp);
        let ctx = test_generator(&tmp);
        let trader = make_app("trader", "Trader", true);
        let wallet = make_app("wallet", "Wallet", false);

        ctx.generate_root(&[trader, wallet]).unwrap();

        let root = fs::read_to_string(tmp.join("CLAUDE.md")).unwrap();
        assert!(root.contains("Atelier Apps Workspace"));
        assert!(root.contains("| Trader | `trader`"));
        assert!(root.contains("| Wallet | `wallet`"));
        assert!(tmp.join(".claude/settings.json").exists());
        assert!(tmp.join(".mcp.json").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_if_changed_skips_when_identical() {
        let tmp = std::env::temp_dir().join("atelier-apps-context-test-3");
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
        let tmp = std::env::temp_dir().join("atelier-apps-context-write-if-missing");
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
