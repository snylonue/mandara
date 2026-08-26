//! Book metadata endpoints: catalog, upload (attach / auto / manual
//! metadata), metadata edit, refresh-from-plugin, deletion, and attaching
//! files to an existing metadata entry.

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use bookshelf_core::BookExt;
use bookshelf_core::model::{BookMeta, FileMeta, SeriesMeta, Visibility};

use crate::error::ApiError;
use crate::routes::series::SeriesBrief;
use crate::routes::{ListParams, St, current_user};
use crate::service::library::{AcquireContent, AcquireMetadata, MetadataOverrides};

#[derive(Serialize)]
pub struct BookDetail {
    pub book: BookMeta,
    /// Files of this book visible to the current user.
    pub files: Vec<FileMeta>,
    /// The series this book is a volume of (`null` = standalone).
    pub series: Option<SeriesMeta>,
}

/// One library entry: metadata plus its visible files (and the series it
/// belongs to, when any — so the UI groups without extra round trips).
#[derive(Serialize)]
pub struct BookListEntry {
    pub book: BookMeta,
    pub files: Vec<FileMeta>,
    pub series: Option<SeriesMeta>,
}

/// Response of one acquisition (`POST /api/books`): the series created by
/// a volume split (when the source declared >1 volumes) and the books
/// that entered the library. `books` is non-empty; it has exactly one
/// element unless the acquisition split volumes.
#[derive(Serialize)]
pub struct AcquireResult {
    pub series: Option<SeriesBrief>,
    pub books: Vec<BookDetail>,
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

/// Metadata fields of an addition form, besides the content itself.
///
/// The unified acquisition model (获取书籍 → 添加元数据): the *content*
/// comes either from an uploaded `file` or from a plugin source
/// (`plugin_source` + `plugin_book_id`); the *metadata* is attached via
/// `book_id` (existing entry), taken from a plugin catalog
/// (`plugin_source`), or generated automatically (parsed/identified).
/// `title`/`authors`/`description`/`cover_url` override the produced
/// metadata (never applied when attaching).
#[derive(Default)]
struct UploadFields {
    book_id: Option<String>,
    plugin_source: Option<String>,
    plugin_book_id: Option<String>,
    title: Option<String>,
    authors: Option<Vec<String>>,
    description: Option<String>,
    cover_url: Option<String>,
    /// Extended metadata (JSON object; ISBN/publisher/…, see
    /// `BookExt`). Overrides the parsed/plugin value per-key.
    ext: Option<serde_json::Value>,
}

async fn field_text(field: axum::extract::multipart::Field<'_>) -> Result<String, ApiError> {
    field
        .text()
        .await
        .map(|s| s.trim().to_string())
        .map_err(|e| ApiError::bad_request(format!("field read: {e}")))
}

/// Read a multipart addition form: `file` (optional — plugin sources
/// supply the content without one), `visibility`, `label` and the
/// optional metadata fields. Returns `None` as the file when the form
/// has no `file` field (the content then comes from a plugin source).
async fn read_upload(
    mut multipart: Multipart,
) -> Result<(Option<(Vec<u8>, String)>, Visibility, String, UploadFields), ApiError> {
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
            Some("ext") => {
                let raw = field_text(field).await?;
                if !raw.is_empty() {
                    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
                        ApiError::bad_request(format!("`ext` must be a JSON object: {e}"))
                    })?;
                    if !value.is_object() {
                        return Err(ApiError::bad_request("`ext` must be a JSON object"));
                    }
                    fields.ext = Some(value);
                }
            }
            _ => {}
        }
    }

    let file = match (bytes, filename) {
        (Some(bytes), filename) => Some((bytes, filename.unwrap_or_else(|| "book.unknown".into()))),
        (None, _) => None,
    };
    Ok((file, visibility, label, fields))
}

/// The metadata mode of an addition: attach to an existing entry
/// (`book_id`), metadata from a plugin catalog (`plugin_source`), or a
/// new entry (auto/parsed/identified; `overrides` apply).
fn metadata_mode<'a>(
    fields: &'a UploadFields,
    overrides: &'a MetadataOverrides,
) -> Result<AcquireMetadata<'a>, ApiError> {
    if let Some(book_id) = &fields.book_id {
        return Ok(AcquireMetadata::Attach { book_id });
    }
    if let Some(source) = &fields.plugin_source {
        let book_id_in_source = fields.plugin_book_id.as_deref().ok_or_else(|| {
            ApiError::bad_request("`plugin_book_id` is required together with `plugin_source`")
        })?;
        return Ok(AcquireMetadata::Plugin {
            source,
            book_id_in_source,
            overrides,
        });
    }
    if fields.plugin_book_id.is_some() {
        return Err(ApiError::bad_request(
            "`plugin_book_id` requires `plugin_source`",
        ));
    }
    Ok(AcquireMetadata::New { overrides })
}

/// The content source of an addition: the uploaded file, or a book of a
/// plugin source (file mode via `get-book-file` when available, else
/// chapter-mode virtual file).
fn content_source<'a>(
    file: &'a Option<(Vec<u8>, String)>,
    fields: &'a UploadFields,
) -> Result<AcquireContent<'a>, ApiError> {
    if let Some((bytes, filename)) = file {
        return Ok(AcquireContent::File { bytes, filename });
    }
    let (Some(source), Some(book_id_in_source)) = (&fields.plugin_source, &fields.plugin_book_id)
    else {
        return Err(ApiError::bad_request(
            "missing `file`, or a plugin book (`plugin_source` + `plugin_book_id`)",
        ));
    };
    Ok(AcquireContent::Plugin {
        source,
        book_id_in_source,
    })
}

fn overrides(fields: &UploadFields) -> Result<MetadataOverrides, ApiError> {
    Ok(MetadataOverrides {
        title: fields.title.clone(),
        authors: fields.authors.clone(),
        description: fields.description.clone(),
        cover_url: fields.cover_url.clone(),
        ext: match &fields.ext {
            None => None,
            Some(raw) => Some(BookExt::from_value(raw.clone()).map_err(ApiError::bad_request)?),
        },
    })
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
    // Attach the series of every entry (deduplicated lookups — library
    // sizes are small, so an N+1 fetch per series id is fine).
    let mut series_cache: std::collections::HashMap<String, SeriesMeta> = Default::default();
    let mut out = Vec::with_capacity(entries.len());
    for (book, files) in entries {
        if let Some(id) = &book.series_id
            && !series_cache.contains_key(id)
            && let Some(s) = st.library.get_series(id).await?
        {
            series_cache.insert(id.clone(), s);
        }
        out.push(BookListEntry {
            series: book
                .series_id
                .as_ref()
                .and_then(|id| series_cache.get(id).cloned()),
            book,
            files,
        });
    }
    Ok(Json(out))
}

// POST /api/books (multipart) ------------------------------------------------
//
// The single library acquisition endpoint (获取书籍 → 添加元数据): the
// content is either an uploaded `file` or a plugin book
// (`plugin_source` + `plugin_book_id`); the metadata entry is created
// automatically, taken from the plugin catalog, or attached to an
// existing entry (`book_id`). Manual `title`/`authors`/`description`/
// `cover_url` override the produced metadata (never when attaching).

pub async fn upload_book(
    State(st): State<St>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let (file, visibility, label, fields) = read_upload(multipart).await?;
    let overrides = overrides(&fields)?;
    let outcome = st
        .library
        .acquire_book(
            &user,
            content_source(&file, &fields)?,
            metadata_mode(&fields, &overrides)?,
            visibility,
            &label,
        )
        .await?;
    // `series` is a full brief (with the member count) per the contract.
    let series = match outcome.series {
        Some(s) => Some(SeriesBrief::from_meta(&st, s).await?),
        None => None,
    };
    Ok((
        StatusCode::CREATED,
        Json(AcquireResult {
            series,
            books: outcome
                .books
                .into_iter()
                .map(|(book, file)| BookDetail {
                    series: None,
                    book,
                    files: vec![file],
                })
                .collect(),
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

    let (file, visibility, label, fields) = read_upload(multipart).await?;
    // Only file content here — the target metadata is the URL's book.
    let some_file = file.ok_or_else(|| ApiError::bad_request("missing `file` field"))?;
    let outcome = st
        .library
        .acquire_book(
            &user,
            AcquireContent::File {
                bytes: &some_file.0,
                filename: &some_file.1,
            },
            AcquireMetadata::Attach { book_id: &book_id },
            if visibility == Visibility::Private {
                default_visibility
            } else {
                visibility
            },
            &label,
        )
        .await?;
    let _ = fields; // attach mode: metadata fields are ignored
    let (_book, file) = outcome
        .books
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("acquisition produced no books")))?;
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
    let series = match &book.series_id {
        Some(sid) => st.library.get_series(sid).await?,
        None => None,
    };
    Ok(Json(BookDetail {
        book,
        files,
        series,
    }))
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
    /// Extended-metadata merge patch (JSON object): absent key = keep,
    /// `null` = clear, value = set. Re-validated on merge.
    pub ext: Option<serde_json::Value>,
    /// Series assignment (see `library::set_book_series`): a series id
    /// moves the book into it, `null` unassigns, absent keeps the
    /// current series.
    #[serde(default)]
    pub series_id: Option<Option<String>>,
    /// 1-based volume number (absent/`null` = next free volume).
    #[serde(default)]
    pub volume_no: Option<Option<u32>>,
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
    // Series membership (assign / move / unassign) is orthogonal to the
    // metadata fields; both may be sent in one request.
    if req.series_id.is_some() || req.volume_no.is_some() {
        st.library
            .set_book_series(&user, &id, req.series_id, req.volume_no)
            .await?;
    }
    // Extended metadata merge-patch (three-state per key).
    if let Some(patch) = &req.ext {
        st.library.merge_book_ext(&id, patch).await?;
    }
    let updated = st
        .library
        .update_book(
            &id,
            req.title.as_deref(),
            req.description.as_deref(),
            None,
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
