//! Share endpoints: public read links for books and progress links for
//! reading sessions.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{ChapterMeta, Share, ShareKind, User};

use crate::error::ApiError;
use crate::rows::ShareRow;
use crate::routes::{can_manage, current_user, load_visible_book, St};

fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Serialize)]
pub struct ShareResponse {
    pub token: String,
    pub url: String,
    pub kind: ShareKind,
    pub mode: String,
    pub book_id: String,
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
            book_id: share.book_id,
            session_id: share.session_id,
            expires_at: share.expires_at,
            created_at: share.created_at,
        }
    }
}

async fn load_share(st: &St, token: &str) -> Result<ShareRow, ApiError> {
    let row: Option<ShareRow> = sqlx::query_as(
        "SELECT token, kind, mode, book_id, session_id, created_by, expires_at, created_at \
         FROM shares WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(&st.db)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("share"));
    };
    if let Some(expires) = &row.expires_at {
        let now = now();
        if expires.as_str() < now.as_str() {
            // RFC3339 strings with the same prefix format compare correctly
            return Err(ApiError::not_found("share"));
        }
    }
    Ok(row)
}

// POST /api/books/:id/shares ---------------------------------------------------

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
    Path(book_id): Path<String>,
    Json(req): Json<CreateShare>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let book = load_visible_book(&st, &user, &book_id).await?;
    // Owners and admins manage shares; for public books (typically plugin
    // catalogs with no owner) any viewer may create a share link.
    let shareable_by_viewer =
        book.visibility == bookshelf_core::model::Visibility::Public && book.owner_id.is_none();
    if !can_manage(&user, &book) && !shareable_by_viewer {
        return Err(ApiError::Forbidden);
    }

    let kind = match req.kind.as_deref() {
        None | Some("book") => ShareKind::Book,
        Some("session") => ShareKind::Session,
        Some(other) => return Err(ApiError::bad_request(format!("unknown kind: {other}"))),
    };

    let mut session_id = None;
    let mode = match kind {
        ShareKind::Book => "read",
        ShareKind::Session => "progress",
    };

    if kind == ShareKind::Session {
        let sid = req
            .session_id
            .ok_or_else(|| ApiError::bad_request("session_id required for session shares"))?;
        // session must exist, belong to the caller and point at this book
        let row: Option<(String, String, String)> =
            sqlx::query_as("SELECT user_id, book_id, label FROM sessions WHERE id = ?")
                .bind(&sid)
                .fetch_optional(&st.db)
                .await?;
        let Some((owner, sbook, _label)) = row else {
            return Err(ApiError::not_found("session"));
        };
        if owner != user.id && user.role != bookshelf_core::model::Role::Admin {
            return Err(ApiError::Forbidden);
        }
        if sbook != book_id {
            return Err(ApiError::bad_request(
                "session does not belong to this book",
            ));
        }
        session_id = Some(sid);
    }

    let token = uuid::Uuid::new_v4().simple().to_string();
    let expires_at = req.expires_days.map(|days| {
        (Utc::now() + Duration::days(days as i64)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    });

    sqlx::query(
        "INSERT INTO shares (token, kind, mode, book_id, session_id, created_by, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&token)
    .bind(match kind {
        ShareKind::Book => "book",
        ShareKind::Session => "session",
    })
    .bind(mode)
    .bind(&book_id)
    .bind(&session_id)
    .bind(&user.id)
    .bind(&expires_at)
    .execute(&st.db)
    .await?;

    let row: ShareRow = sqlx::query_as(
        "SELECT token, kind, mode, book_id, session_id, created_by, expires_at, created_at \
         FROM shares WHERE token = ?",
    )
    .bind(&token)
    .fetch_one(&st.db)
    .await?;
    let share = row.into_model()?;
    Ok((StatusCode::CREATED, Json(ShareResponse::from_share(share))))
}

/// Public snapshot of a share (no auth): used by the share viewer frontend.
#[derive(Serialize)]
pub struct ShareView {
    pub kind: ShareKind,
    pub mode: String,
    pub book: ShareBookView,
    pub session: Option<ShareSessionView>,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Serialize)]
pub struct ShareBookView {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub chapter_count: u32,
}

#[derive(Serialize)]
pub struct ShareSessionView {
    pub id: String,
    pub label: String,
    pub owner_username: String,
    pub position: bookshelf_core::model::Position,
    pub percent: u8,
    pub updated_at: String,
}

// GET /api/shares/:token -------------------------------------------------------

pub async fn get_share(
    State(st): State<St>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &token).await?;
    let share = row.into_model()?;

    let book = st
        .library
        .get_book(&share.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;

    let mut session = None;
    if let Some(sid) = &share.session_id {
        let srow: Option<(String, String, i64, i64, f64, String, String)> = sqlx::query_as(
            "SELECT s.label, u.username, s.chapter_idx, s.offset, s.fraction, s.updated_at, s.id \
             FROM sessions s LEFT JOIN users u ON u.id = s.user_id WHERE s.id = ?",
        )
        .bind(sid)
        .fetch_optional(&st.db)
        .await?;
        if let Some((label, owner, chapter_idx, offset, fraction, updated_at, _id)) = srow {
            let position = bookshelf_core::model::Position {
                chapter_idx: chapter_idx.max(0) as u32,
                offset: offset.max(0) as u32,
                fraction: fraction.clamp(0.0, 1.0),
            };
            session = Some(ShareSessionView {
                id: sid.clone(),
                label,
                owner_username: owner,
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
            chapter_count: book.chapter_count,
        },
        session,
        created_at: share.created_at,
        expires_at: share.expires_at,
    }))
}

// GET /api/shares/:token/book ---------------------------------------------------

#[derive(Serialize)]
pub struct ShareBookResponse {
    pub book: ShareBookView,
    pub chapters: Vec<ChapterMeta>,
}

pub async fn share_book(
    State(st): State<St>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let row = load_share(&st, &token).await?;
    let book = st
        .library
        .get_book(&row.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    st.library.ensure_titles(&book).await?;
    let chapters = st.library.chapter_titles(&book.id).await?;
    Ok(Json(ShareBookResponse {
        book: ShareBookView {
            id: book.id,
            title: book.title,
            authors: book.authors,
            description: book.description,
            chapter_count: book.chapter_count,
        },
        chapters,
    }))
}

// GET /api/shares/:token/chapters/:idx -------------------------------------------

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
    let book = st
        .library
        .get_book(&row.book_id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    match st.library.get_chapter(&book, path.idx).await? {
        Some(chapter) => Ok(Json(chapter)),
        None => Err(ApiError::not_found("chapter")),
    }
}

// DELETE /api/shares/:token ------------------------------------------------------

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
    sqlx::query("DELETE FROM shares WHERE token = ?")
        .bind(&token)
        .execute(&st.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}