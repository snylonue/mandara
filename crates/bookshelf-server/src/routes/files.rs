//! File endpoints: file detail (+ chapters), chapter content, visibility
//! toggling, deletion.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{BookMeta, ChapterMeta, FileMeta, TocNode, Visibility};

use crate::error::ApiError;
use crate::routes::{can_manage_file, current_user, load_visible_file, St};

#[derive(Serialize)]
pub struct FileDetail {
    pub file: FileMeta,
    pub book: BookMeta,
    pub chapters: Vec<ChapterMeta>,
    pub toc: Vec<TocNode>,
}

// GET /api/files/{id} ---------------------------------------------------------

pub async fn get_file(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
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
    State(st): State<St>,
    headers: HeaderMap,
    Path(path): Path<ChapterPath>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let file = load_visible_file(&st, &user, &path.id).await?;
    match st.library.get_chapter(&file, path.idx).await? {
        Some(chapter) => Ok(Json(chapter)),
        None => Err(ApiError::not_found("chapter")),
    }
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
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PatchFile>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
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

// DELETE /api/files/{id} -------------------------------------------------------

pub async fn delete_file(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let file = load_visible_file(&st, &user, &id).await?;
    if !can_manage_file(&user, &file) {
        return Err(ApiError::Forbidden);
    }
    st.library.delete_file(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
