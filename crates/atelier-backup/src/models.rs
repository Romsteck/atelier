use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Phase d'un run, dans l'ordre d'exécution. Sérialisée en minuscule pour le
/// frontend (`backup:live` → `data.phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Vérification/initialisation du dépôt restic.
    Repo,
    Git,
    Postgres,
    Config,
    /// `restic forget --prune` (rétention).
    Prune,
    /// Run terminé avec succès.
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Repo => "repo",
            Phase::Git => "git",
            Phase::Postgres => "postgres",
            Phase::Config => "config",
            Phase::Prune => "prune",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventStatus {
    Running,
    Success,
    Failed,
    Cancelled,
}

/// Détail de phase pendant le streaming (octets/fichiers d'un snapshot en cours).
#[derive(Debug, Clone, Default, Serialize)]
pub struct PhaseDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_done: Option<u64>,
    /// `None` pour un dump streamé (taille inconnue → pas de pourcentage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    /// Tag concerné : "git" | "postgres" | "config".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// Événement live diffusé sur le WebSocket (`{ "type":"backup:live", "data":<this> }`).
#[derive(Debug, Clone, Serialize)]
pub struct BackupEvent {
    pub run_id: Uuid,
    pub phase: Phase,
    pub status: EventStatus,
    /// Message lisible (FR), ex. « Archivage du dépôt git… ».
    pub message: String,
    /// Progression globale 0..=100 (pondérée par phase).
    pub progress: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<PhaseDetail>,
    pub at: DateTime<Utc>,
}

/// Résultat parsé du `{"message_type":"summary",...}` d'un `restic backup`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SnapshotResult {
    pub snapshot_id: Option<String>,
    pub files: i64,
    pub bytes_processed: i64,
    pub bytes_added: i64,
}

/// Statistiques du dépôt restic (pour le bandeau d'aperçu). Mis en cache côté
/// service (appel sur SMB potentiellement lent).
#[derive(Debug, Clone, Default, Serialize)]
pub struct RepoStats {
    pub total_size_bytes: i64,
    pub snapshot_count: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ToolStatus {
    pub restic: bool,
    pub rclone: bool,
}
