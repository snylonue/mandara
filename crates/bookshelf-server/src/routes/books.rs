//! Book metadata endpoints: catalog, upload (new book), attach a file to an
//! existing metadata entry.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{BookMeta, FileMeta, Visibility};

use crate::error::ApiError;
use crate::routes::{current_user, ListParams, St};

#[derive(Serialize)]
pub struct BookDetail {
    pub book: BookMeta,
    /// Files of this book visible to the current user.
    pub files: Vec<FileMeta>,
}

/// One library entry: metadata plus its visible files.
#[derive(Serialize)]
pub struct BookListEntry {
    pub book: BookMeta,
    pub files: Vec<FileMeta>,
}

fn parse_visibility(value: Option<&str>, default: Visibility) -> Result<Visibility, ApiError> {
    match value {
        None => Ok(default),
        Some("public") => Ok(Visibility::Public),
        Some("private") => Ok(Visibility::Private),
        Some(other) => Err(ApiError::bad_request(format!(
            "visibility must be 'public' or 'private', got `{other}`"
        ))),
    }
}

/// Read a multipart upload form: `file` (required), `visibility`, `label`.
async fn read_upload(mut multipart: Multipart) -> Result<(Vec<u8>, String, Visibility, String), ApiError> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut visibility = Visibility::Private;
    let mut label = String::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("multipart: {e}")))?
    {
        match field.name() {
            Some("file") => {
                filename = field.file_name().map(|s| s.to_string());
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("upload read: {e}")))?
                {
                    buf.extend_from_slice(&chunk);
                }
                bytes = Some(buf);
            }
            Some("visibility") => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("visibility field: {e}")))?;
                visibility = parse_visibility(Some(v.trim()), Visibility::Private)?;
            }
            Some("label") => {
                label = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("label field: {e}")))?
                    .trim()
                    .to_string();
            }
            _ => {}
        }
    }

    let bytes = bytes.ok_or_else(|| ApiError::bad_request("missing `file` field"))?;
    let filename = filename.unwrap_or_else(|| "book.unknown".into());
    Ok((bytes, filename, visibility, label))
}

// GET /api/books -----------------------------------------------------------

pub async fn list_books(
    State(st): State<St>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let entries = st
        .library
        .list_books(params.q.as_deref(), params.source.as_deref(), &user)
        .await?;
    let entries: Vec<BookListEntry> = entries
        .into_iter()
        .map(|(book, files)| BookListEntry { book, files })
        .collect();
    Ok(Json(entries))
}

// POST /api/books (multipart: upload a new book = metadata + first file) ----

pub async fn upload_book(
    State(st): State<St>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let (bytes, filename, visibility, label) = read_upload(multipart).await?;
    let (book, file) = st
        .library
        .ingest_upload(&user.id, &filename, &bytes, visibility, &label)
        .await?;
    Ok((StatusCode::CREATED, Json(BookDetail { book, files: vec![file] })))
}

// POST /api/books/{id}/files (multipart: attach another edition) ------------

pub async fn attach_file(
    State(st): State<St>,
    headers: HeaderMap,
    Path(book_id): Path<String>,
    multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    // The book itself must exist; visibility of the new file defaults to the
    // most visible existing file of the book (or private for a fresh book).
    let default_visibility = st
        .library
        .files_of_book(&book_id, &user)
        .await?
        .first()
        .map(|f| f.visibility)
        .unwrap_or(Visibility::Private);

    let (bytes, filename, visibility, label) = read_upload(multipart).await?;
    let file = st
        .library
        .attach_upload(
            &book_id,
            &user.id,
            &filename,
            &bytes,
            if visibility == Visibility::Private { default_visibility } else { visibility },
            &label,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(file)))
}

// GET /api/books/{id} -------------------------------------------------------

pub async fn get_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = st
        .library
        .get_book(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    let files = st.library.files_of_book(&book.id, &user).await?;
    if files.is_empty() {
        return Err(ApiError::not_found("book"));
    }
    Ok(Json(BookDetail { book, files }))
}

// PATCH /api/books/{id} ------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchBook {
    pub title: Option<String>,
    pub description: Option<String>,
}

pub async fn patch_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PatchBook>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = st
        .library
        .get_book(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    // Only the creator of the metadata (or an admin) may edit it.
    let owned = book.created_by.as_deref() == Some(user.id.as_str());
    if !owned && user.role != bookshelf_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }
    let updated = st
        .library
        .update_book(&id, req.title.as_deref(), req.description.as_deref())
        .await?;
    Ok(Json(updated))
}

// DELETE /api/books/{id} -----------------------------------------------------

pub async fn delete_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = st
        .library
        .get_book(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    let owned = book.created_by.as_deref() == Some(user.id.as_str());
    if !owned && user.role != bookshelf_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }
    st.library.delete_book(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}