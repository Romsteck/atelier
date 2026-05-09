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

/// Where the canonical `src/` tree of this app lives. Determines whether the
/// `app.build` pipeline must rsync sources up to CloudMaster or whether the
/// sources already live there (post wave-2 migration).
///
/// The default is `CloudMaster`: nouvelles apps sont scaffoldées directement
/// sur CloudMaster (workspace agent + build remote sans rsync UP). Les apps
/// existantes (legacy) ont leur valeur explicite dans `apps.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourcesLocation {
    /// Sources live on Medion under `/opt/homeroute/apps/{slug}/src/` (legacy
    /// pre-migration layout). Build must rsync them up to CloudMaster first.
    Medion,
    /// Sources live on CloudMaster under `/opt/homeroute/apps/{slug}/src/`.
    /// Build skips the rsync-up step — only the artefact is rsynced back down.
    #[default]
    CloudMaster,
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
    /// `hr-dataverse` Postgres engine, dedicated database `app_{slug}`.
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

/// An application managed by HomeRoute, running directly on the host.
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
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,
    #[serde(default)]
    pub state: AppState,
    /// Where the canonical sources live. Defaults to `Medion` for back-compat
    /// with the existing `apps.json` (no field present → Medion).
    #[serde(default)]
    pub sources_on: SourcesLocation,
    /// Which managed-DB engine this app uses. Existing apps default to
    /// `LegacySqlite`; new apps are created with `PostgresDataverse`
    /// (controlled by the registry's create flow).
    #[serde(default)]
    pub db_backend: DbBackend,
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
            state: AppState::Stopped,
            sources_on: SourcesLocation::default(),
            db_backend: DbBackend::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Root directory for this app's source, build artifacts and DB.
    pub fn app_dir(&self) -> PathBuf {
        PathBuf::from(format!("/opt/homeroute/apps/{}", self.slug))
    }

    /// Path to the managed SQLite database for this app.
    pub fn db_path(&self) -> PathBuf {
        self.app_dir().join("db.sqlite")
    }

    /// Path to the runtime `.env` file.
    pub fn env_file(&self) -> PathBuf {
        self.app_dir().join(".env")
    }

    /// Path to the source tree for this app **and the code-server workspace
    /// root** — c'est là que l'agent Claude Code lit `CLAUDE.md`, `.claude/`,
    /// `.mcp.json`. Tout fichier de contexte destiné à l'agent DOIT être
    /// écrit sous ce chemin, jamais sous `app_dir()` directement.
    ///
    /// Voir l'INVARIANT documenté en tête de [`crate::context`] et la rule
    /// `.claude/rules/apps-workspace-layout.md` du repo HomeRoute.
    pub fn src_dir(&self) -> PathBuf {
        self.app_dir().join("src")
    }
}
