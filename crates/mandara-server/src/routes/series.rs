//! Series endpoints: list/create/detail/edit/delete + member reorder.
//!
//! A series groups the volumes of one publication family (e.g. a
//! light-novel series split from a multi-volume plugin source). Member
//! books carry `series_id` + `volume_no`; the order is the volume order.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use mandara_core::model::SeriesMeta;
use mandara_core::{SeriesExt, SeriesStatus};

use crate::error::ApiError;
use crate::routes::books::BookDetail;
use crate::routes::{St, current_user};

/// A series as seen by the API: metadata + its member count.
#[derive(Serialize)]
pub struct SeriesBrief {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Extended series metadata (publisher/status/tags/…).
    #[serde(default)]
    pub ext: SeriesExt,
    /// Convenience projection: publication status (`null` = unset).
    pub status: Option<SeriesStatus>,
    pub volume_count: u32,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl SeriesBrief {
    fn project(series: SeriesMeta, volume_count: u32) -> Self {
        let ext = series.ext;
        let status = ext.status;
        Self {
            id: series.id,
            title: series.title,
            authors: series.authors,
            description: series.description,
            cover_url: series.cover_url,
            ext,
            status,
            volume_count,
            created_by: series.created_by,
            created_at: series.created_at,
        }
    }
}

impl SeriesBrief {
    pub(crate) async fn from_meta(st: &St, series: SeriesMeta) -> Result<SeriesBrief, ApiError> {
        let volume_count = st.library.series_volume_count(&series.id).await?;
        Ok(SeriesBrief::project(series, volume_count))
    }
}

/// Series detail: metadata + member books (ordered by volume number) with
/// the files the caller can see.
#[derive(Serialize)]
pub struct SeriesDetail {
    pub series: SeriesBrief,
    pub books: Vec<BookDetail>,
}

// GET /api/series -----------------------------------------------------------

pub async fn list_series(
    State(st): State<St>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let series_list = st.library.list_series(&user).await?;
    let mut out = Vec::with_capacity(series_list.len());
    for (series, count) in series_list {
        out.push(SeriesBrief::project(series, count));
    }
    Ok(Json(out))
}

// POST /api/series -----------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateSeries {
    pub title: String,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    /// Extended series metadata (publisher/status/tags/…).
    pub ext: Option<SeriesExt>,
}

pub async fn create_series(
    State(st): State<St>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateSeries>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let title = req.title.trim();
    if title.is_empty() {
        return Err(ApiError::bad_request("series title must not be empty"));
    }
    let series = st
        .library
        .create_series(
            &user,
            title,
            req.authors.as_ref(),
            req.description.as_deref(),
            req.ext,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(SeriesBrief::from_meta(&st, series).await?),
    ))
}

// GET /api/series/{id} -------------------------------------------------------

/// Load the series (visibility-checked) + its member books in volume order.
async fn load_series_detail(
    st: &St,
    user: &mandara_core::model::User,
    id: &str,
) -> Result<SeriesDetail, ApiError> {
    if !st.library.series_visible_to(user, id).await? {
        return Err(ApiError::NotFound(format!("series `{id}` not found")));
    }
    let series = st
        .library
        .get_series(id)
        .await?
        .ok_or_else(|| ApiError::not_found("series"))?;
    let volume_count = st.library.series_volume_count(id).await?;
    let member_books = st.library.series_member_books(id).await?;
    let mut books = Vec::with_capacity(member_books.len());
    for book in member_books {
        let files = st.library.files_of_book(&book.id, user).await?;
        books.push(BookDetail {
            series: Some(series.clone()),
            book,
            files,
        });
    }
    Ok(SeriesDetail {
        series: SeriesBrief::project(series, volume_count),
        books,
    })
}

pub async fn get_series_detail(
    State(st): State<St>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    Ok(Json(load_series_detail(&st, &user, &id).await?))
}

// PATCH /api/series/{id} -------------------------------------------------------

#[derive(Deserialize)]
pub struct PatchSeries {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    /// Extended-metadata merge patch (JSON object): absent key = keep,
    /// `null` = clear, value = set.
    pub ext: Option<serde_json::Value>,
}

pub async fn patch_series(
    State(st): State<St>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PatchSeries>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    let updated = st
        .library
        .update_series(
            &user,
            &id,
            req.title.as_deref(),
            req.authors.as_ref(),
            req.description.as_deref(),
            req.ext.as_ref(),
        )
        .await?;
    Ok(Json(SeriesBrief::from_meta(&st, updated).await?))
}

// PUT /api/series/{id}/members -------------------------------------------------

/// Replace the member set + order of a series in one call.
#[derive(Deserialize)]
pub struct PutMembers {
    /// Books that become the series' volumes, in order (volume_no =
    /// index + 1). Books previously in the series but not listed are
    /// unassigned.
    pub book_ids: Vec<String>,
}

pub async fn put_series_members(
    State(st): State<St>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PutMembers>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    st.library
        .set_series_members(&user, &id, &req.book_ids)
        .await?;
    Ok(Json(load_series_detail(&st, &user, &id).await?))
}

// DELETE /api/series/{id} ------------------------------------------------------

pub async fn delete_series(
    State(st): State<St>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    st.library.delete_series(&user, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
