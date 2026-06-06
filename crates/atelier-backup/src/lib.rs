//! Sauvegarde Atelier vers un serveur Samba via **restic + rclone** (sous-process).
//!
//! Sauvegarde **incrémentale / dédupliquée / chiffrée** de trois ensembles, en 3
//! snapshots restic taggés `git` / `postgres` / `config` :
//!   - GIT    : le dossier des dépôts bare (`/var/lib/atelier/git/`)
//!   - POSTGRES : `pg_dumpall` (superuser) streamé dans `restic backup --stdin`
//!   - CONFIG : `.env`, registres, secrets dataverse, `.env` par app, docs
//!
//! La destination Samba est atteinte par le backend `rclone` de restic (pas de
//! mount root). Périmètre = SAUVEGARDE UNIQUEMENT (aucune restauration). Le
//! service est **inert** (mode noop) tant que Postgres (`atelier_meta`) est
//! injoignable ou que les binaires `restic`/`rclone` manquent — un run renvoie
//! alors une erreur propre. Pas de scheduler actif par défaut (`schedule_enabled`
//! = false), l'infra de planification est présente mais désactivée.

#[allow(unused_imports)]
pub(crate) mod sqlx {
    pub use sqlx_core::Error;
    pub use sqlx_core::executor::Executor;
    pub use sqlx_core::pool::Pool;
    pub use sqlx_core::query::query;
    pub use sqlx_core::query_as::query_as;
    pub use sqlx_core::query_scalar::query_scalar;
    pub use sqlx_core::raw_sql::raw_sql;
    pub use sqlx_core::row::Row;
    pub use sqlx_core::sql_str::AssertSqlSafe;
    pub use sqlx_postgres::{PgPool, PgPoolOptions, PgRow, Postgres};
}

pub mod migration;
pub mod models;
pub mod pgdump;
pub mod rclone;
pub mod restic;
pub mod runs;
pub mod scheduler;
pub mod service;
pub mod sources;
pub mod target;

pub use models::{BackupEvent, EventStatus, Phase, PhaseDetail, RepoStats, SnapshotResult, ToolStatus};
pub use runs::{BackupRun, RunSnapshot, RunsStore};
pub use service::{BackupService, BackupServiceConfig, BackupStatus};
pub use sources::SourcePaths;
pub use target::{BackupTarget, NewTarget, TargetStore};
