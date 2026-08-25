//! Image endpoint: serves localized chapter images by their sha256 id.
//!
//! Deliberately unauthenticated: chapter HTML embeds plain `<img>` tags,
//! which cannot send the Authorization header — same capability-like
//! model as book covers and share tokens (ids are unguessable sha256
//! hashes of the bytes).

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::IntoResponse;

use crate::error::ApiError;
use crate::routes::St;

/// GET /api/images/{id} — the stored image bytes (public).
pub async fn get_image(
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // The id doubles as the filename — only sha256 hex is accepted, so
    // nothing outside data/files/images/ is reachable.
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("image"));
    }
    let Some(bytes) = st.library.image_bytes(&id).await? else {
        return Err(ApiError::not_found("image"));
    };
    let mime = st
        .library
        .image_mime(&id)
        .await?
        .unwrap_or_else(|| "image/jpeg".into());
    Ok((
        [(header::CONTENT_TYPE, mime)],
        [(header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
        bytes,
    ))
}
