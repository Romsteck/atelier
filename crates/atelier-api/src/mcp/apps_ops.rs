//! IPC handlers for `App*` variants (atelier-apps integration).
//!
//! Split out of `ipc_handler.rs` to keep that file manageable.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::dto::{
    AppDbRelation, AppDbTableColumn, AppDbTableSchema,
    AppDbTablesData, AppExecResult, AppListData, AppLogEntry, AppLogsData,
    AppStatusData, ApplicationDto,
};
use super::scaffold;

use atelier_apps::types::{AppState, Application, DbBackend, Visibility, valid_slug};
use atelier_apps::{AppSupervisor, ContextGenerator, ProcessStatus};
use atelier_common::events::AppBuildEvent;
use atelier_dataverse::DataverseManager;
use tokio::sync::broadcast;

fn detect_level(msg: &str) -> &'static str {
    let m = msg.to_ascii_lowercase();
    if m.contains("error") || m.contains("panic") || m.contains("fatal") {
        "error"
    } else if m.contains("warn") {
        "warn"
    } else if m.contains("debug") {
        "debug"
    } else {
        "info"
    }
}
use atelier_ipc::EdgeClient;
use atelier_ipc::types::IpcResponse;
use tracing::{error, info, warn};

/// Base URL of the Atelier API as seen *from the build host*. Used to bind
/// the `origin` remote of each app's working tree to the atelier-git Smart-HTTP
/// endpoint at scaffold time. Override via `ATELIER_GIT_API_BASE`.
pub const GIT_API_BASE: &str = "http://127.0.0.1:4100";
/// Cap stdout/stderr capture per pipeline stage to ~1 MB.
const OUTPUT_CAP_BYTES: usize = 1024 * 1024;

/// Build-host configuration read from env. `None` = build locally in-process
/// (the default on Medion since the 2026-05-27 rapatriement). `Some((user@host,
/// key_path))` = SSH to that host for every build step. Replaces the
/// per-app `sources_on` flag of the pre-rapatriement era.
pub fn build_host_config() -> Option<(String, String)> {
    let host = std::env::var("ATELIER_BUILD_HOST").ok().unwrap_or_default();
    if host.is_empty() {
        return None;
    }
    let key = std::env::var("ATELIER_BUILD_SSH_KEY").ok().unwrap_or_default();
    Some((host, key))
}

/// Wrap a shell command for local execution. The build user is resolved by
/// [`scaffold::build_as_user`]: `ATELIER_BUILD_AS_USER` if set, else `hr-studio`
/// when Atelier runs as root, else `None` (dev). When a user is resolved the
/// command is spawned via `sudo -H -u <user> -- bash -lc …` so the build
/// inherits that user's PATH (cargo lives under `~/.cargo/bin/`, absent from
/// root's PATH) and produces artefacts owned by `<user>` — never root. `umask
/// 002` is forced so artefacts get group-write, which keeps the `hr-studio`
/// Studio agent (a co-member of the build group) able to edit/clean them.
///
/// WHY the root→hr-studio default (not a plain root `bash -lc`): a build that
/// falls back to root leaves root-owned `node_modules`/`target`/`dist` that the
/// hr-studio agent can neither overwrite nor delete → EACCES on the next build.
/// Only a genuinely non-root dev environment gets the plain `bash -lc`.
fn wrap_local_cmd(cmd: &str) -> (&'static str, Vec<String>) {
    let wrapped = format!("umask 002 && {cmd}");
    match scaffold::build_as_user() {
        Some(user) => (
            "sudo",
            vec![
                "-H".into(),
                "-u".into(),
                user,
                "--".into(),
                "bash".into(),
                "-lc".into(),
                wrapped,
            ],
        ),
        None => ("bash", vec!["-lc".into(), wrapped]),
    }
}

/// Context for App* handlers.
#[derive(Clone)]
pub struct AppsContext {
    pub supervisor: AppSupervisor,
    /// New Postgres+GraphQL backend, populated when
    /// `HR_DATAVERSE_ADMIN_URL` is set at boot. None means apps flagged
    /// `db_backend: postgres-dataverse` will get an explicit error.
    pub dataverse_manager: Option<Arc<DataverseManager>>,
    pub context_generator: Arc<ContextGenerator>,
    /// `None` when Atelier cannot reach the hr-edge IPC socket
    /// (câblage edge à reprendre : le socket hr-edge est désormais local).
    /// `set_app_route` / `remove_app_route` calls are then skipped with a warn.
    pub edge: Option<Arc<EdgeClient>>,
    pub git: Arc<atelier_git::GitService>,
    pub base_domain: String,
    /// Per-slug locks to serialise concurrent `build()` invocations.
    pub build_locks:
        Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Broadcast channel for build progress events.
    pub app_build_tx: broadcast::Sender<AppBuildEvent>,
    /// Remontées plateforme (`atelier_meta.platform_issues`) — purgées au delete
    /// d'app pour ne pas laisser de remontées orphelines dans le triage dev.
    pub issues: atelier_common::issue_store::PlatformIssueStore,
    /// Notifications plateforme (`atelier_meta.platform_notifications`) —
    /// purgées au delete d'app, même raison que `issues`.
    pub notifications: atelier_common::notification_store::NotificationStore,
    /// Réglages par conversation agent (`atelier_meta.agent_conversation_meta`) —
    /// purgés au delete d'app (les sessions SDK de l'app disparaissent avec elle).
    pub conversation_meta: atelier_common::conversation_meta::ConversationMetaStore,
    /// Token Claude des apps opt-in — lu FRAIS à chaque render `.env` par
    /// `platform_env` pour injecter `CLAUDE_CODE_OAUTH_TOKEN` quand `claude_access`.
    pub app_claude_auth: atelier_common::app_claude_auth::AppClaudeAuthStore,
}

impl AppsContext {
    /// Build an `AppsContext` wired to the SHARED build-event channel
    /// (`state.events.app_build`), the one the WebSocket relay subscribes to
    /// (`routes/ws.rs`). WHY: the HTTP `ship` route and the MCP entrypoint each
    /// used to construct an `AppsContext` with a THROWAWAY `broadcast::channel`,
    /// so every emitted `AppBuildEvent` went to a sender with no subscriber and
    /// the Studio's BuildBadge never lit up. Always use this constructor so the
    /// three call sites can't drift back into that bug.
    pub fn from_api_state(state: &crate::state::ApiState) -> Self {
        Self {
            supervisor: (*state.supervisor).clone(),
            dataverse_manager: state.dv.clone(),
            context_generator: state.context_generator.clone(),
            // Câblage edge non repris ici (cf. McpState::from_api_state).
            edge: None,
            git: state.git.clone(),
            base_domain: state.context_generator.base_domain.clone(),
            build_locks: state.build_locks.clone(),
            app_build_tx: state.events.app_build.clone(),
            issues: state.issues.clone(),
            notifications: state.notifications.clone(),
            conversation_meta: state.conversation_meta.clone(),
            app_claude_auth: state.app_claude_auth.clone(),
        }
    }

    /// Resolve the postgres-dataverse engine for `slug`. Maps any
    /// configuration / connectivity error into a ready-to-return
    /// [`IpcResponse`] so call sites stay compact.
    pub(crate) async fn dv_engine_for(
        &self,
        slug: &str,
    ) -> std::result::Result<Arc<atelier_dataverse::DataverseEngine>, IpcResponse> {
        let mgr = self.dataverse_manager.as_ref().ok_or_else(|| {
            IpcResponse::err(
                "postgres-dataverse backend is not configured on this orchestrator \
                 (set HR_DATAVERSE_ADMIN_URL and restart)",
            )
        })?;
        mgr.engine_for(slug)
            .await
            .map_err(|e| IpcResponse::err(format!("dataverse engine: {e}")))
    }

    /// Best-effort list of an app's postgres-dataverse tables for context
    /// generation. Returns `None` (never an error) when the app has no DB,
    /// the dataverse manager is unconfigured, the app isn't provisioned, or
    /// the query fails — context generation stays non-fatal.
    async fn dv_list_tables_opt(&self, slug: &str, has_db: bool) -> Option<Vec<String>> {
        if !has_db {
            return None;
        }
        let mgr = self.dataverse_manager.as_ref()?;
        let engine = mgr.engine_for(slug).await.ok()?;
        engine.list_tables().await.ok()
    }

    /// Count rows of a single postgres-dataverse table. Mirrors the shape the
    /// MCP `db_count_rows` tool expects (a `count` column).
    pub async fn db_count_rows(&self, slug: String, table: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        match engine.count_rows(&table).await {
            Ok(count) => IpcResponse::ok_data(serde_json::json!({
                "columns": ["count"],
                "rows": [{ "count": count }],
                "total": 1,
            })),
            Err(e) => IpcResponse::err(format!("count_rows: {e}")),
        }
    }

    // Env management (platform-tier injection of `HR_DV_*` / `ATELIER_*`,
    // user-tier CRUD, `.env` rendering, boot reconcile) lives in
    // [`super::env_ops`] — `reconcile_app_env` / `reconcile_all_env` /
    // `env_set_var` / `env_view`. The old `sync_dv_env` (HR_DV_* upsert) and
    // its dead boot-sweep `sync_dv_env_all` were folded into that single
    // reconciler (2026-06-16).
}

impl AppsContext {
    pub async fn list(&self) -> IpcResponse {
        let apps = self.supervisor.registry.list().await;
        info!(count = apps.len(), "AppList");
        let dtos: Vec<ApplicationDto> = apps.iter().map(app_to_dto).collect();
        IpcResponse::ok_data(AppListData { apps: dtos })
    }

    pub async fn get(&self, slug: &str) -> IpcResponse {
        if !valid_slug(slug) {
            return IpcResponse::err("invalid slug");
        }
        match self.supervisor.registry.get(slug).await {
            Some(app) => {
                info!(slug = %slug, "AppGet");
                IpcResponse::ok_data(app_to_dto(&app))
            }
            None => IpcResponse::err(format!("app not found: {slug}")),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(skip(self), fields(slug = %slug))]
    pub async fn create(
        &self,
        slug: String,
        name: String,
        stack: String,
        has_db: bool,
        visibility: String,
        run_command: Option<String>,
        build_command: Option<String>,
        health_path: Option<String>,
        build_artefact: Option<String>,
    ) -> IpcResponse {
        let start = Instant::now();
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        if self.supervisor.registry.get(&slug).await.is_some() {
            return IpcResponse::err(format!("app already exists: {slug}"));
        }

        // Stack = label libre, purement informatif (la plateforme est
        // stack-agnostique). Borné pour l'affichage en liste.
        let stack = stack.trim().to_string();
        if stack.len() > 64 {
            return IpcResponse::err("stack label too long (max 64 chars)");
        }
        let visibility_enum = match parse_visibility(&visibility) {
            Some(v) => v,
            None => return IpcResponse::err(format!("invalid visibility: {visibility}")),
        };

        // Assign port BEFORE creating the Application so it is persisted.
        let port = match self.supervisor.port_registry.assign(&slug).await {
            Ok(p) => p,
            Err(e) => {
                error!(slug = %slug, error = %e, "AppCreate: port assignment failed");
                return IpcResponse::err(format!("port assignment failed: {e}"));
            }
        };

        let mut app = Application::new(slug.clone(), name, stack);
        app.has_db = has_db;
        app.visibility = visibility_enum;
        app.port = port;
        app.domain = format!("{}.{}", slug, self.base_domain);
        if let Some(cmd) = run_command {
            app.run_command = cmd;
        }
        app.build_command = build_command;
        app.build_artefact = build_artefact;
        if let Some(hp) = health_path {
            app.health_path = hp;
        }
        info!(slug = %slug, "AppCreate: scaffolding new app");

        let app_dir = app.app_dir();
        if let Err(e) = tokio::fs::create_dir_all(&app_dir).await {
            error!(slug = %slug, error = %e, "AppCreate: create app_dir failed");
            self.supervisor.port_registry.release(&slug).await.ok();
            return IpcResponse::err(format!("create app dir failed: {e}"));
        }
        if let Err(e) = tokio::fs::create_dir_all(&app.src_dir()).await {
            warn!(slug = %slug, error = %e, "AppCreate: create src_dir failed");
        }
        // First perms pass right after the tree creation: the git remote bind
        // below runs as the build user and needs a writable `src/` (a second
        // pass at the end of create covers the context files + `.env`).
        scaffold::normalize_app_tree_perms(&app_dir).await;

        // Pas de scaffold ni de defaults run/build : l'app naît vide et non
        // configurée. C'est la première conversation Studio qui génère le
        // projet et pose run_command/build_command/health_path via app.update
        // (la plateforme ne connaît aucune stack, elle publie un contrat :
        // $PORT, /apps/{slug}/, .env, 0-build/ship).

        // Persist app.
        if let Err(e) = self.supervisor.registry.upsert(app.clone()).await {
            self.supervisor.port_registry.release(&slug).await.ok();
            error!(slug = %slug, error = %e, "AppCreate: registry upsert failed");
            return IpcResponse::err(format!("registry upsert failed: {e}"));
        }

        // Provision the dataverse database (base app_{slug} + rôle + entrée
        // secrets) BEFORE the initial env render so `HR_DV_*` lands in the very
        // first `.env`. WHY: create never provisioned — has_db apps were born
        // without gateway credentials (reconcile warned "HR_DV_* omitted")
        // until a manual provision; `delete()` already mirrors this via
        // `drop_app()`. Best-effort like the other create steps.
        if app.has_db {
            match &self.dataverse_manager {
                Some(mgr) => match mgr.provision(&slug).await {
                    Ok(_) => info!(slug = %slug, "AppCreate: dataverse database provisioned"),
                    Err(e) => {
                        warn!(slug = %slug, error = %e, "AppCreate: dataverse provision failed — HR_DV_* will be absent")
                    }
                },
                None => {
                    warn!(slug = %slug, "AppCreate: dataverse manager unavailable — db not provisioned")
                }
            }
        }

        // atelier-git bare repo (best-effort).
        if let Err(e) = self.git.create_repo(&slug).await {
            warn!(slug = %slug, error = %e, "AppCreate: git create_repo failed (non-fatal)");
        }
        // Bind `origin` of the app's working tree to the Atelier Smart-HTTP
        // endpoint so the Studio agent can `git push` out of the box.
        if let Err(e) = bind_git_remote_for_slug(&slug).await {
            warn!(slug = %slug, error = %e, "AppCreate: git remote bind failed (non-fatal)");
        }

        // hr-edge route (best-effort).
        let auth_required = matches!(app.visibility, Visibility::Private);
        if let Some(edge) = &self.edge {
            if let Err(e) = edge
                .set_app_route(
                    app.domain.clone(),
                    slug.clone(),
                    "local".to_string(),
                    "127.0.0.1".to_string(),
                    port,
                    auth_required,
                    false,
                )
                .await
            {
                warn!(slug = %slug, domain = %app.domain, error = %e, "AppCreate: edge set_app_route failed (non-fatal)");
            }
        } else {
            warn!(slug = %slug, domain = %app.domain, "AppCreate: edge client unavailable, route not propagated");
        }

        // Regen context (CLAUDE.md, .mcp.json, .claude/) — always local now.
        let all = self.supervisor.registry.list().await;
        let db_tables = self.dv_list_tables_opt(&slug, app.has_db).await;
        if let Err(e) = self
            .context_generator
            .generate_for_app(&app, &all, db_tables)
        {
            warn!(slug = %slug, error = %e, "AppCreate: context generation failed (non-fatal)");
        }
        if let Err(e) = self.context_generator.generate_root(&all) {
            warn!(error = %e, "AppCreate: root context generation failed (non-fatal)");
        }

        // Render the initial `.env` (PORT + dataverse gateway contract + logging
        // contract). WHY: the old `sync_dv_env` was never called on create, so
        // new apps had NO `HR_DV_*` until a manual token rotation — now the
        // gateway credentials land on disk at creation time.
        if let Err(e) = self.reconcile_app_env(&slug, false).await {
            warn!(slug = %slug, error = %e, "AppCreate: env reconcile failed (non-fatal)");
        }

        // Normalize ownership/perms of the whole freshly-created tree LAST so
        // it also covers the context files and `.env` written above. WHY: the
        // service runs as root — without this the scaffolded tree is
        // root-owned 0755 and the app is stillborn: the Studio agent's
        // workspace is read-only, and the git remote bind + first build die
        // on Permission denied.
        scaffold::normalize_app_tree_perms(&app_dir).await;

        info!(slug = %slug, port, duration_ms = start.elapsed().as_millis() as u64, "AppCreate ok");
        IpcResponse::ok_data(app_to_dto(&app))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        slug: String,
        name: Option<String>,
        stack: Option<String>,
        visibility: Option<String>,
        run_command: Option<String>,
        build_command: Option<String>,
        health_path: Option<String>,
        env_vars: Option<BTreeMap<String, String>>,
        has_db: Option<bool>,
        build_artefact: Option<String>,
        claude_access: Option<bool>,
    ) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        // Guarded read-modify-write; released before reconcile_app_env below,
        // which re-acquires the same per-slug guard.
        let guard = self.supervisor.registry.lock_slug(&slug).await;
        let mut app = match self.supervisor.registry.get(&slug).await {
            Some(a) => a,
            None => return IpcResponse::err(format!("app not found: {slug}")),
        };

        if let Some(n) = name {
            app.name = n;
        }
        if let Some(s) = stack {
            let s = s.trim().to_string();
            if s.len() > 64 {
                return IpcResponse::err("stack label too long (max 64 chars)");
            }
            app.stack = s;
        }
        if let Some(v) = visibility {
            match parse_visibility(&v) {
                Some(vv) => app.visibility = vv,
                None => return IpcResponse::err(format!("invalid visibility: {v}")),
            }
        }
        if let Some(rc) = run_command {
            app.run_command = rc;
        }
        if build_command.is_some() {
            app.build_command = build_command;
        }
        if build_artefact.is_some() {
            app.build_artefact = build_artefact;
        }
        if let Some(hp) = health_path {
            app.health_path = hp;
        }
        if let Some(ev) = env_vars {
            // Converge the MCP `app.update` env path onto the structured model:
            // each entry becomes a USER var (secret by name heuristic, runtime
            // scope), MERGED with existing user vars (not a full replace). The
            // `.env` is re-rendered by the reconcile at the end of this fn.
            for (k, v) in ev {
                if super::env_ops::is_platform_key(&k)
                    || super::env_ops::is_forbidden_user_var(&k, &v)
                    || !atelier_apps::valid_env_key(&k)
                {
                    continue;
                }
                let secret = super::env_ops::looks_secret(&k);
                app.env_set(atelier_apps::types::EnvVar {
                    key: k,
                    value: v,
                    secret,
                    scope: atelier_apps::types::EnvScope::Runtime,
                });
            }
            app.env_vars.clear();
        }
        if let Some(new_has_db) = has_db {
            if new_has_db && !app.has_db {
                info!(slug = %slug, "has_db enabled");
            } else if !new_has_db && app.has_db {
                info!(slug = %slug, "has_db disabled");
            }
            app.has_db = new_has_db;
        }
        if let Some(new_ca) = claude_access {
            if new_ca != app.claude_access {
                info!(slug = %slug, claude_access = new_ca, "claude_access changed");
            }
            app.claude_access = new_ca;
        }

        if let Err(e) = self.supervisor.registry.upsert(app.clone()).await {
            error!(slug = %slug, error = %e, "AppUpdate: registry upsert failed");
            return IpcResponse::err(format!("registry upsert failed: {e}"));
        }
        drop(guard);

        // Push updated edge route if visibility changed
        let auth_required = matches!(app.visibility, Visibility::Private);
        if let Some(edge) = &self.edge {
            if let Err(e) = edge
                .set_app_route(
                    app.domain.clone(),
                    slug.clone(),
                    "local".to_string(),
                    "127.0.0.1".to_string(),
                    app.port,
                    auth_required,
                    false,
                )
                .await
            {
                warn!(slug = %slug, error = %e, "AppUpdate: edge set_app_route failed (non-fatal)");
            }
        } else {
            warn!(slug = %slug, "AppUpdate: edge client unavailable, route not propagated");
        }

        // Regenerate context
        let all = self.supervisor.registry.list().await;
        let db_tables = self.dv_list_tables_opt(&slug, app.has_db).await;
        if let Err(e) = self
            .context_generator
            .generate_for_app(&app, &all, db_tables)
        {
            warn!(slug = %slug, error = %e, "AppUpdate: context regeneration failed");
        }

        // Re-render the `.env` (platform + user) after any env / has_db change.
        if let Err(e) = self.reconcile_app_env(&slug, false).await {
            warn!(slug = %slug, error = %e, "AppUpdate: env reconcile failed");
        }

        info!(slug = %slug, "AppUpdate ok");
        IpcResponse::ok_data(app_to_dto(&app))
    }

    #[tracing::instrument(skip(self), fields(slug = %slug, keep_data))]
    pub async fn delete(&self, slug: String, keep_data: bool) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let app = match self.supervisor.registry.get(&slug).await {
            Some(a) => a,
            None => return IpcResponse::err(format!("app not found: {slug}")),
        };

        // 1. Stop process
        if let Err(e) = self.supervisor.stop(&slug).await {
            warn!(slug = %slug, error = %e, "AppDelete: stop failed (continuing)");
        }
        // 2. Remove edge route
        if let Some(edge) = &self.edge {
            if let Err(e) = edge.remove_app_route(&app.domain).await {
                warn!(slug = %slug, domain = %app.domain, error = %e, "AppDelete: edge remove_app_route failed");
            }
        } else {
            warn!(slug = %slug, domain = %app.domain, "AppDelete: edge client unavailable, route not cleaned");
        }
        // 3. Remove from registry + release port, under the per-slug guard so a
        // concurrent env/state mutation can't re-upsert the row after removal.
        {
            let _guard = self.supervisor.registry.lock_slug(&slug).await;
            if let Err(e) = self.supervisor.registry.remove(&slug).await {
                error!(slug = %slug, error = %e, "AppDelete: registry remove failed");
                return IpcResponse::err(format!("registry remove failed: {e}"));
            }
            // 4. Release port
            if let Err(e) = self.supervisor.port_registry.release(&slug).await {
                warn!(slug = %slug, error = %e, "AppDelete: port release failed");
            }
        }
        // 5. Purge des remontées plateforme de l'app (best-effort). WHY même quand
        // keep_data : ce sont des frictions PLATEFORME, pas de la donnée d'app ;
        // une app supprimée n'a plus de chat qui les contextualise.
        self.issues.delete_by_slug(&slug).await;
        self.notifications.delete_by_slug(&slug).await;
        // Idem pour les réglages de conversations agent : les sessions SDK de
        // l'app disparaissent avec son workspace, leur meta n'a plus de sens.
        self.conversation_meta.delete_by_slug(&slug).await;
        if !keep_data {
            // De-provision the dataverse database (base app_{slug} + rôle +
            // entrée secrets + pool évincé). Sans ça, chaque delete laissait
            // une base/rôle orphelins, et recréer le même slug adoptait
            // l'ancien schéma. Best-effort loggé, comme le reste du delete.
            if app.has_db {
                match &self.dataverse_manager {
                    Some(mgr) => {
                        if let Err(e) = mgr.drop_app(&slug).await {
                            warn!(slug = %slug, error = %e, "AppDelete: dataverse drop_app failed — orphan db likely");
                        } else {
                            info!(slug = %slug, "AppDelete: dataverse database dropped");
                        }
                    }
                    None => {
                        warn!(slug = %slug, "AppDelete: dataverse manager unavailable — db not dropped");
                    }
                }
            }
            let dir = app.app_dir();
            if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                warn!(slug = %slug, dir = %dir.display(), error = %e, "AppDelete: rm -rf failed");
            }
            // L'historique git est de la donnée → suit keep_data. Sans ça, chaque
            // suppression d'app laissait un dépôt bare (et son entrée mirror)
            // orphelin dans /var/lib/atelier/git. Suppression LOCALE — ne touche
            // pas au miroir GitHub distant.
            let mut cfg = self.git.load_config().await.unwrap_or_default();
            if cfg.mirrors.remove(&slug).is_some() {
                if let Err(e) = self.git.save_config(&cfg).await {
                    warn!(slug = %slug, error = %e, "AppDelete: mirror config cleanup failed");
                }
            }
            if let Err(e) = self.git.delete_repo(&slug).await {
                warn!(slug = %slug, error = %e, "AppDelete: git repo delete failed");
            }
        } else {
            info!(slug = %slug, "AppDelete: keep_data=true, sources préservées");
        }

        // Regenerate root context
        let all = self.supervisor.registry.list().await;
        if let Err(e) = self.context_generator.generate_root(&all) {
            warn!(error = %e, "AppDelete: root context regeneration failed");
        }

        info!(slug = %slug, keep_data, "AppDelete ok");
        IpcResponse::ok_data(serde_json::json!({ "ok": true }))
    }

    pub async fn control(&self, slug: String, action: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let start = Instant::now();
        let res = match action.as_str() {
            "start" => self.supervisor.start(&slug).await,
            "stop" => self.supervisor.stop(&slug).await,
            "restart" => self.supervisor.restart(&slug).await,
            other => return IpcResponse::err(format!("invalid action: {other}")),
        };

        match res {
            Ok(()) => {
                info!(
                    slug = %slug,
                    action = %action,
                    duration_ms = start.elapsed().as_millis() as u64,
                    "AppControl ok"
                );
                IpcResponse::ok_data(serde_json::json!({ "ok": true }))
            }
            Err(e) => {
                error!(slug = %slug, action = %action, error = %e, "AppControl failed");
                IpcResponse::err(format!("{action} failed: {e}"))
            }
        }
    }

    pub async fn status(&self, slug: &str) -> IpcResponse {
        if !valid_slug(slug) {
            return IpcResponse::err("invalid slug");
        }
        match self.supervisor.status(slug).await {
            Some(s) => IpcResponse::ok_data(process_status_to_dto(slug, &s)),
            None => {
                // Return a Stopped placeholder so callers don't 404 on never-started apps.
                let port = self.supervisor.port_registry.get(slug).await.unwrap_or(0);
                IpcResponse::ok_data(AppStatusData {
                    slug: slug.to_string(),
                    pid: None,
                    state: "stopped".to_string(),
                    port,
                    uptime_secs: 0,
                    restart_count: 0,
                    exe_path: None,
                    exe_mtime: None,
                })
            }
        }
    }

    pub async fn logs(
        &self,
        slug: String,
        limit: Option<usize>,
        level: Option<String>,
    ) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let n = limit.unwrap_or(200).min(5000);
        // Nom d'unité dérivé par le supervisor (source unique — un préfixe hardcodé
        // ici avait causé le symptôme "logs figés au 9 mai" : journalctl lisait une
        // unité morte après le renommage hr-app → atelier-app).
        let unit = atelier_apps::supervisor::unit_name(&slug);
        let output = tokio::process::Command::new("journalctl")
            .args([
                "-u",
                &unit,
                "-n",
                &n.to_string(),
                "--no-pager",
                "--output=short-iso",
            ])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                let level_filter = level.as_deref();
                let mut logs: Vec<AppLogEntry> = Vec::new();
                for line in text.lines() {
                    if line.starts_with("--") || line.is_empty() {
                        continue;
                    }
                    // Format: "2024-01-02T10:11:12+0000 host unit[pid]: message"
                    let (timestamp, rest) = match line.split_once(' ') {
                        Some(p) => p,
                        None => continue,
                    };
                    let msg = match rest.find("]: ") {
                        Some(i) => rest[i + 3..].to_string(),
                        None => match rest.find(": ") {
                            Some(i) => rest[i + 2..].to_string(),
                            None => rest.to_string(),
                        },
                    };
                    let lvl = detect_level(&msg);
                    if let Some(f) = level_filter {
                        if !lvl.eq_ignore_ascii_case(f) {
                            continue;
                        }
                    }
                    logs.push(AppLogEntry {
                        timestamp: timestamp.to_string(),
                        level: lvl.to_string(),
                        message: msg,
                        data: None,
                    });
                }
                info!(slug = %slug, count = logs.len(), "AppLogs queried (journald)");
                IpcResponse::ok_data(AppLogsData { slug, logs })
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                warn!(slug = %slug, status = %out.status, stderr = %stderr, "journalctl failed");
                IpcResponse::ok_data(AppLogsData { slug, logs: vec![] })
            }
            Err(e) => {
                error!(slug = %slug, error = %e, "journalctl spawn failed");
                IpcResponse::err(format!("log query failed: {e}"))
            }
        }
    }

    pub async fn exec(
        &self,
        slug: String,
        command: String,
        timeout_secs: Option<u64>,
    ) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let app = match self.supervisor.registry.get(&slug).await {
            Some(a) => a,
            None => return IpcResponse::err(format!("app not found: {slug}")),
        };
        let cwd = app.src_dir();
        let timeout = std::time::Duration::from_secs(timeout_secs.unwrap_or(60).max(1));
        let start = Instant::now();

        let child = tokio::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();

        let child = match child {
            Ok(c) => c,
            Err(e) => {
                error!(slug = %slug, error = %e, "AppExec spawn failed");
                return IpcResponse::err(format!("spawn: {e}"));
            }
        };

        let wait_res = tokio::time::timeout(timeout, child.wait_with_output()).await;
        let out = match wait_res {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return IpcResponse::err(format!("wait: {e}"));
            }
            Err(_) => {
                return IpcResponse::err(format!("timeout after {}s", timeout.as_secs()));
            }
        };

        let result = AppExecResult {
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            exit_code: out.status.code().unwrap_or(-1),
            duration_ms: start.elapsed().as_millis() as u64,
        };
        info!(
            slug = %slug,
            exit_code = result.exit_code,
            duration_ms = result.duration_ms,
            "AppExec ok"
        );
        IpcResponse::ok_data(result)
    }

    pub async fn regenerate_context(&self, slug: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let app = match self.supervisor.registry.get(&slug).await {
            Some(a) => a,
            None => return IpcResponse::err(format!("app not found: {slug}")),
        };
        let all = self.supervisor.registry.list().await;
        let db_tables = self.dv_list_tables_opt(&slug, app.has_db).await;

        if let Err(e) = self
            .context_generator
            .generate_for_app(&app, &all, db_tables)
        {
            error!(slug = %slug, error = %e, "AppRegenerateContext failed");
            return IpcResponse::err(format!("generate_for_app: {e}"));
        }

        if let Err(e) = self.context_generator.generate_root(&all) {
            warn!(error = %e, "AppRegenerateContext root failed");
        }
        info!(slug = %slug, "AppRegenerateContext ok");
        IpcResponse::ok_data(serde_json::json!({ "ok": true }))
    }

    /// Régénère le contexte de TOUTES les apps + le root (une seule fois, pas
    /// N× comme quand on boucle sur `regenerate_context`). `db_tables`
    /// best-effort par app ; un échec par app est warn-only, la passe continue.
    /// Appelée au boot (le contexte généré suit le BINAIRE : un deploy qui
    /// change les renderers doit se propager sans attendre un AppUpdate) et par
    /// le tool MCP `studio.refresh_all`. Idempotent (`write_if_changed`).
    pub async fn regenerate_all_contexts(&self) -> (u32, usize) {
        let all = self.supervisor.registry.list().await;
        let mut ok = 0u32;
        for app in &all {
            let db_tables = self.dv_list_tables_opt(&app.slug, app.has_db).await;
            match self.context_generator.generate_for_app(app, &all, db_tables) {
                Ok(()) => ok += 1,
                Err(e) => warn!(slug = %app.slug, error = %e, "context regen failed"),
            }
        }
        if let Err(e) = self.context_generator.generate_root(&all) {
            warn!(error = %e, "root context regen failed");
        }
        (ok, all.len())
    }

    // ── App DB ─────────────────────────────────────────────────

    pub async fn db_list_tables(&self, slug: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        match engine.list_tables().await {
            Ok(tables) => {
                info!(slug = %slug, count = tables.len(), backend = "postgres-dataverse", "AppDbListTables ok");
                IpcResponse::ok_data(AppDbTablesData { tables })
            }
            Err(e) => IpcResponse::err(format!("list_tables: {e}")),
        }
    }

    pub async fn db_describe_table(&self, slug: String, table: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        let dv_schema = match engine.get_schema().await {
            Ok(s) => s,
            Err(e) => return IpcResponse::err(format!("get_schema: {e}")),
        };
        let Some(t) = dv_schema.tables.iter().find(|t| t.name == table) else {
            return IpcResponse::err(format!("table '{}' not found", table));
        };
        let row_count = engine.count_rows(&table).await.unwrap_or(0) as u64;
        let columns = t
            .columns
            .iter()
            .map(|c| AppDbTableColumn {
                name: c.name.clone(),
                field_type: format!("{:?}", c.field_type),
                required: c.required,
                unique: c.unique,
                choices: c.choices.clone(),
                formula_expression: c.formula_expression.clone(),
            })
            .collect();
        let relations = dv_schema
            .relations
            .iter()
            .filter(|r| r.from_table == table)
            .map(|r| {
                // Target's primary display column (auto-resolved), not the id.
                let display_column = dv_schema
                    .tables
                    .iter()
                    .find(|t| t.name == r.to_table)
                    .map(|target| dv_schema.effective_display_column(target))
                    .unwrap_or_else(|| "id".to_string());
                AppDbRelation {
                    from_column: r.from_column.clone(),
                    to_table: r.to_table.clone(),
                    to_column: r.to_column.clone(),
                    display_column,
                }
            })
            .collect();
        IpcResponse::ok_data(AppDbTableSchema {
            name: t.name.clone(),
            columns,
            relations,
            row_count,
        })
    }

    pub async fn db_execute(
        &self,
        slug: String,
        _sql: String,
        _params: Vec<serde_json::Value>,
    ) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        IpcResponse::err(
            "raw SQL mutations are not supported on the postgres-dataverse backend — \
             use the `dv_insert`/`dv_update`/`dv_delete` MCP tools (or the REST \
             gateway at /api/dv/{slug}/{table})",
        )
    }

    pub async fn db_sync_schema(&self, slug: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        // On the postgres-dataverse backend, `_dv_*` IS the source of truth —
        // there is nothing to sync from. Return an empty SyncResult so the
        // caller sees a successful no-op.
        IpcResponse::ok_data(serde_json::json!({
            "tables_added": [],
            "columns_added": [],
            "relations_added": 0,
        }))
    }

    pub async fn db_get_schema(&self, slug: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        match engine.get_schema().await {
            Ok(schema) => IpcResponse::ok_data(schema),
            Err(e) => IpcResponse::err(format!("get_schema: {e}")),
        }
    }

    pub async fn db_create_table(&self, slug: String, definition: serde_json::Value) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        // Fill in defaults so callers only need to supply `name` and `columns`.
        // Both backends accept the same JSON shape for TableDefinition.
        let mut def_value = definition;
        if let serde_json::Value::Object(ref mut map) = def_value {
            let now = chrono::Utc::now().to_rfc3339();
            if !map.contains_key("slug") {
                if let Some(name) = map.get("name").and_then(|v| v.as_str()) {
                    map.insert("slug".to_string(), serde_json::Value::String(name.to_string()));
                }
            }
            map.entry("created_at".to_string())
                .or_insert_with(|| serde_json::Value::String(now.clone()));
            map.entry("updated_at".to_string())
                .or_insert_with(|| serde_json::Value::String(now));
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        let def: atelier_dataverse::TableDefinition = match serde_json::from_value(def_value) {
            Ok(d) => d,
            Err(e) => return IpcResponse::err(format!("invalid table definition: {e}")),
        };
        info!(slug = %slug, table = %def.name, backend = "postgres-dataverse", "Creating table");
        match engine.create_table(&def).await {
            Ok(version) => IpcResponse::ok_data(serde_json::json!({ "version": version })),
            Err(e) => IpcResponse::err(format!("create_table: {e}")),
        }
    }

    pub async fn db_drop_table(&self, slug: String, table: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        info!(slug = %slug, table = %table, backend = "postgres-dataverse", "Dropping table");
        match engine.drop_table(&table).await {
            Ok(version) => IpcResponse::ok_data(serde_json::json!({ "version": version })),
            Err(e) => IpcResponse::err(format!("drop_table: {e}")),
        }
    }

    pub async fn db_add_column(&self, slug: String, table: String, column: serde_json::Value) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        let col: atelier_dataverse::ColumnDefinition = match serde_json::from_value(column) {
            Ok(c) => c,
            Err(e) => return IpcResponse::err(format!("invalid column definition: {e}")),
        };
        info!(slug = %slug, table = %table, column = %col.name, backend = "postgres-dataverse", "Adding column");
        match engine.add_column(&table, &col).await {
            Ok(version) => IpcResponse::ok_data(serde_json::json!({ "version": version })),
            Err(e) => IpcResponse::err(format!("add_column: {e}")),
        }
    }

    pub async fn db_remove_column(&self, slug: String, table: String, column: String) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        info!(slug = %slug, table = %table, column = %column, backend = "postgres-dataverse", "Removing column");
        match engine.remove_column(&table, &column).await {
            Ok(version) => IpcResponse::ok_data(serde_json::json!({ "version": version })),
            Err(e) => IpcResponse::err(format!("remove_column: {e}")),
        }
    }

    pub async fn db_create_relation(&self, slug: String, relation: serde_json::Value) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        let rel: atelier_dataverse::RelationDefinition = match serde_json::from_value(relation) {
            Ok(r) => r,
            Err(e) => return IpcResponse::err(format!("invalid relation definition: {e}")),
        };
        info!(slug = %slug, from = %rel.from_table, to = %rel.to_table, backend = "postgres-dataverse", "Creating relation");
        match engine.create_relation(&rel).await {
            Ok(version) => IpcResponse::ok_data(serde_json::json!({ "version": version })),
            Err(e) => IpcResponse::err(format!("create_relation: {e}")),
        }
    }

    pub async fn db_set_display_column(
        &self,
        slug: String,
        table: String,
        column: Option<String>,
    ) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let engine = match self.dv_engine_for(&slug).await {
            Ok(e) => e,
            Err(resp) => return resp,
        };
        info!(slug = %slug, table = %table, column = ?column, backend = "postgres-dataverse", "Setting display column");
        match engine.set_display_column(&table, column.as_deref()).await {
            Ok(version) => IpcResponse::ok_data(serde_json::json!({ "version": version })),
            Err(e) => IpcResponse::err(format!("set_display_column: {e}")),
        }
    }

}

impl AppsContext {
    /// Build an app remotely on the configured build host (ATELIER_BUILD_HOST).
    ///
    /// Steps :
    /// 1. SSH probe (fast-fail with actionable error if not configured).
    /// 2. `mkdir -p` the remote source dir.
    /// 3. `rsync` source up (excludes target/, node_modules/, .next/, dist/, .git/).
    /// 4. `ssh` the build command.
    /// 5. `rsync` the configured artefacts back.
    ///
    /// A per-slug lock prevents concurrent builds for the same app.
    /// The whole pipeline is bounded by `timeout_secs` (default 1800 = 30 min).
    #[tracing::instrument(skip(self), fields(slug = %slug))]
    pub async fn build(&self, slug: String, timeout_secs: Option<u64>) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let app = match self.supervisor.registry.get(&slug).await {
            Some(a) => a,
            None => return IpcResponse::err(format!("app not found: {slug}")),
        };

        let build_command = match app.build_command.clone().filter(|c| !c.trim().is_empty()) {
            Some(c) => c,
            None => {
                warn!(slug = %slug, "build: no build_command configured");
                return IpcResponse::err(
                    "no build_command configured for this app — set it via app.update \
                     (the platform is stack-agnostic; the app owns its build command)"
                        .to_string(),
                );
            }
        };
        // Artefacts : uniquement requis pour le rsync-back depuis un build
        // host distant. En build local (le défaut) ils sont déjà en place.
        let artefacts = resolve_artefacts(&app);
        if build_host_config().is_some() && artefacts.is_empty() {
            return IpcResponse::err(
                "no artefacts to rsync back from the build host (set build_artefact)",
            );
        }

        // Build-scoped user vars (VITE_*/NEXT_PUBLIC_*…) exported before the
        // build command. Empty unless the app declares `scope: build|both` vars.
        let build_env_prefix =
            super::env_ops::build_env_sh_prefix(&self.render_build_env(&app).await);

        // ── Per-slug lock ───────────────────────────────────────────
        let lock = {
            let mut map = self.build_locks.lock().await;
            map.entry(slug.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                warn!(slug = %slug, "build: already in progress");
                return IpcResponse::err(format!(
                    "BUILD_BUSY: another build for '{slug}' is already running. \
                     STOP and WAIT — do not retry automatically. \
                     Pause your work, inform the user that a concurrent build is in progress, \
                     and wait for the user to explicitly tell you to rebuild before calling app.build again."
                ));
            }
        };

        // Emit "started" event now that the lock is acquired.
        emit_build_event(
            &self.app_build_tx,
            &slug,
            "started",
            None,
            None,
            None,
            Some("build pipeline started".to_string()),
            None,
            None,
        );

        let host_cfg = build_host_config();
        // Clamp 1s..=2h — aligns with ship() and prevents a caller from
        // pinning an orchestrator task on a quasi-infinite timeout.
        let timeout = Duration::from_secs(timeout_secs.unwrap_or(1800).clamp(1, 7200));
        let started = Instant::now();
        let local_src = app.src_dir();
        // Mirror layout on the remote build host when SSH is used.
        let remote_src = local_src.display().to_string();
        let local_src_str = format!("{}/", local_src.display());

        // SSH ControlMaster : multiplex all ssh/rsync calls of this build over a
        // single TCP connection to save ~200-300ms per call. Socket lives in
        // /tmp with slug + pid to avoid collisions between concurrent builds.
        let ctl_socket = format!("/tmp/atelier-build-ssh-{}-{}.sock", slug, std::process::id());
        let ctl_path_opt = format!("ControlPath={ctl_socket}");
        let ssh_e_arg = host_cfg.as_ref().map(|(_, key)| format!(
            "ssh -i {key} -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
             -o ControlMaster=auto -o {ctl_path_opt} -o ControlPersist=30 \
             -o ServerAliveInterval=10 -o ServerAliveCountMax=3"
        ));

        match &host_cfg {
            Some((host, _)) => info!(slug = %slug, host = %host, build_command = %build_command, timeout_secs = timeout.as_secs(), "build: start (remote)"),
            None => info!(slug = %slug, build_command = %build_command, timeout_secs = timeout.as_secs(), "build: start (local)"),
        }

        let app_build_tx = self.app_build_tx.clone();
        let slug_for_pipeline = slug.clone();
        let supervisor_for_pipeline = self.supervisor.clone();
        let pipeline = async {
            let mut acc = StageAccumulator::new();
            let emit_step = |step: u32, phase: &str, dur_ms: u64, msg: Option<String>| {
                emit_build_event(
                    &app_build_tx,
                    &slug_for_pipeline,
                    "step",
                    Some(step),
                    Some(5),
                    Some(phase.to_string()),
                    msg,
                    Some(dur_ms),
                    None,
                );
            };

            if host_cfg.is_none() {
                // Build local : pas de ssh-probe ni de rsync-up nécessaire, on
                // émet les deux "skipped" steps pour préserver la timeline 5-step
                // attendue par l'UI.
                info!(slug = %slug, "build: local — skipping ssh-probe / rsync-up");
                emit_step(1, "skipped:ssh-probe (local build)", 0, Some("local build".to_string()));
                emit_step(2, "skipped:rsync-up (local build)", 0, Some("local build".to_string()));
            } else {
                let (host, key) = host_cfg.as_ref().unwrap();
                let ssh_e_arg = ssh_e_arg.as_ref().unwrap();
                // 1) SSH probe
                info!(slug = %slug, host = %host, "build: ssh probe");
                let probe = run_capture(
                    "ssh",
                    &[
                        "-i", &key,
                        "-o", "BatchMode=yes",
                        "-o", "ConnectTimeout=5",
                        "-o", "StrictHostKeyChecking=accept-new",
                        "-o", "ControlMaster=auto",
                        "-o", &ctl_path_opt,
                        "-o", "ControlPersist=30", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
                        &host,
                        "true",
                    ],
                    None,
                )
                .await;
                acc.push("ssh-probe", &probe);
                emit_step(1, "ssh-probe", probe.duration_ms, None);
                if probe.exit_code != 0 {
                    error!(slug = %slug, exit_code = probe.exit_code, stderr = %truncate(&probe.stderr, 512), "build: ssh probe failed");
                    return acc.into_result(format!(
                        "ssh probe failed (host {host}); ensure SSH key {key} can log into the build host (BatchMode)"
                    ), started);
                }

                // 2) mkdir remote
                info!(slug = %slug, remote_src = %remote_src, "build: mkdir remote");
                let mkdir = run_capture(
                    "ssh",
                    &[
                        "-i", &key,
                        "-o", "BatchMode=yes",
                        "-o", "StrictHostKeyChecking=accept-new",
                        "-o", "ControlMaster=auto",
                        "-o", &ctl_path_opt,
                        "-o", "ControlPersist=30", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
                        &host,
                        &format!("mkdir -p {}", shell_quote(&remote_src)),
                    ],
                    None,
                )
                .await;
                acc.push("mkdir", &mkdir);
                if mkdir.exit_code != 0 {
                    return acc.into_result("remote mkdir failed".into(), started);
                }

                // 3) rsync up
                info!(slug = %slug, "build: rsync up");
                let dest = format!("{}:{}/", host, remote_src);
                // LAN 10GbE: -W (whole-file) skips delta-xfer which is only useful on
                // slow networks; drop -z compression which caps throughput on CPU.
                let up = run_capture(
                    "rsync",
                    &[
                        "-a", "-W", "--delete",
                        "--exclude", "target/",
                        "--exclude", "node_modules/",
                        "--exclude", ".next/",
                        "--exclude", "dist/",
                        "--exclude", ".git/",
                        "-e", &ssh_e_arg,
                        &local_src_str,
                        &dest,
                    ],
                    None,
                )
                .await;
                acc.push("rsync-up", &up);
                emit_step(2, "rsync-up", up.duration_ms + mkdir.duration_ms, None);
                if up.exit_code != 0 {
                    return acc.into_result("rsync up failed".into(), started);
                }

            }

            // 3) compile — wrap in `bash -lc` so the user's login shell sources
            // .profile / .cargo/env (otherwise cargo/rustup aren't in PATH).
            info!(slug = %slug, "build: compile (CI=true universal)");
            // Forcer CI=true pour pnpm/npm non-interactifs (sinon
            // ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY). NPM_CONFIG_FUND=false
            // réduit le bruit.
            let inner_cmd = format!(
                "export CI=true NPM_CONFIG_FUND=false && {}cd {} && {}",
                build_env_prefix,
                shell_quote(&remote_src),
                build_command
            );
            let compile = match &host_cfg {
                Some((host, key)) => {
                    let remote_cmd = format!("bash -lc {}", shell_quote(&inner_cmd));
                    run_capture(
                        "ssh",
                        &[
                            "-i", key,
                            "-o", "BatchMode=yes",
                            "-o", "StrictHostKeyChecking=accept-new",
                            "-o", "ControlMaster=auto",
                            "-o", &ctl_path_opt,
                            "-o", "ControlPersist=30", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
                            host,
                            &remote_cmd,
                        ],
                        None,
                    )
                    .await
                }
                None => {
                    let (prog, args) = wrap_local_cmd(&inner_cmd);
                    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
                    run_capture(prog, &args_ref, None).await
                }
            };
            acc.push("compile", &compile);
            emit_step(3, "compile", compile.duration_ms, None);
            if compile.exit_code != 0 {
                error!(slug = %slug, exit_code = compile.exit_code, "build: compile failed");
                return acc.into_result("build command failed".into(), started);
            }

            // 3b) stop the supervised process before overwriting artefacts on disk.
            // Avoids serving a partially-rsynced .next/, target/release binary, etc.
            // Best-effort: if the app is not running, this is a no-op.
            if let Err(e) = supervisor_for_pipeline.stop(&slug_for_pipeline).await {
                warn!(slug = %slug_for_pipeline, error = %e, "build: pre-rsync stop failed (continuing)");
            }

            // 4) rsync each artefact back from the build host. For local builds
            // the artefacts are already in `local_src`, so we only check
            // existence and skip the rsync calls.
            let rsync_back_started = Instant::now();
            if let Some((host, key)) = &host_cfg {
                let ssh_e_arg = ssh_e_arg.as_ref().unwrap();
                for art_spec in &artefacts {
                    let (art, optional) = parse_artefact_spec(art_spec);
                    info!(slug = %slug, artefact = %art, optional, "build: rsync down");
                    let remote_path = format!("{}/{}", remote_src, art);
                    let local_path = local_src.join(art);
                    if let Some(parent) = local_path.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    // Existence check first to give a useful error.
                    let exists = run_capture(
                        "ssh",
                        &[
                            "-i", key,
                            "-o", "BatchMode=yes",
                            "-o", "StrictHostKeyChecking=accept-new",
                            "-o", "ControlMaster=auto",
                            "-o", &ctl_path_opt,
                            "-o", "ControlPersist=30", "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3",
                            host,
                            &format!("test -e {}", shell_quote(&remote_path)),
                        ],
                        None,
                    )
                    .await;
                    if exists.exit_code != 0 {
                        if optional {
                            info!(slug = %slug, artefact = %art, "build: optional artefact absent, skipping");
                            continue;
                        }
                        acc.push(&format!("check-{art}"), &exists);
                        return acc.into_result(
                            format!("artefact missing on remote: {art}"),
                            started,
                        );
                    }
                    // Detect dir vs file on remote — for dirs we use trailing slash
                    // + --delete to mirror exact contents. Without trailing slash
                    // rsync nests the source dir INSIDE an existing dst dir
                    // (observed: forge.next/.next/BUILD_ID instead of forge.next/BUILD_ID).
                    let is_dir = run_capture(
                        "ssh",
                        &[
                            "-i", key,
                            "-o", "BatchMode=yes",
                            "-o", "StrictHostKeyChecking=accept-new",
                            "-o", "ControlMaster=auto",
                            "-o", &ctl_path_opt,
                            "-o", "ControlPersist=30",
                            host,
                            &format!("test -d {}", shell_quote(&remote_path)),
                        ],
                        None,
                    )
                    .await;
                    let dir_mode = is_dir.exit_code == 0;
                    let (src_arg, dst_arg, extra_args): (String, String, &[&str]) = if dir_mode {
                        let _ = tokio::fs::create_dir_all(&local_path).await;
                        (
                            format!("{}:{}/", host, remote_path),
                            format!("{}/", local_path.display()),
                            &["--delete"],
                        )
                    } else {
                        (
                            format!("{}:{}", host, remote_path),
                            local_path.display().to_string(),
                            &[],
                        )
                    };
                    let mut rsync_args: Vec<&str> = vec!["-a", "-W"];
                    rsync_args.extend_from_slice(extra_args);
                    rsync_args.extend_from_slice(&["-e", ssh_e_arg, &src_arg, &dst_arg]);
                    let down = run_capture("rsync", &rsync_args, None).await;
                    acc.push(&format!("rsync-down:{art}"), &down);
                    if down.exit_code != 0 {
                        return acc.into_result(
                            format!("rsync down failed for {art}"),
                            started,
                        );
                    }
                }
                emit_step(
                    4,
                    "rsync-back",
                    rsync_back_started.elapsed().as_millis() as u64,
                    None,
                );
            } else {
                emit_step(4, "skipped:rsync-back (local build)", 0, Some("local build".to_string()));
            }

            // 6) restart the app so the freshly rsynced artefacts are picked up.
            // A start failure means the app is down — the build must NOT report
            // success (exit_code != 0 surfaces as success:false to callers).
            let restart_started = Instant::now();
            let start_err = supervisor_for_pipeline.start(&slug_for_pipeline).await.err();
            emit_step(
                5,
                "restart",
                restart_started.elapsed().as_millis() as u64,
                start_err.as_ref().map(|e| format!("start failed: {e}")),
            );
            if let Some(e) = start_err {
                error!(slug = %slug_for_pipeline, error = %e, "build: post-rsync start failed — app is down");
                return acc.into_result(
                    format!("artefacts rebuilt but the app failed to start: {e}"),
                    started,
                );
            }

            info!(slug = %slug_for_pipeline, duration_ms = started.elapsed().as_millis() as u64, "build: ok");
            acc.into_result_ok(started)
        };

        let resp = match tokio::time::timeout(timeout, pipeline).await {
            Ok(r) => r,
            Err(_) => {
                error!(slug = %slug, timeout_secs = timeout.as_secs(), "build: timeout");
                IpcResponse::err(format!("build timed out after {}s", timeout.as_secs()))
            }
        };

        // Emit final build event: "finished" on success, "error" otherwise.
        let total_ms = started.elapsed().as_millis() as u64;
        if resp.ok {
            // The pipeline returns ok_data even when the inner command failed
            // (it stuffs an exit_code in AppExecResult). Inspect that to know.
            let exit_code = resp
                .data
                .as_ref()
                .and_then(|d| d.get("exit_code"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if exit_code == 0 {
                emit_build_event(
                    &self.app_build_tx,
                    &slug,
                    "finished",
                    Some(5),
                    Some(5),
                    None,
                    Some("build finished".to_string()),
                    Some(total_ms),
                    None,
                );
            } else {
                let err_msg = resp
                    .data
                    .as_ref()
                    .and_then(|d| d.get("stderr"))
                    .and_then(|v| v.as_str())
                    .map(|s| truncate(s, 512))
                    .unwrap_or_else(|| "build failed".to_string());
                emit_build_event(
                    &self.app_build_tx,
                    &slug,
                    "error",
                    None,
                    Some(5),
                    None,
                    None,
                    Some(total_ms),
                    Some(err_msg),
                );
            }
        } else {
            let err_msg = resp.error.clone().unwrap_or_else(|| "build failed".into());
            emit_build_event(
                &self.app_build_tx,
                &slug,
                "error",
                None,
                Some(5),
                None,
                None,
                Some(total_ms),
                Some(err_msg),
            );
        }

        // Refresh the per-app context (build command may have changed).
        let all = self.supervisor.registry.list().await;
        let db_tables = self.dv_list_tables_opt(&slug, app.has_db).await;
        if let Err(e) = self.context_generator.generate_for_app(&app, &all, db_tables) {
            warn!(slug = %slug, error = %e, "build: context regen failed (non-fatal)");
        }

        resp
    }

    /// Broadcast an externally-supplied AppBuildEvent on the per-app channel.
    /// Used by the `app-build` skill (running locally on Medion) to keep
    /// the Studio's per-app live panel in sync with build progress.
    #[allow(clippy::too_many_arguments)]
    pub async fn emit_external_build_event(
        &self,
        slug: String,
        status: String,
        phase: Option<String>,
        message: Option<String>,
        duration_ms: Option<u64>,
        error: Option<String>,
        step: Option<u32>,
        total_steps: Option<u32>,
    ) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        emit_build_event(
            &self.app_build_tx,
            &slug,
            &status,
            step,
            total_steps,
            phase,
            message,
            duration_ms,
            error,
        );
        IpcResponse::ok_data(serde_json::json!({ "ok": true }))
    }

    /// Ship pre-built artefacts from a remote build host to Medion + restart the
    /// supervised process. Skips compile (the agent already ran the build
    /// on the build host). Steps: stop → rsync-back per artefact → restart.
    /// Emits AppBuildEvents on the per-app channel so the Studio panel reflects
    /// progress.
    pub async fn ship(&self, slug: String, timeout_secs: Option<u64>) -> IpcResponse {
        if !valid_slug(&slug) {
            return IpcResponse::err("invalid slug");
        }
        let app = match self.supervisor.registry.get(&slug).await {
            Some(a) => a,
            None => return IpcResponse::err(format!("app not found: {slug}")),
        };

        // Artefacts : uniquement consommés par le rsync-back distant. En
        // local (le défaut) ship = stop + restart, aucun artefact requis.
        let artefacts = resolve_artefacts(&app);
        if build_host_config().is_some() && artefacts.is_empty() {
            return IpcResponse::err(
                "no artefacts to ship from the build host (set build_artefact)",
            );
        }

        // Per-slug lock partagé avec build() — pour éviter qu'un ship se
        // déclenche pendant qu'un build tourne (ou inversement).
        let lock = {
            let mut map = self.build_locks.lock().await;
            map.entry(slug.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                return IpcResponse::err(format!(
                    "BUILD_BUSY: another build/ship for '{slug}' is in progress. Wait."
                ));
            }
        };

        emit_build_event(
            &self.app_build_tx,
            &slug,
            "started",
            None,
            None,
            Some("ship".to_string()),
            Some("ship pipeline started (skip compile)".to_string()),
            None,
            None,
        );

        let host_cfg = build_host_config();
        let timeout = Duration::from_secs(timeout_secs.unwrap_or(900).max(60).min(7200));
        let started = Instant::now();
        let local_src = app.src_dir();
        let remote_src = local_src.display().to_string();

        let ctl_socket = format!("/tmp/atelier-ship-ssh-{}-{}.sock", slug, std::process::id());
        let ctl_path_opt = format!("ControlPath={ctl_socket}");
        let ssh_e_arg = host_cfg.as_ref().map(|(_, key)| format!(
            "ssh -i {key} -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
             -o ControlMaster=auto -o {ctl_path_opt} -o ControlPersist=30 \
             -o ServerAliveInterval=10 -o ServerAliveCountMax=3"
        ));

        match &host_cfg {
            Some((host, _)) => info!(slug = %slug, host = %host, "ship: start (remote)"),
            None => info!(slug = %slug, "ship: start (local — stop+restart only)"),
        }

        let app_build_tx = self.app_build_tx.clone();
        let slug_for_pipeline = slug.clone();
        let supervisor_for_pipeline = self.supervisor.clone();
        let pipeline = async {
            let mut acc = StageAccumulator::new();

            // 1) Stop the supervised process before overwriting artefacts.
            let stop_started = Instant::now();
            if let Err(e) = supervisor_for_pipeline.stop(&slug_for_pipeline).await {
                warn!(slug = %slug_for_pipeline, error = %e, "ship: pre-rsync stop failed (continuing)");
            }
            emit_build_event(
                &app_build_tx,
                &slug_for_pipeline,
                "step",
                Some(1),
                Some(3),
                Some("stop".to_string()),
                None,
                Some(stop_started.elapsed().as_millis() as u64),
                None,
            );

            // 2) Rsync each artefact back from the remote build host. For local
            // builds the artefacts are already in `local_src`, so we skip the
            // SSH/rsync loop entirely.
            let rsync_back_started = Instant::now();
            if let Some((host, key)) = &host_cfg {
                let ssh_e_arg = ssh_e_arg.as_ref().unwrap();
                for art_spec in &artefacts {
                    let (art, optional) = parse_artefact_spec(art_spec);
                    info!(slug = %slug_for_pipeline, artefact = %art, optional, "ship: rsync down");
                    let remote_path = format!("{}/{}", remote_src, art);
                    let local_path = local_src.join(art);
                    if let Some(parent) = local_path.parent() {
                        let _ = tokio::fs::create_dir_all(parent).await;
                    }
                    let exists = run_capture(
                        "ssh",
                        &[
                            "-i", key,
                            "-o", "BatchMode=yes",
                            "-o", "StrictHostKeyChecking=accept-new",
                            "-o", "ControlMaster=auto",
                            "-o", &ctl_path_opt,
                            "-o", "ControlPersist=30",
                            host,
                            &format!("test -e {}", shell_quote(&remote_path)),
                        ],
                        None,
                    )
                    .await;
                    if exists.exit_code != 0 {
                        if optional {
                            info!(slug = %slug_for_pipeline, artefact = %art, "ship: optional artefact absent, skipping");
                            continue;
                        }
                        acc.push(&format!("check-{art}"), &exists);
                        return acc.into_result(
                            format!("artefact missing on remote: {art} (build it first on the configured ATELIER_BUILD_HOST)"),
                            started,
                        );
                    }
                    let is_dir = run_capture(
                        "ssh",
                        &[
                            "-i", key,
                            "-o", "BatchMode=yes",
                            "-o", "StrictHostKeyChecking=accept-new",
                            "-o", "ControlMaster=auto",
                            "-o", &ctl_path_opt,
                            "-o", "ControlPersist=30",
                            host,
                            &format!("test -d {}", shell_quote(&remote_path)),
                        ],
                        None,
                    )
                    .await;
                    let dir_mode = is_dir.exit_code == 0;
                    let (src_arg, dst_arg, extra_args): (String, String, &[&str]) = if dir_mode {
                        let _ = tokio::fs::create_dir_all(&local_path).await;
                        (
                            format!("{}:{}/", host, remote_path),
                            format!("{}/", local_path.display()),
                            &["--delete"],
                        )
                    } else {
                        (
                            format!("{}:{}", host, remote_path),
                            local_path.display().to_string(),
                            &[],
                        )
                    };
                    let mut rsync_args: Vec<&str> = vec!["-a", "-W"];
                    rsync_args.extend_from_slice(extra_args);
                    rsync_args.extend_from_slice(&["-e", ssh_e_arg, &src_arg, &dst_arg]);
                    let down = run_capture("rsync", &rsync_args, None).await;
                    acc.push(&format!("rsync-down:{art}"), &down);
                    if down.exit_code != 0 {
                        return acc.into_result(format!("rsync down failed for {art}"), started);
                    }
                }
                emit_build_event(
                    &app_build_tx,
                    &slug_for_pipeline,
                    "step",
                    Some(2),
                    Some(3),
                    Some("rsync-back".to_string()),
                    None,
                    Some(rsync_back_started.elapsed().as_millis() as u64),
                    None,
                );
            } else {
                emit_build_event(
                    &app_build_tx,
                    &slug_for_pipeline,
                    "step",
                    Some(2),
                    Some(3),
                    Some("skipped:rsync-back (local build)".to_string()),
                    Some("local build".to_string()),
                    Some(0),
                    None,
                );
            }

            // 3) Restart supervisor. A start failure means the app is down —
            // ship must report failure (exit_code != 0), not success.
            let restart_started = Instant::now();
            let start_err = supervisor_for_pipeline.start(&slug_for_pipeline).await.err();
            emit_build_event(
                &app_build_tx,
                &slug_for_pipeline,
                "step",
                Some(3),
                Some(3),
                Some("restart".to_string()),
                start_err.as_ref().map(|e| format!("start failed: {e}")),
                Some(restart_started.elapsed().as_millis() as u64),
                None,
            );
            if let Some(e) = start_err {
                error!(slug = %slug_for_pipeline, error = %e, "ship: post-rsync start failed — app is down");
                return acc.into_result(
                    format!("artefacts shipped but the app failed to start: {e}"),
                    started,
                );
            }

            info!(slug = %slug_for_pipeline, duration_ms = started.elapsed().as_millis() as u64, "ship: ok");
            acc.into_result_ok(started)
        };

        let resp = match tokio::time::timeout(timeout, pipeline).await {
            Ok(r) => r,
            Err(_) => {
                error!(slug = %slug, "ship: timeout");
                IpcResponse::err(format!("ship timed out after {}s", timeout.as_secs()))
            }
        };

        let total_ms = started.elapsed().as_millis() as u64;
        // The pipeline returns ok_data even on failure (exit_code stuffed into
        // AppExecResult) — inspect it so a failed ship emits "error", not
        // "finished".
        let exit_code = resp
            .data
            .as_ref()
            .and_then(|d| d.get("exit_code"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if resp.ok && exit_code == 0 {
            emit_build_event(
                &self.app_build_tx,
                &slug,
                "finished",
                Some(3),
                Some(3),
                Some("ship".to_string()),
                Some("ship finished".to_string()),
                Some(total_ms),
                None,
            );
        } else {
            let err_msg = resp
                .error
                .clone()
                .or_else(|| {
                    resp.data
                        .as_ref()
                        .and_then(|d| d.get("stderr"))
                        .and_then(|v| v.as_str())
                        .map(|s| truncate(s, 512))
                })
                .unwrap_or_else(|| "ship failed".into());
            emit_build_event(
                &self.app_build_tx,
                &slug,
                "error",
                None,
                Some(3),
                Some("ship".to_string()),
                None,
                Some(total_ms),
                Some(err_msg),
            );
        }

        resp
    }
}

/// Parse a single line of the `build_artefact` spec. Lines starting with
/// `?` are treated as **optional** artefacts: the rsync skips silently if
/// the source path is absent on the build host, instead of erroring out.
/// Used for inputs like custom-server bundles (`server.js` for Next.js
/// custom-HTTP-handler apps) that some apps emit and others don't.
///
/// Returns `(path, optional)`.
fn parse_artefact_spec(spec: &str) -> (&str, bool) {
    match spec.strip_prefix('?') {
        Some(rest) => (rest, true),
        None => (spec, false),
    }
}

/// Resolve the list of artefacts to rsync down for `build`/`ship`. Reads
/// `app.build_artefact` if set (one path per line, `?` prefix = optional).
/// No stack defaults: the platform is stack-agnostic, artefacts are declared
/// per app (and only matter for the remote-build-host rsync path).
fn resolve_artefacts(app: &Application) -> Vec<String> {
    app.build_artefact
        .as_deref()
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

struct StageOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    duration_ms: u64,
}

struct StageAccumulator {
    stdout: String,
    stderr: String,
    last_exit: i32,
    total_ms: u64,
}

impl StageAccumulator {
    fn new() -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            last_exit: 0,
            total_ms: 0,
        }
    }

    fn push(&mut self, stage: &str, out: &StageOutput) {
        self.stdout.push_str(&format!("\n=== {stage} (exit={}, {}ms) ===\n", out.exit_code, out.duration_ms));
        self.stdout.push_str(&out.stdout);
        if !out.stderr.is_empty() {
            self.stderr.push_str(&format!("\n=== {stage} ===\n"));
            self.stderr.push_str(&out.stderr);
        }
        self.total_ms += out.duration_ms;
        if out.exit_code != 0 && self.last_exit == 0 {
            self.last_exit = out.exit_code;
        }
    }

    fn into_result(mut self, message: String, started: Instant) -> IpcResponse {
        if !self.stderr.is_empty() {
            self.stderr.push('\n');
        }
        self.stderr.push_str(&message);
        let exit = if self.last_exit == 0 { 1 } else { self.last_exit };
        let result = AppExecResult {
            stdout: cap_string(self.stdout),
            stderr: cap_string(self.stderr),
            exit_code: exit,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        IpcResponse::ok_data(result)
    }

    fn into_result_ok(self, started: Instant) -> IpcResponse {
        let result = AppExecResult {
            stdout: cap_string(self.stdout),
            stderr: cap_string(self.stderr),
            exit_code: 0,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        IpcResponse::ok_data(result)
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_build_event(
    tx: &broadcast::Sender<AppBuildEvent>,
    slug: &str,
    status: &str,
    step: Option<u32>,
    total_steps: Option<u32>,
    phase: Option<String>,
    message: Option<String>,
    duration_ms: Option<u64>,
    error: Option<String>,
) {
    let event = AppBuildEvent {
        slug: slug.to_string(),
        status: status.to_string(),
        step,
        total_steps,
        phase: phase.clone(),
        message,
        duration_ms,
        error: error.clone(),
    };
    info!(
        slug = %slug,
        status = %status,
        step = ?step,
        phase = ?phase,
        duration_ms = ?duration_ms,
        error = ?error,
        "AppBuildEvent emitted"
    );
    let _ = tx.send(event);
}

fn cap_string(mut s: String) -> String {
    if s.len() > OUTPUT_CAP_BYTES {
        let cut = OUTPUT_CAP_BYTES;
        // Snap to a char boundary
        let mut idx = cut;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        s.truncate(idx);
        s.push_str("\n[truncated]\n");
    }
    s
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut idx = n;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        format!("{}…", &s[..idx])
    }
}

fn shell_quote(s: &str) -> String {
    // Single-quote everything; embed any internal single quotes.
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

async fn run_capture(program: &str, args: &[&str], cwd: Option<&std::path::Path>) -> StageOutput {
    let started = Instant::now();
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let child = cmd.spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return StageOutput {
                stdout: String::new(),
                stderr: format!("spawn {program}: {e}"),
                exit_code: -1,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };
    let out = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => {
            return StageOutput {
                stdout: String::new(),
                stderr: format!("wait {program}: {e}"),
                exit_code: -1,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
    };
    StageOutput {
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        exit_code: out.status.code().unwrap_or(-1),
        duration_ms: started.elapsed().as_millis() as u64,
    }
}

// ── Helpers ────────────────────────────────────────────────────

fn parse_visibility(s: &str) -> Option<Visibility> {
    match s {
        "public" => Some(Visibility::Public),
        "private" => Some(Visibility::Private),
        _ => None,
    }
}

fn visibility_to_str(v: &Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Private => "private",
    }
}

fn state_to_str(s: &AppState) -> &'static str {
    match s {
        AppState::Stopped => "stopped",
        AppState::Starting => "starting",
        AppState::Running => "running",
        AppState::Stopping => "stopping",
        AppState::Crashed => "crashed",
        AppState::Unknown => "unknown",
    }
}

pub fn app_to_dto(app: &Application) -> ApplicationDto {
    ApplicationDto {
        slug: app.slug.clone(),
        name: app.name.clone(),
        description: app.description.clone(),
        stack: app.stack.clone(),
        has_db: app.has_db,
        claude_access: app.claude_access,
        visibility: visibility_to_str(&app.visibility).to_string(),
        domain: app.domain.clone(),
        port: app.port,
        run_command: app.run_command.clone(),
        build_command: app.build_command.clone(),
        build_artefact: app.build_artefact.clone(),
        health_path: app.health_path.clone(),
        // User var names only, values masked. Sourced from the structured `env`
        // model (the legacy flat `env_vars` map is now always empty). Platform
        // vars (PORT/HR_DV_*/ATELIER_*) are intentionally not surfaced here.
        env_vars: app
            .env
            .iter()
            .map(|e| (e.key.clone(), "***".to_string()))
            .collect(),
        state: state_to_str(&app.state).to_string(),
        db_backend: db_backend_to_str(&app.db_backend).to_string(),
        created_at: app.created_at.to_rfc3339(),
        updated_at: app.updated_at.to_rfc3339(),
    }
}

fn db_backend_to_str(b: &DbBackend) -> &'static str {
    match b {
        DbBackend::PostgresDataverse => "postgres-dataverse",
    }
}

fn process_status_to_dto(slug: &str, s: &ProcessStatus) -> AppStatusData {
    AppStatusData {
        slug: slug.to_string(),
        pid: s.pid,
        state: state_to_str(&s.state).to_string(),
        port: s.port,
        uptime_secs: s.uptime_secs,
        restart_count: s.restart_count,
        exe_path: s.exe_path.clone(),
        exe_mtime: s.exe_mtime,
    }
}

/// Initialise (idempotent) le working tree git de l'app et pointe `origin`
/// vers le bare repo atelier-git servi par Atelier (Smart-HTTP). Idempotent : safe
/// à re-rouler. Si `ATELIER_BUILD_HOST` est défini, exécute via SSH ; sinon
/// localement dans le src_dir de l'app.
pub(crate) async fn bind_git_remote_for_slug(slug: &str) -> anyhow::Result<()> {
    let api_base = std::env::var("ATELIER_GIT_API_BASE").unwrap_or_else(|_| GIT_API_BASE.to_string());
    let origin_url = format!("{api_base}/api/git/repos/{slug}.git");
    let apps_root = std::env::var("ATELIER_APPS_RUNTIME_ROOT")
        .unwrap_or_else(|_| "/var/lib/atelier/apps".to_string());
    let src = format!("{apps_root}/{slug}/src");
    // `git init` est idempotent ; `remote set-url` (fallback `add`) garantit la
    // bonne URL même si un remote `origin` existait déjà.
    let cmd = format!(
        "set -e; cd {src}; git init -q; \
         git remote set-url origin {url} 2>/dev/null || git remote add origin {url}",
        src = shell_quote(&src),
        url = shell_quote(&origin_url),
    );
    let out = match build_host_config() {
        Some((host, key)) => {
            run_capture(
                "ssh",
                &[
                    "-i", &key,
                    "-o", "BatchMode=yes",
                    "-o", "StrictHostKeyChecking=accept-new",
                    &host,
                    &cmd,
                ],
                None,
            )
            .await
        }
        None => {
            let (prog, args) = wrap_local_cmd(&cmd);
            let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
            run_capture(prog, &args_ref, None).await
        }
    };
    if out.exit_code != 0 {
        anyhow::bail!(
            "git remote bind failed (exit={}): {}",
            out.exit_code,
            truncate(&out.stderr, 256)
        );
    }
    info!(slug, origin = %origin_url, "git origin bound");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Régression : un AppBuildEvent émis sur un `Sender` est livré à un
    /// `subscribe()` du MÊME canal. C'est exactement ce qui était cassé — les
    /// AppsContext étaient construits avec un canal JETABLE (zéro abonné), donc
    /// le badge ne s'allumait jamais. `from_api_state` câble désormais
    /// `state.events.app_build.clone()` (même canal que le relais WebSocket).
    #[tokio::test]
    async fn emit_build_event_reaches_subscriber_of_same_channel() {
        let (tx, mut rx) = broadcast::channel::<AppBuildEvent>(8);
        emit_build_event(
            &tx,
            "trader",
            "started",
            None,
            None,
            Some("compile".to_string()),
            Some("local build".to_string()),
            None,
            None,
        );
        let ev = rx.try_recv().expect("event delivered to subscriber");
        assert_eq!(ev.slug, "trader");
        assert_eq!(ev.status, "started");
        assert_eq!(ev.phase.as_deref(), Some("compile"));
    }

    /// Un Sender CLONÉ partage le canal (le pattern de `from_api_state`) : un
    /// event émis via le clone atteint l'abonné de l'original.
    #[tokio::test]
    async fn cloned_sender_shares_channel() {
        let (tx, mut rx) = broadcast::channel::<AppBuildEvent>(8);
        let cloned = tx.clone();
        assert!(cloned.same_channel(&tx));
        emit_build_event(&cloned, "home", "finished", None, None, None, None, Some(1234), None);
        let ev = rx.try_recv().expect("event from clone delivered");
        assert_eq!(ev.slug, "home");
        assert_eq!(ev.status, "finished");
        assert_eq!(ev.duration_ms, Some(1234));
    }
}
