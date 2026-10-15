//! User-wide reading contexts with independently writable per-file progress.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, prelude::SelectableHelper as _,
};
use diesel_async::RunQueryDsl as _;
use mandara_core::model::{
    InChapter, Position, PositionKind, ReadingProgress, ReadingSession, User,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    extractors::AuthenticatedUser,
    routes::{St, load_visible_file},
    rows::{READING_SESSION_COLUMNS, SessionRow},
    schema::{reading_sessions, sessions},
};

#[derive(Serialize)]
pub struct SessionsResponse {
    pub file_id: String,
    pub sessions: Vec<ReadingProgress>,
}

async fn owned_session(st: &St, user: &User, id: &str) -> Result<ReadingSession, ApiError> {
    let row: ReadingSession = reading_sessions::table
        .find(id)
        .select(READING_SESSION_COLUMNS)
        .first(&mut st.diesel_db.get().await?)
        .await
        .optional()?
        .ok_or_else(|| ApiError::not_found("session"))?;
    if row.user_id != user.id {
        return Err(ApiError::Forbidden);
    }
    Ok(row)
}

pub async fn list_my_sessions(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
) -> Result<impl IntoResponse, ApiError> {
    let rows: Vec<ReadingSession> = reading_sessions::table
        .filter(reading_sessions::user_id.eq(user.id))
        .order((
            reading_sessions::created_at.asc(),
            reading_sessions::id.asc(),
        ))
        .select(READING_SESSION_COLUMNS)
        .load(&mut st.diesel_db.get().await?)
        .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CreateSession {
    pub label: Option<String>,
}

pub async fn create_session(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Json(req): Json<CreateSession>,
) -> Result<impl IntoResponse, ApiError> {
    let label = req.label.unwrap_or_else(|| "default".into());
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 64 {
        return Err(ApiError::bad_request("label must contain 1 to 64 chars"));
    }
    let mut conn = st.diesel_db.get().await?;
    diesel::insert_into(reading_sessions::table)
        .values((
            reading_sessions::id.eq(uuid::Uuid::new_v4().simple().to_string()),
            reading_sessions::user_id.eq(&user.id),
            reading_sessions::label.eq(label),
            reading_sessions::created_at.eq(chrono::Utc::now()),
        ))
        .on_conflict((reading_sessions::user_id, reading_sessions::label))
        .do_nothing()
        .execute(&mut conn)
        .await?;
    let row: ReadingSession = reading_sessions::table
        .filter(reading_sessions::user_id.eq(&user.id))
        .filter(reading_sessions::label.eq(label))
        .select(READING_SESSION_COLUMNS)
        .first(&mut conn)
        .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn list_file_sessions(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(file_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    load_visible_file(&st, &user, &file_id).await?;
    let rows = sessions::table
        .filter(sessions::user_id.eq(&user.id))
        .filter(sessions::file_id.eq(&file_id))
        .order(sessions::updated_at.desc())
        .select(SessionRow::as_select())
        .load(&mut st.diesel_db.get().await?)
        .await?;
    Ok(Json(SessionsResponse {
        file_id,
        sessions: rows
            .into_iter()
            .map(ReadingProgress::try_from)
            .collect::<Result<_, _>>()?,
    }))
}

pub async fn list_progress(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    owned_session(&st, &user, &id).await?;
    let rows = sessions::table
        .filter(sessions::reading_session_id.eq(id))
        .order(sessions::updated_at.desc())
        .select(SessionRow::as_select())
        .load(&mut st.diesel_db.get().await?)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(ReadingProgress::try_from)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

async fn load_progress(
    st: &St,
    session_id: &str,
    file_id: &str,
) -> Result<Option<SessionRow>, ApiError> {
    Ok(sessions::table
        .filter(sessions::reading_session_id.eq(session_id))
        .filter(sessions::file_id.eq(file_id))
        .select(SessionRow::as_select())
        .first(&mut st.diesel_db.get().await?)
        .await
        .optional()?)
}

async fn store_progress(
    st: &St,
    context: ReadingSession,
    file_id: &str,
    position: Position,
) -> Result<ReadingProgress, ApiError> {
    let now = chrono::Utc::now();
    let position_kind = match position.in_chapter {
        InChapter::Fraction { .. } => PositionKind::Fraction,
        InChapter::CharOffset { .. } => PositionKind::Offset,
    };
    let coordinates = crate::writes::SessionCoordinates {
        chapter_idx: position.chapter_idx,
        offset: position.offset(),
    };
    let mut conn = st.diesel_db.get().await?;
    diesel::insert_into(sessions::table)
        .values((
            sessions::id.eq(uuid::Uuid::new_v4().simple().to_string()),
            sessions::user_id.eq(context.user_id),
            sessions::label.eq(context.label),
            sessions::reading_session_id.eq(&context.id),
            sessions::file_id.eq(file_id),
            coordinates,
            sessions::fraction.eq(position.fraction()),
            sessions::position_kind.eq(position_kind),
            sessions::updated_at.eq(now),
        ))
        .on_conflict((sessions::reading_session_id, sessions::file_id))
        .do_update()
        .set((
            coordinates,
            sessions::fraction.eq(position.fraction()),
            sessions::position_kind.eq(position_kind),
            sessions::updated_at.eq(now),
        ))
        .execute(&mut conn)
        .await?;
    let row = sessions::table
        .filter(sessions::reading_session_id.eq(context.id))
        .filter(sessions::file_id.eq(file_id))
        .select(SessionRow::as_select())
        .first(&mut conn)
        .await?;
    Ok(ReadingProgress::try_from(row)?)
}

#[derive(Deserialize)]
pub struct UpdatePosition {
    pub chapter_idx: Option<u32>,
    pub in_chapter: Option<InChapter>,
}

pub async fn update_progress(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path((id, file_id)): Path<(String, String)>,
    Json(req): Json<UpdatePosition>,
) -> Result<impl IntoResponse, ApiError> {
    let context = owned_session(&st, &user, &id).await?;
    let file = load_visible_file(&st, &user, &file_id).await?;
    let old = load_progress(&st, &id, &file_id)
        .await?
        .map(ReadingProgress::try_from)
        .transpose()?
        .map(|p| p.position)
        .unwrap_or_default();
    let position = Position {
        chapter_idx: req.chapter_idx.unwrap_or(old.chapter_idx),
        in_chapter: req.in_chapter.unwrap_or(old.in_chapter),
    }
    .clamped(file.chapter_count);
    Ok(Json(
        store_progress(&st, context, &file_id, position).await?,
    ))
}

#[derive(Deserialize)]
pub struct SyncProgress {
    pub source_session_id: String,
}

/// Copy only this file, including backwards moves; never merge by maximum.
pub async fn sync_progress(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path((id, file_id)): Path<(String, String)>,
    Json(req): Json<SyncProgress>,
) -> Result<impl IntoResponse, ApiError> {
    let target = owned_session(&st, &user, &id).await?;
    owned_session(&st, &user, &req.source_session_id).await?;
    if id == req.source_session_id {
        return Err(ApiError::bad_request("source and target must differ"));
    }
    let file = load_visible_file(&st, &user, &file_id).await?;
    let source = load_progress(&st, &req.source_session_id, &file_id)
        .await?
        .ok_or_else(|| ApiError::not_found("source progress"))?;
    let position = ReadingProgress::try_from(source)?
        .position
        .clamped(file.chapter_count);
    Ok(Json(store_progress(&st, target, &file_id, position).await?))
}

pub async fn delete_session(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    owned_session(&st, &user, &id).await?;
    diesel::delete(reading_sessions::table.find(id))
        .execute(&mut st.diesel_db.get().await?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
