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

use bookshelf_core::model::{ChapterMeta, Position, Share, ShareKind, TocNode, User, Visibility};

use crate::error::ApiError;
use crate::routes::{St, can_manage_file, current_user, load_visible_file};
use crate::rows::ShareRow;
#[allow(unused_imports)]
use crate::schema::{sessions, shares, users};

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Serialize)]
pub struct ShareResponse {
    pub token: String,
    pub url: String,
    pub kind: ShareKind,
    pub mode: String,
    pub file_id: String,
    pub session_id: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

impl ShareResponse {
    fn from_share(share: Share) -> Self {
        ShareResponse {
            url: format!("/share/{}", share.token),
            token: share.token,
            kind: share.kind,
            mode: share.mode,
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
        && expires.as_str() < now().as_str()
    {
        // RFC3339 strings with the same prefix format compare correctly
        return Err(ApiError::not_found("share"));
    }
    Ok(row)
}

// POST /api/files/{id}/shares ----------------------------------------------------

#[derive(Deserialize)]
pub struct CreateShare {
    /// `"book"` (default) or `"session"`.
    pub kind: Option<String>,
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

    let kind: ShareKind = match req.kind.as_deref() {
        None => ShareKind::Book,
        Some(s) => s.parse().map_err(ApiError::bad_request)?,
    };

    let mut session_id = None;
    let mode = kind.mode();

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
    let expires_at = req.expires_days.map(|days| {
        (Utc::now() + Duration::days(days as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });

    let mut conn = st.diesel_db.get().await?;
    diesel::insert_into(shares::table)
        .values((
            shares::token.eq(&token),
            shares::kind.eq(kind.as_ref()),
            shares::mode.eq(mode),
            shares::file_id.eq(&file_id),
            shares::session_id.eq(&session_id),
            shares::created_by.eq(&user.id),
            shares::expires_at.eq(&expires_at),
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
    pub format: String,
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
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct ShareView {
    pub kind: ShareKind,
    pub mode: String,
    pub book: ShareBookView,
    pub file: ShareFileView,
    pub session: Option<ShareSessionView>,
    pub created_at: String,
    pub expires_at: Option<String>,
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
        if let Some((label, owner, chapter_idx, offset, fraction, updated_at)) = srow {
            let position = Position {
                chapter_idx: chapter_idx.max(0) as u32,
                offset: offset.max(0) as u32,
                fraction: fraction.clamp(0.0, 1.0),
            };
            session = Some(ShareSessionView {
                id: sid.clone(),
                label,
                owner_username: owner.unwrap_or_default(),
                position,
                percent: position.percent(),
                updated_at,
            });
        }
    }

    Ok(Json(ShareView {
        kind: share.kind,
        mode: share.mode,
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
