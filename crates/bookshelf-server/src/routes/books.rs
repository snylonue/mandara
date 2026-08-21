//! Book endpoints: catalog, upload, detail, chapters.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{BookMeta, Chapter, ChapterMeta, Visibility};

use crate::error::ApiError;
use crate::routes::{
    can_manage, current_user, load_visible_book, ListParams, St,
};

#[derive(Serialize)]
pub struct BookDetail {
    pub book: BookMeta,
    pub chapters: Vec<ChapterMeta>,
}

// GET /api/books -----------------------------------------------------------

pub async fn list_books(
    State(st): State<St>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let books = st
        .library
        .list_books(params.q.as_deref(), params.source.as_deref(), &user)
        .await?;
    Ok(Json(books))
}

// POST /api/books (multipart file upload) ----------------------------------

pub async fn upload_book(
    State(st): State<St>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let mut bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("multipart: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        filename = field.file_name().map(|s| s.to_string());
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::bad_request(format!("upload read: {e}")))?
        {
            buf.extend_from_slice(&chunk);
            if buf.len() > st.cfg.max_upload_mb as usize * 1024 * 1024 {
                return Err(ApiError::bad_request("upload too large"));
            }
        }
        bytes = Some(buf);
    }

    let bytes = bytes.ok_or_else(|| ApiError::bad_request("missing `file` field"))?;
    let filename = filename.unwrap_or_else(|| "book.unknown".into());
    let book = st.library.ingest_upload(&user.id, &filename, &bytes).await?;
    Ok((StatusCode::CREATED, Json(book)))
}

// GET /api/books/:id --------------------------------------------------------

pub async fn get_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = load_visible_book(&st, &user, &id).await?;
    st.library.ensure_titles(&book).await?;
    let chapters = st.library.chapter_titles(&book.id).await?;
    Ok(Json(BookDetail { book, chapters }))
}

// GET /api/books/:id/chapters/:idx ------------------------------------------

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
    let book = load_visible_book(&st, &user, &path.id).await?;
    let chapter: Option<Chapter> = st.library.get_chapter(&book, path.idx).await?;
    match chapter {
        Some(chapter) => Ok(Json(chapter)),
        None => Err(ApiError::not_found("chapter")),
    }
}

// PATCH /api/books/:id ------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchBook {
    pub title: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<Visibility>,
}

pub async fn patch_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PatchBook>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = load_visible_book(&st, &user, &id).await?;
    if !can_manage(&user, &book) {
        return Err(ApiError::Forbidden);
    }
    let updated = st
        .library
        .update_book(
            &id,
            req.title.as_deref(),
            req.description.as_deref(),
            req.visibility,
        )
        .await?;
    Ok(Json(updated))
}

// DELETE /api/books/:id -----------------------------------------------------

pub async fn delete_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = load_visible_book(&st, &user, &id).await?;
    if !can_manage(&user, &book) {
        return Err(ApiError::Forbidden);
    }
    st.library.delete_book(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}