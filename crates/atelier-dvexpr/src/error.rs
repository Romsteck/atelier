use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("lex error at offset {offset}: {message}")]
    Lex { offset: usize, message: String },

    #[error("parse error at offset {offset}: {message}")]
    Parse { offset: usize, message: String },

    #[error("type error: {0}")]
    Type(String),

    #[error("eval error: {0}")]
    Eval(String),

    #[error("unknown identifier: {0}")]
    UnknownIdent(String),

    #[error("unknown function: {0}")]
    UnknownFunction(String),

    #[error("arity mismatch for {name}: expected {expected}, got {got}")]
    Arity {
        name: String,
        expected: String,
        got: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
