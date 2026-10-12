//! API error type rendered as JSON.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use mandara_core::error::Error as CoreError;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    /// Invalid client data with its original validation/parser error.
    InvalidInput(anyhow::Error),
    Unauthorized,
    Forbidden,
    NotFound(String),
    Conflict(String),
    /// Deleting a metadata entry / plugin instance while it still owns
    /// files. Carries the file ids so clients can delete them explicitly
    /// first (metadata and file deletion are deliberately distinct
    /// operations).
    ConflictWithFiles(Vec<String>),
    /// Structured configuration validation errors (field-by-field, for
    /// inline admin-form display).
    ConfigErrors(mandara_plugin::ConfigErrors),
    Internal(anyhow::Error),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError::BadRequest(msg.into())
    }
    pub fn invalid_input(error: impl Into<anyhow::Error>) -> Self {
        ApiError::InvalidInput(error.into())
    }
    pub fn internal(error: impl Into<anyhow::Error>) -> Self {
        Self::Internal(error.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError::NotFound(msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::InvalidInput(e) => (StatusCode::BAD_REQUEST, e.to_string()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".into()),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::ConflictWithFiles(files) => {
                let msg = format!("still has {} file(s); delete the files first", files.len());
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": msg,
                        "files": files,
                    })),
                )
                    .into_response();
            }
            ApiError::ConfigErrors(errors) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "invalid plugin configuration",
                        "errors": errors.0,
                    })),
                )
                    .into_response();
            }
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".into(),
                )
            }
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::BadRequest(m) => write!(f, "bad request: {m}"),
            ApiError::InvalidInput(e) => write!(f, "bad request: {e}"),
            ApiError::Unauthorized => write!(f, "unauthorized"),
            ApiError::Forbidden => write!(f, "forbidden"),
            ApiError::NotFound(m) => write!(f, "not found: {m}"),
            ApiError::Conflict(m) => write!(f, "conflict: {m}"),
            ApiError::ConflictWithFiles(files) => {
                write!(f, "conflict: still has {} file(s)", files.len())
            }
            ApiError::ConfigErrors(errors) => write!(f, "invalid plugin configuration: {errors}"),
            ApiError::Internal(e) => write!(f, "internal error: {e:#}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ApiError::Internal(e) | ApiError::InvalidInput(e) => Some(e.as_ref()),
            ApiError::ConfigErrors(e) => Some(e),
            _ => None,
        }
    }
}

impl From<diesel::result::Error> for ApiError {
    fn from(e: diesel::result::Error) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

impl From<diesel_async::pooled_connection::PoolError> for ApiError {
    fn from(e: diesel_async::pooled_connection::PoolError) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

impl From<diesel_async::pooled_connection::deadpool::PoolError> for ApiError {
    fn from(e: diesel_async::pooled_connection::deadpool::PoolError) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Internal(e.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound(m) => ApiError::NotFound(m),
            CoreError::InvalidArgument(m) => ApiError::BadRequest(m),
            e @ CoreError::UnsupportedFormat => Self::invalid_input(e),
            e @ CoreError::Plugin(_) => ApiError::Internal(e.into()),
            CoreError::Io(e) => ApiError::Internal(anyhow::Error::new(e)),
        }
    }
}

impl From<mandara_core::ext::MetadataError> for ApiError {
    fn from(e: mandara_core::ext::MetadataError) -> Self {
        Self::invalid_input(e)
    }
}
impl From<mandara_core::model::ModelError> for ApiError {
    fn from(e: mandara_core::model::ModelError) -> Self {
        Self::invalid_input(e)
    }
}
impl From<mandara_formats::error::Error> for ApiError {
    fn from(e: mandara_formats::error::Error) -> Self {
        Self::invalid_input(e)
    }
}
impl From<mandara_plugin::error::Error> for ApiError {
    fn from(e: mandara_plugin::error::Error) -> Self {
        Self::Internal(e.into())
    }
}
impl From<mandara_plugin::error::BookEntryError> for ApiError {
    fn from(e: mandara_plugin::error::BookEntryError) -> Self {
        Self::invalid_input(e)
    }
}

impl From<crate::auth::AuthError> for ApiError {
    fn from(e: crate::auth::AuthError) -> Self {
        Self::Internal(e.into())
    }
}
impl From<crate::routes::auth::UsernameError> for ApiError {
    fn from(e: crate::routes::auth::UsernameError) -> Self {
        Self::invalid_input(e)
    }
}
impl From<axum::extract::multipart::MultipartError> for ApiError {
    fn from(e: axum::extract::multipart::MultipartError) -> Self {
        crate::routes::books::UploadError::from(e).into()
    }
}

impl From<crate::routes::books::UploadError> for ApiError {
    fn from(e: crate::routes::books::UploadError) -> Self {
        Self::invalid_input(e)
    }
}
