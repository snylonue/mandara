//! Errors from parsing supported book formats.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported format")]
    UnsupportedFormat,
    #[error("invalid epub container: {0}")]
    Container(#[from] zip::result::ZipError),
    #[error("invalid epub document: {0}")]
    Epub(#[from] epub::doc::DocError),
    #[error("invalid utf-8 content: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("epub has no readable chapters")]
    EmptySpine,
}

pub type Result<T> = std::result::Result<T, Error>;
