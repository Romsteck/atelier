use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Technology stack for an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppStack {
    NextJs,
    AxumVite,
    Axum,
    Flutter,
}

impl AppStack {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::NextJs => "Next.js",
            Self::AxumVite => "Vite+Rust",
            Self::Axum => "Rust Only",
            Self::Flutter => "Flutter",
        }
    }

    pub fn default_health_path(&self) -> &'static str {
        "/health"
    }
}

/// Whether an app is reachable without authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    #[default]
    Private,
}

/// Runtime state of an app process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
    Unknown,
}

impl AppState {
    /// Lowercase wire form, matching the serde representation. Used to mirror
    /// the state into the `applications.state` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Crashed => "crashed",
            Self::Unknown => "unknown",
        }
    }
}

/// Managed-DB engine for an app.
///
/// Only `PostgresDataverse` exists post-migration: every app with a
/// database lives in `app_{slug}` (Postgres) and consumes
/// `DATABASE_URL` injected at runtime. The legacy SQLite + transitional
/// "data-migrated" states were removed once all apps were converted.
///
/// Apps with `has_db = false` carry this field anyway (it's the
/// default), but the runtime ignores the value — no PG database is
/// provisioned and no `DATABASE_URL` is injected for them.
///
/// Forward-compat: `#[serde(other)]` accepts the obsolete legacy
/// values (`legacy-sqlite`, `data-migrated`) and silently maps them to
/// `PostgresDataverse` so old `apps.json` files keep deserialising.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DbBackend {
    /// `atelier-dataverse` Postgres engine, dedicated database `app_{slug}`.
    /// The app's binary uses `DATABASE_URL`.
    #[default]
    #[serde(other)]
    PostgresDataverse,
}

impl DbBackend {
    /// Whether the app should receive `DATABASE_URL` in its runtime
    /// env. Always true now that Postgres is the only backend, gated
    /// by `Application::has_db`.
    pub fn injects_database_url(&self) -> bool {
        true
    }
}

pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug.as_bytes()[0].is_ascii_lowercase()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.ends_with('-')
}

/// Validate an environment-variable name: must start with a letter or `_`
/// and contain only letters, digits and `_` (POSIX `name` + the common
/// `VITE_*` / `NEXT_PUBLIC_*` conventions). Rejects empty, spaces, `=`, etc.
pub fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Where a variable must be injected. Runtime → the supervised process env
/// (works identically for Node `process.env` and Rust `std::env`). Build →
/// exported before the build command (needed for framework-baked public vars
/// like `VITE_*` / `NEXT_PUBLIC_*`). Both → exported at build AND runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EnvScope {
    #[default]
    Runtime,
    Build,
    Both,
}

impl EnvScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Build => "build",
            Self::Both => "both",
        }
    }
    /// Injected into the running process env.
    pub fn in_runtime(&self) -> bool {
        matches!(self, Self::Runtime | Self::Both)
    }
    /// Exported into the build command env.
    pub fn in_build(&self) -> bool {
        matches!(self, Self::Build | Self::Both)
    }
}

/// A single user-owned environment variable, the canonical store for app
/// config. Platform-managed vars (`PORT`, `HR_DV_*`, `ATELIER_*`) are NOT
/// stored here — they are recomputed at render time (see
/// `atelier-api`'s env reconciliation).
///
/// `value` is the literal value (config or secret). The `secret` flag drives
/// UI masking + per-row reveal only — the value is stored as-is in JSONB, the
/// same plaintext exposure as `dataverse-secrets.json`. The supervisor never
/// reads this field; it consumes the rendered `.env` written by the reconciler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub scope: EnvScope,
}

/// An application managed by Atelier, running directly on the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Application {
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub stack: AppStack,
    #[serde(default)]
    pub has_db: bool,
    #[serde(default)]
    pub visibility: Visibility,
    pub domain: String,
    pub port: u16,
    pub run_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    /// Override the artefact path(s) to rsync back after a remote build.
    /// If None, defaults are derived from the stack (see `app.build` docs).
    /// Paths are relative to `src/`. Multiple paths separated by newline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_artefact: Option<String>,
    pub health_path: String,
    /// Legacy flat user-env map. Superseded by `env` (structured, ownership-
    /// aware, with secret/scope metadata). Kept `#[serde(default)]` so old
    /// rows keep deserialising; the env reconciler folds any residual entries
    /// into `env` and clears this on first write. No longer injected by the
    /// supervisor.
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,
    /// Canonical user-owned environment variables (config + secrets). Platform
    /// vars are recomputed, never stored here. See [`EnvVar`].
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub state: AppState,
    /// Which managed-DB engine this app uses. Existing apps default to
    /// `LegacySqlite`; new apps are created with `PostgresDataverse`
    /// (controlled by the registry's create flow).
    #[serde(default)]
    pub db_backend: DbBackend,
    /// Legacy flow-callback URL — kept as `#[serde(default)]` to tolerate
    /// existing `apps.json` entries that still carry the field after the
    /// flow-system eradication (2026-05-26). No longer read or written.
    #[serde(default, skip_serializing)]
    pub flow_callback_url: Option<String>,
    /// Legacy flow-callback bearer token — see `flow_callback_url`.
    #[serde(default, skip_serializing)]
    pub flow_callback_token: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Application {
    /// Create a new application with sensible defaults for the given stack.
    pub fn new(slug: String, name: String, stack: AppStack) -> Self {
        let now = Utc::now();
        let domain = format!("{}.mynetwk.biz", slug);
        let health_path = stack.default_health_path().to_string();
        Self {
            slug,
            name,
            description: None,
            stack,
            has_db: false,
            visibility: Visibility::Private,
            domain,
            port: 0,
            run_command: String::new(),
            build_command: None,
            build_artefact: None,
            health_path,
            env_vars: BTreeMap::new(),
            env: Vec::new(),
            state: AppState::Stopped,
            db_backend: DbBackend::default(),
            flow_callback_url: None,
            flow_callback_token: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Root directory for this app's source, build artifacts and DB.
    /// Resolved from `ATELIER_APPS_RUNTIME_ROOT` (default `/var/lib/atelier/apps`).
    pub fn app_dir(&self) -> PathBuf {
        let root = std::env::var("ATELIER_APPS_RUNTIME_ROOT")
            .unwrap_or_else(|_| "/var/lib/atelier/apps".to_string());
        PathBuf::from(root).join(&self.slug)
    }

    /// Path to the runtime `.env` file.
    pub fn env_file(&self) -> PathBuf {
        self.app_dir().join(".env")
    }

    /// Path to the source tree for this app **and the Studio workspace
    /// root** — c'est là que l'agent Claude Code lit `CLAUDE.md`, `.claude/`,
    /// `.mcp.json`. Tout fichier de contexte destiné à l'agent DOIT être
    /// écrit sous ce chemin, jamais sous `app_dir()` directement.
    ///
    /// Voir l'INVARIANT documenté en tête de [`crate::context`] et la rule
    /// `.claude/rules/apps-workspace-layout.md` du repo Atelier.
    pub fn src_dir(&self) -> PathBuf {
        self.app_dir().join("src")
    }

    /// Find a user env var by key.
    pub fn env_get(&self, key: &str) -> Option<&EnvVar> {
        self.env.iter().find(|e| e.key == key)
    }

    /// Insert or replace a user env var (matched by key), keeping the list
    /// sorted by key for deterministic rendering/serialisation.
    pub fn env_set(&mut self, var: EnvVar) {
        match self.env.iter_mut().find(|e| e.key == var.key) {
            Some(existing) => *existing = var,
            None => self.env.push(var),
        }
        self.env.sort_by(|a, b| a.key.cmp(&b.key));
    }

    /// Remove a user env var by key. Returns true if one was removed.
    pub fn env_remove(&mut self, key: &str) -> bool {
        let before = self.env.len();
        self.env.retain(|e| e.key != key);
        self.env.len() != before
    }
}
