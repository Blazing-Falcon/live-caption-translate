use crate::events::FailReason;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Engine(String),
    #[error("Unknown {kind} engine: {name}")]
    UnknownEngine { kind: &'static str, name: String },
    #[error("{message}")]
    Translation { reason: FailReason, message: String },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("Pipeline has stopped")]
    Stopped,
}

pub type Result<T> = std::result::Result<T, Error>;
