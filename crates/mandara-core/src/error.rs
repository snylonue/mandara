use thiserror::Error;

/// Errors produced by core functionality (formatting, plugins, ...).
#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("unsupported format")]
    UnsupportedFormat,

    #[error("plugin error: {0}")]
    Plugin(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
