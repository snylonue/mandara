use thiserror::Error;

/// Errors produced by core functionality (formatting, plugins, ...).
#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;