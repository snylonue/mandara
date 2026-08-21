//! Reading session endpoints: one user, multiple sessions per file
//! (e.g. one per device), each with independent progress.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use bookshelf_core::model::{Position, ReadingSession};

use crate::error::ApiError;
use crate::rows::SessionRow;
use crate::routes::{current_user, load_visible_file, St};

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

    let rows: Vec<SessionRow> = sqlx::query_as(
        "SELECT id, user_id, file_id, label, chapter_idx, offset, fraction, updated_at \
         FROM sessions WHERE user_id = ? AND file_id = ? ORDER BY updated_at DESC",
    )
    .bind(&user.id)
    .bind(&file_id)
    .fetch_all(&st.db)
    .await?;
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
    sqlx::query(
        "INSERT INTO sessions (id, user_id, file_id, label) VALUES (?, ?, ?, ?) \
         ON CONFLICT (user_id, file_id, label) DO NOTHING",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&file_id)
    .bind(label)
    .execute(&st.db)
    .await?;

    let row: SessionRow = sqlx::query_as(
        "SELECT id, user_id, file_id, label, chapter_idx, offset, fraction, updated_at \
         FROM sessions WHERE user_id = ? AND file_id = ? AND label = ?",
    )
    .bind(&user.id)
    .bind(&file_id)
    .bind(label)
    .fetch_one(&st.db)
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

    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT id, user_id, file_id, label, chapter_idx, offset, fraction, updated_at \
         FROM sessions WHERE id = ?",
    )
    .bind(&session_id)
    .fetch_optional(&st.db)
    .await?;
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
        fraction: req.fraction.unwrap_or(row.fraction),
    }
    .clamped(chapter_count);

    sqlx::query(
        "UPDATE sessions SET chapter_idx = ?, offset = ?, fraction = ?, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?",
    )
    .bind(position.chapter_idx as i64)
    .bind(position.offset as i64)
    .bind(position.fraction)
    .bind(&session_id)
    .execute(&st.db)
    .await?;

    let updated: SessionRow = sqlx::query_as(
        "SELECT id, user_id, file_id, label, chapter_idx, offset, fraction, updated_at \
         FROM sessions WHERE id = ?",
    )
    .bind(&session_id)
    .fetch_one(&st.db)
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
    let row: Option<(String,)> = sqlx::query_as("SELECT user_id FROM sessions WHERE id = ?")
        .bind(&session_id)
        .fetch_optional(&st.db)
        .await?;
    let Some((owner,)) = row else {
        return Err(ApiError::not_found("session"));
    };
    if owner != user.id && user.role != bookshelf_core::model::Role::Admin {
        return Err(ApiError::Forbidden);
    }
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(&session_id)
        .execute(&st.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}