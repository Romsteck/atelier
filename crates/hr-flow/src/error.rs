//! Error types for the flow engine.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlowError {
    #[error("flow `{0}` not found")]
    FlowNotFound(String),

    #[error("step `{step_id}` failed: {message}")]
    StepFailed { step_id: String, message: String },

    #[error("connector `{0}` not registered")]
    UnknownConnector(String),

    #[error("operation `{op}` not supported on connector `{connector}`")]
    UnknownOperation { connector: String, op: String },

    #[error("custom action `{0}` not registered")]
    UnknownAction(String),

    #[error("expression error: {0}")]
    Expression(String),

    #[error("invalid flow definition: {0}")]
    InvalidDefinition(String),

    #[error("persistence error: {0}")]
    Persistence(String),

    #[error("connector error: {0}")]
    Connector(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type FlowResult<T> = Result<T, FlowError>;
