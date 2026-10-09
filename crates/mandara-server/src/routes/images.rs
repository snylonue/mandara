//! Image endpoint: serves localized chapter images by their sha256 id.
//!
//! Deliberately unauthenticated: chapter HTML embeds plain `<img>` tags,
//! which cannot send the Authorization header — same capability-like
//! model as book covers and share tokens (ids are unguessable sha256
//! hashes of the bytes).

use axum::extract::{FromRequestParts, Path, State};
use axum::http::header;
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use axum::response::IntoResponse;

use mandara_core::model::ImageId;

use crate::error::ApiError;
use crate::routes::St;

/// Typed path extractor for image ids: an id that is not valid sha256 hex
/// can never reach the service layer (it is rejected as unknown, i.e. 404).
/// A wrapper is required because [`ImageId`] lives in `mandara-core`,
/// which knows nothing about axum.
#[derive(Debug, Clone)]
pub struct ImagePath(pub ImageId);

impl std::ops::Deref for ImagePath {
    type Target = ImageId;

    fn deref(&self) -> &ImageId {
        &self.0
    }
}

impl FromRequestParts<St> for ImagePath {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &St) -> Result<Self, Self::Rejection> {
        let Path(raw): Path<String> = Path::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::not_found("image"))?;
        ImageId::from_sha256_hex(raw)
            .map(Self)
            .map_err(|_| ApiError::not_found("image"))
    }
}

/// GET /api/images/{id} — the stored image bytes (public).
pub async fn get_image(
    State(st): State<St>,
    ImagePath(id): ImagePath,
) -> Result<impl IntoResponse, ApiError> {
    let Some(bytes) = st.library.image_bytes(&id).await? else {
        return Err(ApiError::not_found("image"));
    };
    let mime = st
        .library
        .image_mime(&id)
        .await?
        .unwrap_or_else(|| "image/jpeg".into());
    Ok((
        [(CONTENT_TYPE, mime)],
        [
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            // SVG remains a passive image even when its capability URL is
            // opened as a document instead of embedded in the reader.
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:",
            ),
        ],
        bytes,
    ))
}
