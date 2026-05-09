use thiserror::Error;

pub type Result<T> = std::result::Result<T, DataverseError>;

#[derive(Debug, Error)]
pub enum DataverseError {
    #[error("validation error: {0}")]
    Validation(#[from] crate::validation::ValidationError),

    #[error("sqlx error: {0}")]
    Sqlx(#[from] crate::sqlx::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("table '{0}' not found")]
    TableNotFound(String),

    #[error("column '{column}' not found in table '{table}'")]
    ColumnNotFound { table: String, column: String },

    #[error("relation between '{from_table}.{from_column}' and '{to_table}.{to_column}' is invalid: {reason}")]
    InvalidRelation {
        from_table: String,
        from_column: String,
        to_table: String,
        to_column: String,
        reason: String,
    },

    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    #[error("provisioning failed for app '{slug}': {reason}")]
    Provisioning { slug: String, reason: String },

    #[error("dataverse not provisioned for app '{0}' (no DB or _dv_meta missing)")]
    NotProvisioned(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl DataverseError {
    pub fn provisioning(slug: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Provisioning { slug: slug.into(), reason: reason.into() }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}
