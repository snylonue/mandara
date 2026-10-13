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
        Self::internal(e)
    }
}

impl From<diesel_async::pooled_connection::PoolError> for ApiError {
    fn from(e: diesel_async::pooled_connection::PoolError) -> Self {
        Self::internal(e)
    }
}

impl From<diesel_async::pooled_connection::deadpool::PoolError> for ApiError {
    fn from(e: diesel_async::pooled_connection::deadpool::PoolError) -> Self {
        Self::internal(e)
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::internal(e)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::internal(e)
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound(m) => ApiError::NotFound(m),
            CoreError::InvalidArgument(m) => ApiError::BadRequest(m),
            e @ CoreError::UnsupportedFormat => Self::invalid_input(e),
            e @ CoreError::Plugin(_) => Self::internal(e),
            CoreError::Io(e) => Self::internal(e),
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
        Self::internal(e)
    }
}
impl From<mandara_plugin::error::BookEntryError> for ApiError {
    fn from(e: mandara_plugin::error::BookEntryError) -> Self {
        Self::invalid_input(e)
    }
}

impl From<crate::auth::AuthError> for ApiError {
    fn from(e: crate::auth::AuthError) -> Self {
        Self::internal(e)
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
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn errors_keep_status_and_structured_body() {
        let cases = [
            (
                ApiError::BadRequest("invalid".into()),
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error":"invalid"}),
            ),
            (
                ApiError::NotFound("chapter".into()),
                StatusCode::NOT_FOUND,
                serde_json::json!({"error":"chapter"}),
            ),
            (
                ApiError::ConflictWithFiles(vec!["file".into()]),
                StatusCode::CONFLICT,
                serde_json::json!({"error":"still has 1 file(s); delete the files first","files":["file"]}),
            ),
            (
                ApiError::invalid_input(anyhow::anyhow!("bad input")),
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error":"bad input"}),
            ),
            (
                ApiError::ConfigErrors(mandara_plugin::ConfigErrors(vec![])),
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error":"invalid plugin configuration","errors":[]}),
            ),
            (
                ApiError::internal(anyhow::anyhow!("private details")),
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error":"internal server error"}),
            ),
        ];
        for (error, status, body) in cases {
            let response = error.into_response();
            assert_eq!(response.status(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                body
            );
        }
    }

    #[test]
    fn diagnostic_errors_keep_their_original_cause() {
        let error = ApiError::internal(anyhow::anyhow!("root cause").context("cache write"));
        assert_eq!(
            std::error::Error::source(&error).unwrap().to_string(),
            "cache write"
        );
        assert!(error.to_string().contains("root cause"));
    }
}
