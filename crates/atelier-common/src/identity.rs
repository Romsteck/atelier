//! Caller identity, threaded through every dataverse mutation/read so the
//! gateway can populate `created_by` / `updated_by` and (later) enforce
//! row-level rights.
//!
//! Three actor kinds:
//! - **`User`** — a human identified via the hr-auth session cookie. The
//!   reverse proxy injects `X-Remote-User-Id` (UUIDv4 from `users.yml`).
//! - **`App`** — an application calling the gateway with its per-app
//!   bearer token (`Authorization: Bearer …`). The gateway looks the
//!   token up in `dataverse-secrets.json` and resolves it to the app's
//!   `app_uuid` + slug.
//! - **`System`** — internal background jobs, migrations, healthchecks.
//!   Stored as `created_by_kind = 'system'` with `created_by = NULL`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Identity {
    User {
        uuid: Uuid,
        username: String,
    },
    App {
        uuid: Uuid,
        slug: String,
    },
    System,
}

impl Identity {
    pub fn system() -> Self {
        Identity::System
    }

    pub fn user(uuid: Uuid, username: impl Into<String>) -> Self {
        Identity::User {
            uuid,
            username: username.into(),
        }
    }

    pub fn app(uuid: Uuid, slug: impl Into<String>) -> Self {
        Identity::App {
            uuid,
            slug: slug.into(),
        }
    }

    /// The actor uuid stored on `created_by` / `updated_by`. `None` for
    /// `System` (which writes a NULL uuid + kind='system').
    pub fn actor_uuid(&self) -> Option<Uuid> {
        match self {
            Identity::User { uuid, .. } | Identity::App { uuid, .. } => Some(*uuid),
            Identity::System => None,
        }
    }

    /// The string discriminant stored on `created_by_kind` /
    /// `updated_by_kind` (matches the CHECK constraint values).
    pub fn kind_str(&self) -> &'static str {
        match self {
            Identity::User { .. } => "user",
            Identity::App { .. } => "app",
            Identity::System => "system",
        }
    }

    /// A human-readable label for audit logs (`actor_label` column).
    pub fn label(&self) -> String {
        match self {
            Identity::User { username, .. } => format!("user:{}", username),
            Identity::App { slug, .. } => format!("app:{}", slug),
            Identity::System => "system".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_identity_round_trips() {
        let u = Uuid::new_v4();
        let id = Identity::user(u, "alice");
        let json = serde_json::to_string(&id).unwrap();
        let back: Identity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        assert_eq!(id.actor_uuid(), Some(u));
        assert_eq!(id.kind_str(), "user");
        assert_eq!(id.label(), "user:alice");
    }

    #[test]
    fn app_identity_round_trips() {
        let u = Uuid::new_v4();
        let id = Identity::app(u, "trader");
        let json = serde_json::to_string(&id).unwrap();
        let back: Identity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        assert_eq!(id.kind_str(), "app");
        assert_eq!(id.label(), "app:trader");
    }

    #[test]
    fn system_identity_no_uuid() {
        let id = Identity::system();
        assert_eq!(id.actor_uuid(), None);
        assert_eq!(id.kind_str(), "system");
    }
}
