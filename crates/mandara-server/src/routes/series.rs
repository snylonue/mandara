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

use mandara_core::SeriesExt;
use mandara_core::model::SeriesMeta;

use crate::error::ApiError;
use crate::extractors::AuthenticatedUser;
use crate::routes::St;
use crate::service::library::BookEntry;

/// A series as seen by the API: metadata + its member count.
#[derive(Serialize)]
pub struct SeriesBrief {
    #[serde(flatten)]
    pub metadata: SeriesMeta,
    pub volume_count: u32,
}

impl SeriesBrief {
    pub(crate) async fn from_meta(st: &St, series: SeriesMeta) -> Result<SeriesBrief, ApiError> {
        let volume_count = st.library.series_volume_count(&series.id).await?;
        Ok(Self {
            metadata: series,
            volume_count,
        })
    }
}

/// Series detail: metadata + member books (ordered by volume number) with
/// the files the caller can see.
#[derive(Serialize)]
pub struct SeriesDetail {
    pub series: SeriesBrief,
    pub books: Vec<BookEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_brief_reuses_metadata_without_a_status_projection() {
        for status in [None, Some(mandara_core::SeriesStatus::Ongoing)] {
            let metadata = SeriesMeta {
                id: "series".into(),
                title: "系列".into(),
                authors: mandara_core::Authors(vec!["作者".into()]),
                description: None,
                cover_url: None,
                ext: SeriesExt {
                    status,
                    ..Default::default()
                },
                created_by: None,
                created_at: chrono::Utc::now(),
            };
            let mut expected = serde_json::to_value(&metadata).unwrap();
            expected["volume_count"] = 3.into();
            let brief = serde_json::to_value(SeriesBrief {
                metadata,
                volume_count: 3,
            })
            .unwrap();
            assert_eq!(brief, expected);
            assert!(brief.get("status").is_none());
            assert!(brief.get("metadata").is_none());
        }
    }
}

// GET /api/series -----------------------------------------------------------

pub async fn list_series(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
) -> Result<impl IntoResponse, ApiError> {
    let series_list = st.library.list_series(&user).await?;
    let out: Vec<_> = series_list
        .into_iter()
        .map(|(metadata, volume_count)| SeriesBrief {
            metadata,
            volume_count,
        })
        .collect();
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
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Json(req): Json<CreateSeries>,
) -> Result<impl IntoResponse, ApiError> {
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
    let books = st.library.list_series_books(&series, user).await?;
    Ok(SeriesDetail {
        series: SeriesBrief {
            metadata: series,
            volume_count,
        },
        books,
    })
}

pub async fn get_series_detail(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
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
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<PatchSeries>,
) -> Result<impl IntoResponse, ApiError> {
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
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<PutMembers>,
) -> Result<impl IntoResponse, ApiError> {
    st.library
        .set_series_members(&user, &id, &req.book_ids)
        .await?;
    Ok(Json(load_series_detail(&st, &user, &id).await?))
}

// DELETE /api/series/{id} ------------------------------------------------------

pub async fn delete_series(
    AuthenticatedUser(user): AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    st.library.delete_series(&user, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
