//! Host failures are distinct from the WIT transport error representation.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("plugin runtime error: {0:#}")]
    Runtime(#[from] anyhow::Error),
    #[error("invalid plugin book entry: {0}")]
    Entry(#[from] BookEntryError),
    #[error(
        "`{file}` declare catalog too large: {books}/{max_books} books, {chapters}/{max_chapters} chapters (caps)"
    )]
    CatalogLimit {
        file: String,
        books: usize,
        max_books: usize,
        chapters: usize,
        max_chapters: usize,
    },
    #[error("`{file}` {resource} exceeds the {limit} byte cap")]
    ContentLimit {
        file: String,
        resource: &'static str,
        limit: usize,
    },
}

/// Failures from an image storage implementation, independent of its backend.
#[derive(Debug, thiserror::Error)]
pub enum ImageStoreError {
    #[error("image storage backend failed: {0}")]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl From<anyhow::Error> for ImageStoreError {
    fn from(error: anyhow::Error) -> Self {
        Self::Backend(error.into_boxed_dyn_error())
    }
}

/// Invalid metadata advertised by a guest, before it enters the library.
#[derive(Debug, thiserror::Error)]
pub enum BookEntryError {
    #[error("invalid `extra` metadata JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid `extra` metadata object: {0}")]
    Metadata(#[from] mandara_core::ext::MetadataError),
}

impl From<wasmtime::Error> for Error {
    fn from(e: wasmtime::Error) -> Self {
        Self::Runtime(e.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
