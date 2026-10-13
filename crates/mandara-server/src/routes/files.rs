//! File endpoints: file detail (+ chapters), chapter content, visibility
//! toggling, deletion.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use mandara_core::model::{BookMeta, ChapterMeta, FileMeta, TocNode, Visibility};

use crate::error::ApiError;
use crate::extractors::AuthenticatedUser;
use crate::routes::{St, can_manage_file, load_visible_file};

#[derive(Serialize)]
pub struct FileDetail {
    pub file: FileMeta,
    pub book: BookMeta,
    pub chapters: Vec<ChapterMeta>,
    pub toc: Vec<TocNode>,
}

// GET /api/files/{id} ---------------------------------------------------------

pub async fn get_file(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let file = load_visible_file(&st, &user, &id).await?;
    st.library.ensure_titles(&file).await?;
    let chapters = st.library.chapter_titles(&file.id).await?;
    let book = st
        .library
        .get_book(&file.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    let toc = st.library.file_toc(&file.id).await?;
    Ok(Json(FileDetail {
        file,
        book,
        chapters,
        toc,
    }))
}

// GET /api/files/{id}/chapters/{idx} -------------------------------------------

#[derive(Deserialize)]
pub struct ChapterPath {
    pub id: String,
    pub idx: u32,
}

pub async fn get_chapter(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(path): Path<ChapterPath>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let file = load_visible_file(&st, &user, &path.id).await?;
    Ok(Json(st.library.get_chapter(&file, path.idx).await?))
}

// GET /api/files/{id}/download ------------------------------------------------
//
// Streams the retained original bytes (upload or plugin file-mode pull).
// Authorization is identical to chapter reads (owner/admin/public).
// Files without a retained original (plugin chapter-mode, pre-retention
// uploads) answer 404.
pub async fn download_file(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let file = load_visible_file(&st, &user, &id).await?;
    let (bytes, ext) = st
        .library
        .get_original(&file)
        .await?
        .ok_or_else(|| ApiError::not_found("original"))?;
    let mime = match ext.as_str() {
        "epub" => "application/epub+zip",
        _ => "text/plain; charset=utf-8",
    };
    // Filename: the book title (sanitized), falling back to the file id.
    // Non-ASCII titles cannot round-trip through the ASCII `filename=`
    // parameter, so it is percent-encoded in `filename*` (RFC 5987) and
    // the ASCII fallback keeps only the extension-safe part.
    let book = st
        .library
        .get_book(&file.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    let title = book.title.trim().trim_matches('_');
    let safe_title: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let ascii_name = if safe_title.is_empty() {
        format!("{}.{}", &file.id[..8.min(file.id.len())], ext)
    } else {
        format!("{}.{}", safe_title, ext)
    };
    // RFC 5987: `filename*=UTF-8''<percent-encoded>` carries the real
    // (possibly non-ASCII) name; browsers use it when present.
    let mut encoded = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
            encoded.push(c);
        } else {
            let mut b = [0u8; 4];
            for &x in c.encode_utf8(&mut b).as_bytes() {
                encoded.push_str(&format!("%{x:02X}"));
            }
        }
    }
    let star_name = format!("{}.{}", encoded, ext);
    let disposition =
        format!("attachment; filename=\"{ascii_name}\"; filename*=UTF-8''{star_name}");
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    ))
}

// PATCH /api/files/{id} -------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchFile {
    /// `public` makes the file visible to every logged-in user, `private`
    /// hides it (owner + admins only).
    pub visibility: Option<Visibility>,
    pub label: Option<String>,
}

pub async fn patch_file(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<PatchFile>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let file = load_visible_file(&st, &user, &id).await?;
    if !can_manage_file(&user, &file) {
        return Err(ApiError::Forbidden);
    }
    let updated = st
        .library
        .update_file(&id, req.visibility, req.label.as_deref())
        .await?;
    Ok(Json(updated))
}

// POST /api/files/{id}/rematerialize -------------------------------------------
//
// Clear stored chapter bodies and queue background downloads
// from its content source. The retry path for degraded
// materializations (e.g. images that fell back to remote URLs because the
// download failed — the re-pull stores them via `store-image`).

pub async fn rematerialize_file(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let file = load_visible_file(&st, &user, &id).await?;
    if !can_manage_file(&user, &file) {
        return Err(ApiError::Forbidden);
    }
    let updated = st.library.rematerialize_file(&file).await?;
    Ok(Json(updated))
}

// DELETE /api/files/{id} -------------------------------------------------------

pub async fn delete_file(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let file = load_visible_file(&st, &user, &id).await?;
    if !can_manage_file(&user, &file) {
        return Err(ApiError::Forbidden);
    }
    st.library.delete_file(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
