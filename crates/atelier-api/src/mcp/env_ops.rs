//! Unified environment management for apps — the single authority for what an
//! app's env is, replacing the old split where the on-disk `.env` file and the
//! registry `env_vars` map were merged blindly at spawn.
//!
//! Three ownership tiers:
//!  - **platform** — computed from app identity + secrets (`PORT`, `HR_DV_*`,
//!    `ATELIER_*`). Never stored; recomputed on every render so token rotations
//!    and identity changes propagate and stale copies are GC'd.
//!  - **user config / user secret** — the structured [`EnvVar`] list on the
//!    `Application`. The `secret` flag drives UI masking + per-row reveal; the
//!    value is stored as-is in JSONB (same plaintext exposure as
//!    `dataverse-secrets.json`).
//!
//! [`AppsContext::render_env`] produces the deterministic projection;
//! [`AppsContext::reconcile_app_env`] is the SOLE writer of the `.env` file: it
//! imports any residual hand-seeded vars into the model once, GCs dead vars, and
//! rewrites the file. The supervisor then reads that file as the single delivery
//! channel (identical for Node `process.env` and Rust `std::env`).

use std::collections::BTreeSet;
use std::path::Path;

use atelier_apps::types::{Application, EnvScope, EnvVar, valid_env_key};
use serde::Serialize;
use tracing::{info, warn};

use super::apps_ops::AppsContext;

/// Loopback base URL of the Atelier API as seen from an app process.
const API_LOOPBACK: &str = "http://127.0.0.1:4100";

/// Platform-managed keys: recomputed each render, never user-editable. The API
/// rejects user writes to these and the importer never folds them into the user
/// model (they are recomputed, or dropped if no longer applicable).
pub const PLATFORM_KEYS: &[&str] = &[
    "PORT",
    "HR_DV_BASE_URL",
    "HR_DV_TOKEN",
    "HR_APP_UUID",
    "ATELIER_INGEST_URL",
    "ATELIER_LOGS_TOKEN",
    // Injecté (secret) aux apps opt-in `claude_access` depuis `app_claude_auth`.
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// Vestiges of eradicated subsystems. Dropped on import (never folded into the
/// user model) and never re-rendered → the next reconcile permanently GCs them.
const DEAD_KEYS: &[&str] = &[
    "HR_FLOW_TOKEN",
    "HR_FLOW_BACKEND",
    "HR_FLOWD_URL",
    "HR_FLOWD_TOKEN",
    "FLOW_RUNS_DIR",
];

pub fn is_platform_key(key: &str) -> bool {
    PLATFORM_KEYS.contains(&key)
}

fn is_dead_key(key: &str) -> bool {
    DEAD_KEYS.contains(&key)
}

/// Clés qu'une app n'a JAMAIS à définir : la plateforme gère l'auth Claude des
/// apps (via `CLAUDE_CODE_OAUTH_TOKEN` injecté quand `claude_access`). Laisser une
/// app poser `CLAUDE_CONFIG_DIR` lui permettait de pointer le dossier de config du
/// runner (`/var/lib/hr-studio/.claude`) et, tournant en root, d'en clobberer le
/// `.credentials.json` → toute la pile agent hr-studio cassée (iss-d10ef97b).
/// Rejetées à l'écriture + GC'd à l'import/render (comme [`DEAD_KEYS`]).
const FORBIDDEN_KEYS: &[&str] = &["CLAUDE_CONFIG_DIR"];

fn is_forbidden_key(key: &str) -> bool {
    FORBIDDEN_KEYS.contains(&key)
}

/// Répertoires propriété de la plateforme : une var d'app dont la valeur pointe
/// dessous est refusée (sinon un `HOME=/var/lib/hr-studio` recréerait le vecteur
/// de clobber sans passer par `CLAUDE_CONFIG_DIR`).
const PLATFORM_OWNED_PREFIXES: &[&str] =
    &["/var/lib/hr-studio", "/opt/atelier", "/var/lib/atelier/state"];

/// True si la valeur (ou un de ses segments PATH-style `a:/b`) résout sous un
/// répertoire plateforme. Garde lexical (pas de canonicalisation FS — le chemin
/// peut ne pas exister ; un garde de config n'a pas à suivre les symlinks).
fn value_targets_platform_path(value: &str) -> bool {
    value
        .split([':', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|tok| {
            PLATFORM_OWNED_PREFIXES
                .iter()
                .any(|p| tok == *p || tok.starts_with(&format!("{p}/")))
        })
}

/// Une var USER doit-elle être bannie du modèle et du `.env` rendu ? (clé
/// interdite OU valeur pointant une zone plateforme). Point de vérité partagé par
/// l'écriture (`env_set_var`), l'import (`import_hand_seeded`) et le rendu
/// (`render_env`).
pub(crate) fn is_forbidden_user_var(key: &str, value: &str) -> bool {
    is_forbidden_key(key) || value_targets_platform_path(value)
}

/// Heuristic: does this key name denote a secret? Used when importing
/// hand-seeded `.env` / legacy-map vars and when folding the MCP `app.update`
/// flat map into the model (so they get sealed and masked). Explicit per-key
/// user/API writes carry their own `secret` flag.
pub(crate) fn looks_secret(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    ["TOKEN", "SECRET", "KEY", "PASSWORD", "PASSWD", "PWD", "CREDENTIAL", "PRIVATE"]
        .iter()
        .any(|needle| k.contains(needle))
}

/// One rendered variable with provenance — feeds both the API view and the
/// `.env` writer. `value` is plaintext (sealed user secrets are opened here).
#[derive(Debug, Clone)]
pub struct RenderedVar {
    pub key: String,
    pub value: String,
    pub secret: bool,
    pub scope: EnvScope,
    pub platform: bool,
}

/// JSON-facing view of one variable (value omitted when masked).
#[derive(Debug, Serialize)]
pub struct EnvVarView {
    pub key: String,
    pub owner: &'static str,
    pub scope: &'static str,
    pub secret: bool,
    pub editable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub has_value: bool,
}

/// Outcome of a reconcile pass — what the `.env` projection changed. Secret
/// VALUES are never included, only key names.
#[derive(Debug, Serialize)]
pub struct ReconcileReport {
    pub slug: String,
    pub dry_run: bool,
    /// Hand-seeded vars folded into the user model this pass.
    pub imported: Vec<String>,
    /// Keys newly present in the rendered `.env`.
    pub added: Vec<String>,
    /// Keys GC'd from the `.env` (dead vars, dropped platform vestiges).
    pub removed: Vec<String>,
    /// Keys present before and after.
    pub kept: usize,
    /// Whether the `.env` file was actually rewritten.
    pub wrote: bool,
}

impl AppsContext {
    /// Compute the PLATFORM tier for `app`. Values the platform cannot supply
    /// (e.g. `ATELIER_LOGS_TOKEN` absent from atelier's own env) are omitted.
    pub async fn platform_env(&self, app: &Application) -> Vec<RenderedVar> {
        let mut out = vec![RenderedVar {
            key: "PORT".into(),
            value: app.port.to_string(),
            secret: false,
            scope: EnvScope::Runtime,
            platform: true,
        }];

        // Dataverse gateway contract — only for db-backed, provisioned apps.
        if app.has_db {
            if let Some(mgr) = self.dataverse_manager.as_ref() {
                match mgr.ensure_gateway_credentials(&app.slug) {
                    Ok(secret) => {
                        out.push(RenderedVar {
                            key: "HR_DV_BASE_URL".into(),
                            value: format!("{API_LOOPBACK}/api/dv/{}", app.slug),
                            secret: false,
                            scope: EnvScope::Runtime,
                            platform: true,
                        });
                        out.push(RenderedVar {
                            key: "HR_DV_TOKEN".into(),
                            value: secret.gateway_token.clone(),
                            secret: true,
                            scope: EnvScope::Runtime,
                            platform: true,
                        });
                        out.push(RenderedVar {
                            key: "HR_APP_UUID".into(),
                            value: secret.app_uuid.to_string(),
                            secret: false,
                            scope: EnvScope::Runtime,
                            platform: true,
                        });
                    }
                    Err(e) => {
                        warn!(slug = %app.slug, error = %e, "platform_env: gateway credentials unavailable — HR_DV_* omitted");
                    }
                }
            }
        }

        // Logging-shipper contract. INGEST_URL is the loopback API base; the
        // token is the platform's own (atelier's process env / /opt/atelier/.env).
        out.push(RenderedVar {
            key: "ATELIER_INGEST_URL".into(),
            value: API_LOOPBACK.into(),
            secret: false,
            scope: EnvScope::Runtime,
            platform: true,
        });
        if let Ok(tok) = std::env::var("ATELIER_LOGS_TOKEN") {
            if !tok.is_empty() {
                out.push(RenderedVar {
                    key: "ATELIER_LOGS_TOKEN".into(),
                    value: tok,
                    secret: true,
                    scope: EnvScope::Runtime,
                    platform: true,
                });
            }
        }

        // Auth Claude des apps — uniquement pour les apps opt-in, et seulement si
        // un token est configuré (Paramètres → Token Claude pour les apps). Le SDK
        // (JS ou Python) reconnaît `CLAUDE_CODE_OAUTH_TOKEN` en top-level : l'app
        // n'a besoin d'aucun `CLAUDE_CONFIG_DIR` ni fichier partagé.
        if app.claude_access {
            match claude_token_var(self.app_claude_auth.token().await) {
                Some(var) => out.push(var),
                None => warn!(slug = %app.slug, "platform_env: claude_access activé mais aucun token apps configuré — CLAUDE_CODE_OAUTH_TOKEN omis"),
            }
        }
        out
    }

    /// Full rendered env: platform tier + user tier (secrets opened). User vars
    /// never override platform keys.
    pub async fn render_env(&self, app: &Application) -> Vec<RenderedVar> {
        let mut rendered = self.platform_env(app).await;
        let platform_keys: BTreeSet<String> =
            rendered.iter().map(|v| v.key.clone()).collect();
        for ev in &app.env {
            if platform_keys.contains(&ev.key) {
                continue;
            }
            // Garde de livraison : une var interdite qui aurait échappé aux gardes
            // d'écriture/import ne doit JAMAIS atteindre le `.env` rendu.
            if is_forbidden_user_var(&ev.key, &ev.value) {
                continue;
            }
            rendered.push(RenderedVar {
                key: ev.key.clone(),
                value: ev.value.clone(),
                secret: ev.secret,
                scope: ev.scope,
                platform: false,
            });
        }
        rendered
    }

    /// JSON view for `GET /api/apps/{slug}/env`. Platform vars first, then user,
    /// each sorted by key. Secret values are masked unless `reveal`.
    pub async fn env_view(&self, slug: &str, reveal: bool) -> anyhow::Result<Vec<EnvVarView>> {
        let app = self
            .supervisor
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow::anyhow!("app not found: {slug}"))?;
        let rendered = self.render_env(&app).await;
        let mut out: Vec<EnvVarView> = rendered
            .into_iter()
            .map(|v| {
                let masked = v.secret && !reveal;
                EnvVarView {
                    key: v.key,
                    owner: if v.platform { "platform" } else { "user" },
                    scope: v.scope.as_str(),
                    secret: v.secret,
                    editable: !v.platform,
                    value: if masked { None } else { Some(v.value) },
                    has_value: true,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            let rank = |o: &str| if o == "platform" { 0 } else { 1 };
            rank(a.owner).cmp(&rank(b.owner)).then(a.key.cmp(&b.key))
        });
        Ok(out)
    }

    /// User vars to inject into the BUILD command env (scope `build`/`both`),
    /// opened. Platform vars are runtime-only and never exposed to the build.
    /// This is the channel Node frameworks need for baked public vars
    /// (`VITE_*` / `NEXT_PUBLIC_*`); Rust apps read everything at runtime so
    /// they simply won't have build-scoped vars — same mechanism, both stacks.
    pub async fn render_build_env(&self, app: &Application) -> Vec<(String, String)> {
        app.env
            .iter()
            .filter(|e| e.scope.in_build())
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect()
    }

    /// `eval`-able `export K='v'` lines for the build-scoped vars of `slug`.
    /// Consumed by the generated `build.sh` and `deploy-app.sh` over loopback.
    pub async fn build_env_script(&self, slug: &str) -> anyhow::Result<String> {
        let app = self
            .supervisor
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow::anyhow!("app not found: {slug}"))?;
        Ok(build_env_sh_script(&self.render_build_env(&app).await))
    }

    /// Reveal one variable's plaintext value (platform or user). Used by the
    /// per-row "eye" so secrets stay out of the bulk view payload.
    pub async fn env_var_value(&self, slug: &str, key: &str) -> anyhow::Result<Option<String>> {
        let app = self
            .supervisor
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow::anyhow!("app not found: {slug}"))?;
        Ok(self
            .render_env(&app)
            .await
            .into_iter()
            .find(|v| v.key == key)
            .map(|v| v.value))
    }

    /// Insert or replace a single USER variable, then re-render the `.env`.
    /// Rejects platform keys and malformed names.
    pub async fn env_set_var(
        &self,
        slug: &str,
        key: &str,
        value: &str,
        secret: bool,
        scope: EnvScope,
    ) -> anyhow::Result<()> {
        if !valid_env_key(key) {
            anyhow::bail!("invalid env key (must match ^[A-Za-z_][A-Za-z0-9_]*$)");
        }
        if is_platform_key(key) {
            anyhow::bail!("'{key}' is platform-managed and cannot be set manually");
        }
        if is_forbidden_key(key) {
            anyhow::bail!(
                "'{key}' est géré par la plateforme (l'app reçoit CLAUDE_CODE_OAUTH_TOKEN \
                 si claude_access est activé) et ne peut pas être défini par l'app"
            );
        }
        if value_targets_platform_path(value) {
            anyhow::bail!(
                "valeur interdite pour '{key}' : pointe sous un répertoire plateforme \
                 (/var/lib/hr-studio, /opt/atelier, /var/lib/atelier/state)"
            );
        }
        if value.contains('\n') || value.contains('\r') {
            anyhow::bail!("env values cannot contain newlines");
        }
        let _guard = self.supervisor.registry.lock_slug(slug).await;
        let mut app = self
            .supervisor
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow::anyhow!("app not found: {slug}"))?;
        // Absorb any residual hand-seeded vars into the model FIRST, so a
        // render-only write below never drops them. (No-op once migrated.)
        self.import_hand_seeded(&mut app).await;
        app.env_set(EnvVar { key: key.to_string(), value: value.to_string(), secret, scope });
        self.supervisor.registry.upsert(app.clone()).await?;
        self.write_env_file(&app).await?;
        info!(slug, key, secret, scope = scope.as_str(), "env_set_var");
        Ok(())
    }

    /// Remove a single USER variable, then re-render the `.env`. Returns false
    /// if the key didn't exist as a user var.
    pub async fn env_delete_var(&self, slug: &str, key: &str) -> anyhow::Result<bool> {
        if is_platform_key(key) {
            anyhow::bail!("'{key}' is platform-managed and cannot be removed");
        }
        let _guard = self.supervisor.registry.lock_slug(slug).await;
        let mut app = self
            .supervisor
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow::anyhow!("app not found: {slug}"))?;
        // Absorb hand-seeded vars BEFORE removing — otherwise the target, still
        // present on disk, would be re-imported and the delete would not stick.
        self.import_hand_seeded(&mut app).await;
        let removed = app.env_remove(key);
        // Persist + render even if the key was absent from the model: the import
        // above may have changed the model, and we still want a clean `.env`.
        self.supervisor.registry.upsert(app.clone()).await?;
        self.write_env_file(&app).await?;
        if removed {
            info!(slug, key, "env_delete_var");
        }
        Ok(removed)
    }

    /// Fold the legacy flat `env_vars` map + residual hand-seeded `.env` vars
    /// into the structured user model. Platform + dead keys are never imported;
    /// keys already in the model are left untouched. Mutates `app` in place;
    /// returns the imported key names. Idempotent — a no-op once migrated.
    async fn import_hand_seeded(&self, app: &mut Application) -> Vec<String> {
        let mut imported = Vec::new();

        // GC ACTIF du modèle : une var interdite déjà présente dans `app.env`
        // (posée avant le garde-fou, ex. le `CLAUDE_CONFIG_DIR` de print-forge)
        // est retirée du registre, pas seulement filtrée au rendu. Le reconcile
        // la reportera dans `removed`.
        app.env
            .retain(|ev| !is_forbidden_user_var(&ev.key, &ev.value));

        let legacy: Vec<(String, String)> =
            app.env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (k, v) in legacy {
            if is_platform_key(&k)
                || is_dead_key(&k)
                || is_forbidden_user_var(&k, &v)
                || app.env_get(&k).is_some()
            {
                continue;
            }
            let secret = looks_secret(&k);
            app.env_set(EnvVar { key: k.clone(), value: v, secret, scope: EnvScope::Runtime });
            imported.push(format!("{k} (legacy-map)"));
        }
        app.env_vars.clear();

        for (k, v) in read_env_file(&app.env_file()).await {
            if is_platform_key(&k)
                || is_dead_key(&k)
                || is_forbidden_user_var(&k, &v)
                || app.env_get(&k).is_some()
            {
                continue;
            }
            let secret = looks_secret(&k);
            app.env_set(EnvVar { key: k.clone(), value: v, secret, scope: EnvScope::Runtime });
            imported.push(k);
        }
        imported
    }

    /// Render the app's runtime env (platform + user) and write the `.env`
    /// projection, but ONLY if the content changed. Render-only: does NOT
    /// import or mutate the model. Returns whether the file was rewritten.
    async fn write_env_file(&self, app: &Application) -> anyhow::Result<bool> {
        let rendered = self.render_env(app).await;
        let content = render_dotenv(&rendered);
        let env_path = app.env_file();
        if let Some(parent) = env_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let current = tokio::fs::read_to_string(&env_path).await.unwrap_or_default();
        if current != content {
            tokio::fs::write(&env_path, &content).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Reconcile an app's env: import residual hand-seeded vars into the model
    /// (once, idempotent), GC dead vars, and rewrite the `.env` as a clean
    /// projection of (platform + user). Used by the boot sweep, app create, and
    /// token rotation. The per-variable mutations use `import_hand_seeded` +
    /// `write_env_file` directly (so a delete is not re-imported).
    ///
    /// `dry_run` computes the would-be projection and reports the diff WITHOUT
    /// persisting the model or touching the file.
    pub async fn reconcile_app_env(
        &self,
        slug: &str,
        dry_run: bool,
    ) -> anyhow::Result<ReconcileReport> {
        let _guard = self.supervisor.registry.lock_slug(slug).await;
        let mut app = self
            .supervisor
            .registry
            .get(slug)
            .await
            .ok_or_else(|| anyhow::anyhow!("app not found: {slug}"))?;

        let existing_keys: BTreeSet<String> = read_env_file(&app.env_file())
            .await
            .into_iter()
            .map(|(k, _)| k)
            .collect();

        let before_env = app.env.clone();
        let had_legacy = !app.env_vars.is_empty();
        let imported = self.import_hand_seeded(&mut app).await;
        let model_changed = !imported.is_empty() || had_legacy || app.env != before_env;

        let rendered = self.render_env(&app).await;
        let new_keys: BTreeSet<String> = rendered
            .iter()
            .filter(|v| v.scope.in_runtime())
            .map(|v| v.key.clone())
            .collect();
        let added: Vec<String> = new_keys.difference(&existing_keys).cloned().collect();
        let removed: Vec<String> = existing_keys.difference(&new_keys).cloned().collect();
        let kept = new_keys.intersection(&existing_keys).count();

        let mut wrote = false;
        if !dry_run {
            if model_changed {
                self.supervisor.registry.upsert(app.clone()).await?;
            }
            wrote = self.write_env_file(&app).await?;
        }

        let report = ReconcileReport {
            slug: slug.to_string(),
            dry_run,
            imported,
            added,
            removed,
            kept,
            wrote,
        };
        info!(
            slug,
            dry_run,
            imported = report.imported.len(),
            added = ?report.added,
            removed = ?report.removed,
            kept = report.kept,
            wrote,
            "reconcile_app_env"
        );
        Ok(report)
    }

    /// Reconcile every app's env. Boot sweep — replaces the dead
    /// `sync_dv_env_all`. `dry_run` only logs the plan (gated by
    /// `ATELIER_ENV_RECONCILE_APPLY` at the boot call site).
    pub async fn reconcile_all_env(&self, dry_run: bool) -> Vec<ReconcileReport> {
        let apps = self.supervisor.registry.list().await;
        let mut reports = Vec::with_capacity(apps.len());
        for app in apps {
            match self.reconcile_app_env(&app.slug, dry_run).await {
                Ok(r) => reports.push(r),
                Err(e) => warn!(slug = %app.slug, error = %e, "reconcile_all_env: app failed"),
            }
        }
        reports
    }
}

/// Render the deterministic, sectioned `.env` content from runtime-scoped vars.
/// Platform section first, then user section, each sorted by key. Values are
/// written raw (`KEY=value`) — matching the supervisor's naive `load_env_file`
/// parser, which trims surrounding quotes (so we never add them).
fn render_dotenv(vars: &[RenderedVar]) -> String {
    let mut platform: Vec<&RenderedVar> =
        vars.iter().filter(|v| v.platform && v.scope.in_runtime()).collect();
    let mut user: Vec<&RenderedVar> =
        vars.iter().filter(|v| !v.platform && v.scope.in_runtime()).collect();
    platform.sort_by(|a, b| a.key.cmp(&b.key));
    user.sort_by(|a, b| a.key.cmp(&b.key));

    let mut out = String::new();
    out.push_str("# Généré par Atelier — NE PAS ÉDITER À LA MAIN.\n");
    out.push_str("# Source de vérité : le modèle env de l'app (UI Studio / API /apps/{slug}/env).\n");
    out.push_str("# Régénéré à chaque reconcile (création, boot, changement d'env, rotation de token).\n\n");
    out.push_str("# --- Plateforme (calculé, non éditable) ---\n");
    for v in &platform {
        out.push_str(&format!("{}={}\n", v.key, v.value));
    }
    if !user.is_empty() {
        out.push_str("\n# --- Variables applicatives (gérées via le Studio) ---\n");
        for v in &user {
            out.push_str(&format!("{}={}\n", v.key, v.value));
        }
    }
    out
}

/// Single-quote a value for safe shell `eval`/`export` (`'` → `'\''`).
fn sh_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build-scoped exports as a prefix injected inline before the build command:
/// `export K='v' && export K2='v2' && ` (empty when there are none).
pub fn build_env_sh_prefix(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("export {}={} && ", k, sh_squote(v)))
        .collect()
}

/// Build-scoped exports as newline-separated `export` lines, for `eval` by the
/// generated `build.sh` / `deploy-app.sh`.
fn build_env_sh_script(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("export {}={}\n", k, sh_squote(v)))
        .collect()
}

/// Rendu du token Claude des apps (`CLAUDE_CODE_OAUTH_TOKEN`) en variable
/// plateforme secrète runtime. `None` quand aucun token n'est configuré — le
/// caller émet alors le warn d'observabilité et la clé est simplement omise.
///
/// Extrait de `platform_env` pour être testable sans store live : c'est
/// exactement ce que `reconcile_app_env` écrit dans le `.env`, donc l'invariant
/// dont dépend la propagation du token vers les apps opt-in `claude_access`.
fn claude_token_var(token: Option<String>) -> Option<RenderedVar> {
    token.map(|value| RenderedVar {
        key: "CLAUDE_CODE_OAUTH_TOKEN".into(),
        value,
        secret: true,
        scope: EnvScope::Runtime,
        platform: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rv(key: &str, value: &str, scope: EnvScope, platform: bool) -> RenderedVar {
        RenderedVar { key: key.into(), value: value.into(), secret: false, scope, platform }
    }

    #[test]
    fn render_dotenv_sections_sorts_and_excludes_build_scope() {
        let vars = vec![
            rv("PORT", "3007", EnvScope::Runtime, true),
            rv("ATELIER_INGEST_URL", "http://x", EnvScope::Runtime, true),
            rv("ZED", "z", EnvScope::Runtime, false),
            rv("ALPHA", "a", EnvScope::Runtime, false),
            rv("VITE_X", "vx", EnvScope::Build, false), // build-only → excluded
        ];
        let out = render_dotenv(&vars);
        // Platform section sorted, then user section sorted; build var absent.
        let body: Vec<&str> = out.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
        assert_eq!(
            body,
            vec!["ATELIER_INGEST_URL=http://x", "PORT=3007", "ALPHA=a", "ZED=z"]
        );
        assert!(!out.contains("VITE_X"));
        // Platform header precedes the user header.
        assert!(out.find("Plateforme").unwrap() < out.find("applicatives").unwrap());
    }

    #[test]
    fn build_env_prefix_and_script_quote_safely() {
        let vars = vec![
            ("VITE_A".to_string(), "plain".to_string()),
            ("VITE_B".to_string(), "has 'quote' and space".to_string()),
        ];
        let prefix = build_env_sh_prefix(&vars);
        assert_eq!(prefix, "export VITE_A='plain' && export VITE_B='has '\\''quote'\\'' and space' && ");
        let script = build_env_sh_script(&vars);
        assert!(script.contains("export VITE_A='plain'\n"));
        assert!(script.contains("'\\''quote'\\''"));
    }

    #[test]
    fn claude_token_var_renders_when_configured_and_omits_when_absent() {
        // Token configuré → variable plateforme SECRÈTE runtime (ce que reconcile
        // écrit dans le `.env` des apps opt-in). C'est l'invariant sur lequel repose
        // la propagation : sans lui, le token neuf n'atteint jamais l'app.
        let v = claude_token_var(Some("oauth-tok".into())).expect("token → variable");
        assert_eq!(v.key, "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(v.value, "oauth-tok");
        assert!(v.secret && v.platform);
        assert!(v.scope.in_runtime());
        // Aucun token → omission (le caller loggue le warn d'observabilité, et l'UI
        // affiche l'avertissement « claude_access sans token »).
        assert!(claude_token_var(None).is_none());
    }

    #[test]
    fn platform_and_dead_classification() {
        assert!(is_platform_key("PORT"));
        assert!(is_platform_key("HR_DV_TOKEN"));
        assert!(!is_platform_key("MY_VAR"));
        assert!(is_dead_key("HR_FLOW_TOKEN"));
        assert!(is_dead_key("FLOW_RUNS_DIR"));
        assert!(!is_dead_key("PORT"));
        assert!(looks_secret("OPENROUTER_API_KEY"));
        assert!(looks_secret("DB_PASSWORD"));
        assert!(!looks_secret("ENABLE_SCHEDULERS"));
    }
}

/// Parse a `.env` file the same way the supervisor's `load_env_file` does:
/// first `=` splits key/value, surrounding quotes trimmed, `#`/blank skipped.
/// Missing file → empty.
async fn read_env_file(path: &Path) -> Vec<(String, String)> {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim().to_string();
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !k.is_empty() {
                out.push((k, v));
            }
        }
    }
    out
}
