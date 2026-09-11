//! Share endpoints: public read links for files and progress links for
//! reading sessions.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use diesel::ExpressionMethods as _;
use diesel::OptionalExtension as _;
use diesel::QueryDsl as _;
use diesel::expression_methods::NullableExpressionMethods as _;
use diesel::prelude::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;

use bookshelf_core::model::{
    ChapterMeta, InChapter, Position, Share, ShareKind, TocNode, User, Visibility,
};

use crate::error::ApiError;
use crate::routes::{St, can_manage_file, current_user, load_visible_file};
use crate::rows::ShareRow;
#[allow(unused_imports)]
use crate::schema::{sessions, shares, users};
use crate::time::DbTs;

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

impl ShareResponse {
    fn from_share(share: Share) -> Self {
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

async fn load_share(st: &St, token: &str) -> Result<ShareRow, ApiError> {
    let mut conn = st.diesel_db.get().await?;
    let row: Option<ShareRow> = shares::table
        .find(token)
        .select(ShareRow::as_select())
        .first(&mut conn)
        .await
        .optional()?;
    let Some(row) = row else {
        return Err(ApiError::not_found("share"));
    };
    if let Some(expires) = &row.expires_at
        && DbTs::from_str(expires.clone()).parse() < chrono::Utc::now()
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
}

pub async fn create_share(
    State(st): State<St>,
    headers: HeaderMap,
    Path(file_id): Path<String>,
    Json(req): Json<CreateShare>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let file = load_visible_file(&st, &user, &file_id).await?;
    // Private files are shareable by their owner (or admins) only; public
    // files (e.g. plugin catalogs) may be shared by any viewer.
    let shareable_by_viewer = file.visibility == Visibility::Public && file.owner_id.is_none();
    if !can_manage_file(&user, &file) && !shareable_by_viewer {
        return Err(ApiError::Forbidden);
    }

    let kind = req.kind.unwrap_or(ShareKind::Book);

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
        if owner != user.id && user.role != bookshelf_core::model::Role::Admin {
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
    let expires_at = (expires_days > 0)
        .then(|| DbTs::from_datetime(Utc::now() + Duration::days(expires_days as i64)));

    let mut conn = st.diesel_db.get().await?;
    diesel::insert_into(shares::table)
        .values((
            shares::token.eq(&token),
            shares::kind.eq(kind.as_ref()),
            shares::file_id.eq(&file_id),
            shares::session_id.eq(&session_id),
            shares::created_by.eq(&user.id),
            shares::expires_at.eq(expires_at.as_ref().map(DbTs::as_str)),
        ))
        .execute(&mut conn)
        .await?;

    let row: ShareRow = shares::table
        .find(&token)
        .select(ShareRow::as_select())
        .first(&mut conn)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ShareResponse::from_share(row.into_model()?)),
    ))
}

// Public share views (no auth) ------------------------------------------------

#[derive(Serialize)]
pub struct ShareBookView {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct ShareFileView {
    pub id: String,
    pub format: bookshelf_core::model::FileFormat,
    pub label: String,
    pub chapter_count: u32,
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
    pub session: Option<ShareSessionView>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

// GET /api/shares/{token} --------------------------------------------------------

pub async fn get_share(
    State(st): State<St>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &token).await?;
    let share = row.into_model()?;

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
        let srow: Option<(String, Option<String>, i64, i64, f64, String)> = sessions::table
            .left_join(users::table)
            .filter(sessions::id.eq(sid))
            .select((
                sessions::label,
                users::username.nullable(),
                sessions::chapter_idx,
                sessions::offset,
                sessions::fraction,
                sessions::updated_at,
            ))
            .first(&mut conn)
            .await
            .optional()?;
        // A session share whose session was deleted is a broken link:
        // report it as gone rather than a phantom `session: null`.
        let Some((label, owner, chapter_idx, _offset, fraction, updated_at)) = srow else {
            return Err(ApiError::not_found("share"));
        };
        let position = Position {
            chapter_idx: chapter_idx.max(0) as u32,
            in_chapter: InChapter::Fraction {
                fraction: fraction.clamp(0.0, 1.0),
            },
        };
        session = Some(ShareSessionView {
            id: sid.clone(),
            label,
            owner_username: owner.unwrap_or_default(),
            percent: position.percent(),
            position,
            updated_at: DbTs::from_str(updated_at).parse(),
        });
    }

    Ok(Json(ShareView {
        kind: share.kind,
        book: ShareBookView {
            id: book.id,
            title: book.title,
            authors: book.authors,
            description: book.description,
        },
        file: ShareFileView {
            id: file.id,
            format: file.format,
            label: file.label,
            chapter_count: file.chapter_count,
        },
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

pub async fn share_book(
    State(st): State<St>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &token).await?;
    let file = st
        .library
        .get_file(&row.file_id)
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    let book = st
        .library
        .get_book(&file.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    st.library.ensure_titles(&file).await?;
    let chapters = st.library.chapter_titles(&file.id).await?;
    let toc = st.library.file_toc(&file.id).await?;
    Ok(Json(ShareBookResponse {
        book: ShareBookView {
            id: book.id,
            title: book.title,
            authors: book.authors,
            description: book.description,
        },
        file: ShareFileView {
            id: file.id,
            format: file.format,
            label: file.label,
            chapter_count: file.chapter_count,
        },
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
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &path.token).await?;
    let file = st
        .library
        .get_file(&row.file_id)
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    match st.library.get_chapter(&file, path.idx).await? {
        Some(chapter) => Ok(Json(chapter)),
        None => Err(ApiError::not_found("chapter")),
    }
}

// DELETE /api/shares/{token} ------------------------------------------------------

pub async fn delete_share(
    State(st): State<St>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user: User = current_user(&st, &headers).await?;
    let row = load_share(&st, &token).await?;
    let is_creator = row.created_by.as_deref() == Some(user.id.as_str());
    if !is_creator && user.role != bookshelf_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }
    let mut conn = st.diesel_db.get().await?;
    diesel::delete(shares::table.find(&token))
        .execute(&mut conn)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
