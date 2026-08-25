//! Reading session endpoints: one user, multiple sessions per file
//! (e.g. one per device), each with independent progress.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{Position, ReadingSession};

use diesel::ExpressionMethods as _;
use diesel::OptionalExtension as _;
use diesel::QueryDsl as _;
use diesel::prelude::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;

use crate::error::ApiError;
use crate::routes::{St, current_user, load_visible_file};
use crate::rows::SessionRow;
#[allow(unused_imports)]
use crate::schema::sessions;

#[derive(Serialize)]
pub struct SessionsResponse {
    pub file_id: String,
    pub book_title: String,
    pub sessions: Vec<ReadingSession>,
}

// GET /api/files/{id}/sessions ------------------------------------------------

pub async fn list_sessions(
    State(st): State<St>,
    headers: HeaderMap,
    Path(file_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let file = load_visible_file(&st, &user, &file_id).await?;

    let mut conn = st.diesel_db.get().await?;
    let rows: Vec<SessionRow> = sessions::table
        .filter(sessions::user_id.eq(&user.id))
        .filter(sessions::file_id.eq(&file_id))
        .order(sessions::updated_at.desc())
        .select(SessionRow::as_select())
        .load(&mut conn)
        .await?;
    drop(conn);
    let book_title = st
        .library
        .get_book(&file.book_id)
        .await?
        .map(|b| b.title)
        .unwrap_or_default();
    Ok(Json(SessionsResponse {
        file_id,
        book_title,
        sessions: rows.into_iter().map(|r| r.into_model()).collect(),
    }))
}

// POST /api/files/{id}/sessions -------------------------------------------------

#[derive(Deserialize)]
pub struct CreateSession {
    pub label: Option<String>,
}

pub async fn create_session(
    State(st): State<St>,
    headers: HeaderMap,
    Path(file_id): Path<String>,
    Json(req): Json<CreateSession>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    load_visible_file(&st, &user, &file_id).await?;

    let label = req
        .label
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| "default".into());
    let label = label.trim();
    if label.chars().count() > 64 {
        return Err(ApiError::bad_request("label too long (max 64 chars)"));
    }

    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut conn = st.diesel_db.get().await?;
    diesel::insert_into(sessions::table)
        .values((
            sessions::id.eq(&id),
            sessions::user_id.eq(&user.id),
            sessions::file_id.eq(&file_id),
            sessions::label.eq(label),
        ))
        .on_conflict((sessions::user_id, sessions::file_id, sessions::label))
        .do_nothing()
        .execute(&mut conn)
        .await?;

    let row: SessionRow = sessions::table
        .filter(sessions::user_id.eq(&user.id))
        .filter(sessions::file_id.eq(&file_id))
        .filter(sessions::label.eq(label))
        .select(SessionRow::as_select())
        .first(&mut conn)
        .await?;
    Ok((StatusCode::CREATED, Json(row.into_model())))
}

// PUT /api/sessions/{id} --------------------------------------------------------

#[derive(Deserialize)]
pub struct UpdatePosition {
    pub chapter_idx: Option<u32>,
    pub offset: Option<u32>,
    pub fraction: Option<f64>,
}

pub async fn update_session(
    State(st): State<St>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<UpdatePosition>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;

    let mut conn = st.diesel_db.get().await?;
    let row: Option<SessionRow> = sessions::table
        .find(&session_id)
        .select(SessionRow::as_select())
        .first(&mut conn)
        .await
        .optional()?;
    let Some(row) = row else {
        return Err(ApiError::not_found("session"));
    };
    if row.user_id != user.id && user.role != bookshelf_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }

    let chapter_count = st
        .library
        .get_file(&row.file_id)
        .await?
        .map(|f| f.chapter_count)
        .unwrap_or(0);
    let position = Position {
        chapter_idx: req.chapter_idx.unwrap_or(row.chapter_idx.max(0) as u32),
        offset: req.offset.unwrap_or(row.offset.max(0) as u32),
        fraction: req.fraction.unwrap_or(f64::from(row.fraction)),
    }
    .clamped(chapter_count);

    // Timestamp generated on the Rust side (same format as the SQLite
    // strftime default; the DB clock is no longer consulted for updates).
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    diesel::update(sessions::table.find(&session_id))
        .set((
            sessions::chapter_idx.eq(position.chapter_idx as i32),
            sessions::offset.eq(position.offset as i32),
            sessions::fraction.eq(position.fraction as f32),
            sessions::updated_at.eq(now),
        ))
        .execute(&mut conn)
        .await?;

    let updated: SessionRow = sessions::table
        .find(&session_id)
        .select(SessionRow::as_select())
        .first(&mut conn)
        .await?;
    Ok(Json(updated.into_model()))
}

// DELETE /api/sessions/{id} ----------------------------------------------------

pub async fn delete_session(
    State(st): State<St>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let mut conn = st.diesel_db.get().await?;
    let owner: Option<String> = sessions::table
        .find(&session_id)
        .select(sessions::user_id)
        .first(&mut conn)
        .await
        .optional()?;
    let Some(owner) = owner else {
        return Err(ApiError::not_found("session"));
    };
    if owner != user.id && user.role != bookshelf_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }
    diesel::delete(sessions::table.find(&session_id))
        .execute(&mut conn)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
