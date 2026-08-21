//! API error type rendered as JSON.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use bookshelf_core::error::Error as CoreError;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound(String),
    Conflict(String),
    Internal(anyhow::Error),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        ApiError::BadRequest(msg.into())
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError::NotFound(msg.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".into()),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error".into())
            }
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::BadRequest(m) => write!(f, "bad request: {m}"),
            ApiError::Unauthorized => write!(f, "unauthorized"),
            ApiError::Forbidden => write!(f, "forbidden"),
            ApiError::NotFound(m) => write!(f, "not found: {m}"),
            ApiError::Conflict(m) => write!(f, "conflict: {m}"),
            ApiError::Internal(e) => write!(f, "internal error: {e:#}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ApiError::Internal(e) => e.source(),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Internal(anyhow::Error::new(e))
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
            CoreError::UnsupportedFormat(m) => {
                ApiError::BadRequest(format!("unsupported format: {m}"))
            }
            CoreError::Plugin(m) => ApiError::Internal(anyhow::anyhow!(m)),
            CoreError::Io(e) => ApiError::Internal(anyhow::Error::new(e)),
        }
    }
}