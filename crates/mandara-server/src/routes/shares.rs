//! Share endpoints: public read links for files and progress links for
//! reading sessions.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use diesel::ExpressionMethods as _;
use diesel::OptionalExtension as _;
use diesel::QueryDsl as _;
use diesel::expression_methods::NullableExpressionMethods as _;
use diesel::prelude::SelectableHelper as _;
use diesel_async::{AsyncConnection as _, RunQueryDsl as _};

use mandara_core::model::{
    BookMeta, ChapterMeta, FileMeta, Position, ReadingProgress, Share, ShareKind, TocNode,
    Visibility,
};

use crate::error::ApiError;
use crate::extractors::AuthenticatedUser;
use crate::routes::{St, can_manage_file, load_visible_file};
use crate::rows::SHARE_COLUMNS;
use crate::schema::{book_files, sessions, share_files, shares, users};

#[derive(Serialize)]
pub struct ShareResponse {
    pub token: String,
    pub url: String,
    pub kind: ShareKind,
    pub file_id: String,
    pub session_id: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<Share> for ShareResponse {
    fn from(share: Share) -> Self {
        ShareResponse {
            url: format!("/share/{}", share.token),
            token: share.token,
            kind: share.kind,
            file_id: share.file_id,
            session_id: share.session_id,
            expires_at: share.expires_at,
            created_at: share.created_at,
        }
    }
}

async fn load_share(st: &St, token: &str) -> Result<Share, ApiError> {
    let mut conn = st.diesel_db.get().await?;
    let row: Option<Share> = shares::table
        .find(token)
        .select(SHARE_COLUMNS)
        .first(&mut conn)
        .await
        .optional()?;
    let Some(row) = row else {
        return Err(ApiError::not_found("share"));
    };
    if let Some(expires) = &row.expires_at
        && *expires < chrono::Utc::now()
    {
        return Err(ApiError::not_found("share"));
    }
    Ok(row)
}

// POST /api/files/{id}/shares ----------------------------------------------------

#[derive(Deserialize)]
pub struct CreateShare {
    /// `book` (default) or `session`. Invalid values are rejected at
    /// deserialization (400) rather than parsed by hand.
    pub kind: Option<ShareKind>,
    /// Required when kind == "session".
    pub session_id: Option<String>,
    /// Expiry in days from now (optional).
    pub expires_days: Option<u32>,
    /// Explicit content selection; omission preserves single-file links.
    pub file_ids: Option<Vec<String>>,
}

pub async fn create_share(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(file_id): Path<String>,
    Json(req): Json<CreateShare>,
) -> Result<impl IntoResponse, ApiError> {
    let file = load_visible_file(&st, &user, &file_id).await?;
    // Private files are shareable by their owner (or admins) only; public
    // files (e.g. plugin catalogs) may be shared by any viewer.
    let shareable_by_viewer = file.visibility == Visibility::Public && file.owner_id.is_none();
    if !can_manage_file(&user, &file) && !shareable_by_viewer {
        return Err(ApiError::Forbidden);
    }

    let kind = req.kind.unwrap_or(ShareKind::Book);
    let file_ids = req.file_ids.unwrap_or_else(|| vec![file_id.clone()]);
    let unique: std::collections::HashSet<_> = file_ids.iter().collect();
    if file_ids.is_empty()
        || file_ids.len() > 100
        || unique.len() != file_ids.len()
        || !file_ids.contains(&file_id)
        || (kind == ShareKind::Session && file_ids.len() != 1)
    {
        return Err(ApiError::bad_request(
            "select 1..100 distinct files including the default file; session shares allow one file",
        ));
    }
    for id in &file_ids {
        let selected = load_visible_file(&st, &user, id).await?;
        if selected.book_id != file.book_id {
            return Err(ApiError::bad_request(
                "selected files must belong to the same book",
            ));
        }
        if !can_manage_file(&user, &selected)
            && !(selected.visibility == Visibility::Public && selected.owner_id.is_none())
        {
            return Err(ApiError::Forbidden);
        }
    }

    let mut session_id = None;

    if kind == ShareKind::Session {
        let sid = req
            .session_id
            .ok_or_else(|| ApiError::bad_request("session_id required for session shares"))?;
        // session must exist, belong to the caller and point at this file
        let row: Option<(String, String, String)> = sessions::table
            .find(&sid)
            .select((sessions::user_id, sessions::file_id, sessions::label))
            .first(&mut st.diesel_db.get().await?)
            .await
            .optional()?;
        let Some((owner, sfile, _label)) = row else {
            return Err(ApiError::not_found("session"));
        };
        if owner != user.id && user.role != mandara_core::model::Role::Admin {
            return Err(ApiError::Forbidden);
        }
        if sfile != file_id {
            return Err(ApiError::bad_request(
                "session does not belong to this file",
            ));
        }
        session_id = Some(sid);
    }

    let token = uuid::Uuid::new_v4().simple().to_string();
    // Cap the requested expiry so `Duration::days(u32::MAX)` cannot
    // overflow chrono (which would panic in the request handler). 10
    // years is far beyond any practical share lifetime.
    const MAX_EXPIRY_DAYS: u32 = 3650;
    let expires_days = req.expires_days.unwrap_or(0).min(MAX_EXPIRY_DAYS);
    let expires_at =
        (expires_days > 0).then(|| Utc::now() + Duration::days(i64::from(expires_days)));

    let mut conn = st.diesel_db.get().await?;
    conn.transaction::<_, ApiError, _>(async |conn| {
        diesel::insert_into(shares::table)
            .values((
                shares::token.eq(&token),
                shares::kind.eq(kind),
                shares::file_id.eq(&file_id),
                shares::session_id.eq(&session_id),
                shares::created_by.eq(&user.id),
                shares::expires_at.eq(expires_at),
                shares::created_at.eq(Utc::now()),
            ))
            .execute(&mut *conn)
            .await?;
        for id in &file_ids {
            diesel::insert_into(share_files::table)
                .values((share_files::token.eq(&token), share_files::file_id.eq(id)))
                .on_conflict_do_nothing()
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    })
    .await?;

    let row: Share = shares::table
        .find(&token)
        .select(SHARE_COLUMNS)
        .first(&mut conn)
        .await?;
    Ok((StatusCode::CREATED, Json(ShareResponse::from(row))))
}

// Public share views (no auth) ------------------------------------------------

#[derive(Serialize)]
pub struct ShareBookView {
    pub id: String,
    pub title: String,
    pub authors: mandara_core::Authors,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub ext: mandara_core::ext::BookExt,
}

impl From<BookMeta> for ShareBookView {
    fn from(book: BookMeta) -> Self {
        Self {
            id: book.id,
            title: book.title,
            authors: book.authors,
            description: book.description,
            cover_url: book.cover_url,
            ext: book.ext,
        }
    }
}

#[derive(Serialize)]
pub struct ShareFileView {
    pub id: String,
    pub format: mandara_core::model::FileFormat,
    pub label: String,
    pub chapter_count: u32,
    pub source: String,
}

impl From<FileMeta> for ShareFileView {
    fn from(file: FileMeta) -> Self {
        Self {
            id: file.id,
            format: file.format,
            label: file.label,
            chapter_count: file.chapter_count,
            source: file.source,
        }
    }
}

#[derive(Serialize)]
pub struct ShareSessionView {
    pub id: String,
    pub label: String,
    pub owner_username: String,
    pub position: Position,
    pub percent: u8,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct ShareView {
    pub kind: ShareKind,
    pub book: ShareBookView,
    pub file: ShareFileView,
    pub files: Vec<ShareFileView>,
    pub session: Option<ShareSessionView>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

// GET /api/shares/{token} --------------------------------------------------------

pub async fn get_share(
    State(st): State<St>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let share = load_share(&st, &token).await?;

    let file = st
        .library
        .get_file(&share.file_id)
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    let book = st
        .library
        .get_book(&file.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;

    let mut session = None;
    if let Some(sid) = &share.session_id {
        let mut conn = st.diesel_db.get().await?;
        let srow: Option<(crate::rows::SessionRow, Option<String>)> = sessions::table
            .left_join(users::table)
            .filter(sessions::id.eq(sid))
            .select((
                crate::rows::SessionRow::as_select(),
                users::username.nullable(),
            ))
            .first(&mut conn)
            .await
            .optional()?;
        // A session share whose session was deleted is a broken link:
        // report it as gone rather than a phantom `session: null`.
        let Some((progress, owner)) = srow else {
            return Err(ApiError::not_found("share"));
        };
        let progress = ReadingProgress::try_from(progress)?;
        let position = progress.position;
        session = Some(ShareSessionView {
            id: sid.clone(),
            label: progress.label,
            owner_username: owner.unwrap_or_default(),
            percent: position.percent(),
            position,
            updated_at: progress.updated_at,
        });
    }

    let files: Vec<FileMeta> = share_files::table
        .inner_join(book_files::table)
        .filter(share_files::token.eq(&share.token))
        .filter(book_files::book_id.eq(&file.book_id))
        .order((
            book_files::id.eq(&share.file_id).desc(),
            book_files::created_at.asc(),
            book_files::id.asc(),
        ))
        .select(crate::rows::file_columns())
        .load(&mut st.diesel_db.get().await?)
        .await?;
    Ok(Json(ShareView {
        kind: share.kind,
        book: book.into(),
        file: file.into(),
        files: files.into_iter().map(ShareFileView::from).collect(),
        session,
        created_at: share.created_at,
        expires_at: share.expires_at,
    }))
}

// GET /api/shares/{token}/book ----------------------------------------------------

#[derive(Serialize)]
pub struct ShareBookResponse {
    pub book: ShareBookView,
    pub file: ShareFileView,
    pub chapters: Vec<ChapterMeta>,
    pub toc: Vec<TocNode>,
}

#[derive(Deserialize)]
pub struct ShareFileQuery {
    pub file_id: Option<String>,
}

async fn selected_file(
    st: &St,
    share: &Share,
    requested: Option<&str>,
) -> Result<FileMeta, ApiError> {
    let id = requested.unwrap_or(&share.file_id);
    let allowed = share_files::table
        .find((&share.token, id))
        .select(share_files::file_id)
        .first::<String>(&mut st.diesel_db.get().await?)
        .await
        .optional()?;
    if allowed.is_none() {
        return Err(ApiError::not_found("shared file"));
    }
    let file = st
        .library
        .get_file(id)
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    let default = st
        .library
        .get_file(&share.file_id)
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    if file.book_id != default.book_id {
        return Err(ApiError::not_found("shared file"));
    }
    Ok(file)
}

pub async fn share_book(
    State(st): State<St>,
    Path(token): Path<String>,
    Query(query): Query<ShareFileQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &token).await?;
    let file = selected_file(&st, &row, query.file_id.as_deref()).await?;
    let book = st
        .library
        .get_book(&file.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    let chapters = st.library.chapter_titles(&file.id).await?;
    let toc = st.library.file_toc(&file.id).await?;
    Ok(Json(ShareBookResponse {
        book: book.into(),
        file: file.into(),
        chapters,
        toc,
    }))
}

// GET /api/shares/{token}/chapters/{idx} ---------------------------------------------

#[derive(Deserialize)]
pub struct ShareChapterPath {
    pub token: String,
    pub idx: u32,
}

pub async fn share_chapter(
    State(st): State<St>,
    Path(path): Path<ShareChapterPath>,
    Query(query): Query<ShareFileQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &path.token).await?;
    let file = selected_file(&st, &row, query.file_id.as_deref()).await?;
    Ok(Json(st.library.get_chapter(&file, path.idx).await?))
}

// DELETE /api/shares/{token} ------------------------------------------------------

pub async fn delete_share(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &token).await?;
    let is_creator = row.created_by.as_deref() == Some(user.id.as_str());
    if !is_creator && user.role != mandara_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }
    let mut conn = st.diesel_db.get().await?;
    diesel::delete(shares::table.find(&token))
        .execute(&mut conn)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
