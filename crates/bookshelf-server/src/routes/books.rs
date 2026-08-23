//! Book metadata endpoints: catalog, upload (attach / auto / manual
//! metadata), metadata edit, refresh-from-plugin, deletion, and attaching
//! files to an existing metadata entry.

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{BookMeta, FileMeta, Visibility};

use crate::error::ApiError;
use crate::routes::{ListParams, St, current_user};
use crate::service::library::{MetadataOverrides, UploadMetadata};

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

/// Metadata fields read from an upload form, besides the file itself.
///
/// The metadata fields of an upload that describe *where* the metadata
/// comes from and how it is overridden.
///
/// - `book_id` present       -> attach to existing metadata,
/// - `plugin_source` present  -> metadata from that plugin's catalog
///   (`plugin_book_id` names the entry),
/// - neither                  -> auto: plugins asked to identify the file,
///   falling back to parsing the file.
///
/// `title`/`authors`/`description`/`cover_url` override the produced
/// metadata in the auto/manual/plugin cases (never for attach).
#[derive(Default)]
struct UploadFields {
    book_id: Option<String>,
    plugin_source: Option<String>,
    plugin_book_id: Option<String>,
    title: Option<String>,
    authors: Option<Vec<String>>,
    description: Option<String>,
    cover_url: Option<String>,
}

async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, ApiError> {
    field
        .text()
        .await
        .map(|s| s.trim().to_string())
        .map_err(|e| ApiError::bad_request(format!("field read: {e}")))
}

/// Read a multipart upload form: `file` (required), `visibility`, `label`
/// and the optional metadata fields.
async fn read_upload(
    mut multipart: Multipart,
) -> Result<(Vec<u8>, String, Visibility, String, UploadFields), ApiError> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut visibility = Visibility::Private;
    let mut label = String::new();
    let mut fields = UploadFields::default();

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
                let v = field_text(field).await?;
                visibility = parse_visibility(Some(&v), Visibility::Private)?;
            }
            Some("label") => label = field_text(field).await?,
            Some("book_id") => fields.book_id = Some(field_text(field).await?),
            Some("plugin_source") => fields.plugin_source = Some(field_text(field).await?),
            Some("plugin_book_id") => fields.plugin_book_id = Some(field_text(field).await?),
            Some("title") => fields.title = Some(field_text(field).await?),
            Some("authors") => {
                let raw = field_text(field).await?;
                if !raw.is_empty() {
                    fields.authors = Some(serde_json::from_str(&raw).map_err(|e| {
                        ApiError::bad_request(format!(
                            "authors must be a JSON string array, got `{raw}`: {e}"
                        ))
                    })?);
                }
            }
            Some("description") => {
                let v = field_text(field).await?;
                if !v.is_empty() {
                    fields.description = Some(v);
                }
            }
            Some("cover_url") => {
                let v = field_text(field).await?;
                if !v.is_empty() {
                    fields.cover_url = Some(v);
                }
            }
            _ => {}
        }
    }

    let bytes = bytes.ok_or_else(|| ApiError::bad_request("missing `file` field"))?;
    let filename = filename.unwrap_or_else(|| "book.unknown".into());
    Ok((bytes, filename, visibility, label, fields))
}

fn upload_mode(fields: &UploadFields) -> Result<UploadMetadata, ApiError> {
    if let Some(book_id) = &fields.book_id {
        return Ok(UploadMetadata::Attach {
            book_id: book_id.clone(),
        });
    }
    if let Some(source) = &fields.plugin_source {
        let book_id_in_source = fields.plugin_book_id.clone().ok_or_else(|| {
            ApiError::bad_request("`plugin_book_id` is required together with `plugin_source`")
        })?;
        return Ok(UploadMetadata::Plugin {
            source: source.clone(),
            book_id_in_source,
        });
    }
    Ok(UploadMetadata::Auto)
}

fn overrides(fields: &UploadFields) -> MetadataOverrides {
    MetadataOverrides {
        title: fields.title.clone(),
        authors: fields.authors.clone(),
        description: fields.description.clone(),
        cover_url: fields.cover_url.clone(),
    }
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

// POST /api/books (multipart) ------------------------------------------------
//
// One upload endpoint for all three metadata modes; see `UploadFields` for
// how the mode is selected.

pub async fn upload_book(
    State(st): State<St>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let (bytes, filename, visibility, label, fields) = read_upload(multipart).await?;
    let (book, file) = st
        .library
        .upload_file(
            &user,
            crate::service::library::UploadInput {
                bytes: &bytes,
                filename: &filename,
                visibility,
                label: &label,
            },
            upload_mode(&fields)?,
            &overrides(&fields),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(BookDetail {
            book,
            files: vec![file],
        }),
    ))
}

// POST /api/books/{id}/files (multipart: attach another edition) -------------

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

    let (bytes, filename, visibility, label, fields) = read_upload(multipart).await?;
    let (_book, file) = st
        .library
        .upload_file(
            &user,
            crate::service::library::UploadInput {
                bytes: &bytes,
                filename: &filename,
                visibility: if visibility == Visibility::Private {
                    default_visibility
                } else {
                    visibility
                },
                label: &label,
            },
            UploadMetadata::Attach {
                book_id: book_id.clone(),
            },
            &overrides(&fields),
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

/// Check the caller may manage the book's metadata (its creator or admin).
fn can_manage_metadata(user: &bookshelf_core::model::User, book: &BookMeta) -> bool {
    user.role == bookshelf_core::model::Role::Admin
        || book.created_by.as_deref() == Some(user.id.as_str())
}

// GET /api/books/{id}/cover --------------------------------------------------
//
// Serves the stored cover image bytes. Deliberately unauthenticated:
// `<img>` tags cannot send the Authorization header, and book ids are
// unguessable uuids (same capability-like model as share tokens). A book
// without a stored cover answers 404 so callers fall back to a placeholder.
pub async fn get_book_cover(
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let (bytes, mime) = st
        .library
        .get_cover(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("cover"))?;
    Ok(([(header::CONTENT_TYPE, mime)], bytes))
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
    if !can_manage_metadata(&user, &book) {
        return Err(ApiError::Forbidden);
    }
    let updated = st
        .library
        .update_book(
            &id,
            req.title.as_deref(),
            req.description.as_deref(),
            None,
            None,
        )
        .await?;
    Ok(Json(updated))
}

// POST /api/books/{id}/refresh -------------------------------------------------
//
// Re-pull metadata from the plugin source(s) backing this book.

pub async fn refresh_book(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = st.library.refresh_book(&user, &id).await?;
    Ok(Json(book))
}

// DELETE /api/books/{id} -----------------------------------------------------
//
// Metadata deletion is distinct from file deletion: it only succeeds once
// every file of the book has been deleted explicitly (409 otherwise).

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
    if !can_manage_metadata(&user, &book) {
        return Err(ApiError::Forbidden);
    }
    st.library.delete_book(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
