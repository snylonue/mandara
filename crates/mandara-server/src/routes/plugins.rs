//! Plugin endpoints: instances (admin), catalog search + materialization
//! (any logged-in user), config schema/update (admin), re-sync (admin).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use mandara_plugin::{MAX_SEARCH_LIMIT, MAX_SEARCH_OFFSET};

use crate::error::ApiError;
use crate::extractors::{AdminUser, AuthenticatedUser};
use crate::routes::St;

// GET /api/plugins ------------------------------------------------------------

pub async fn list_plugins(
    _user: AuthenticatedUser,
    State(st): State<St>,
) -> Result<impl IntoResponse, ApiError> {
    // any logged-in user may list instances (for the source browser and
    // upload picker)
    let instances = st.library.plugins().infos().await?;
    Ok(Json(instances))
}

// POST /api/plugins/sync (admin) ----------------------------------------------

pub async fn sync_plugins(
    _admin: AdminUser,
    State(st): State<St>,
) -> Result<impl IntoResponse, ApiError> {
    let synced = st.library.sync_plugins().await?;
    Ok(Json(serde_json::json!({ "synced": synced })))
}

// POST /api/plugins/instances/{id}/sync (admin) ----------------------------------
//
// Re-catalogue ONE instance (the admin page's per-instance 重同步).

pub async fn sync_instance(
    _admin: AdminUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let synced = st.library.sync_instance(&id).await?;
    Ok(Json(serde_json::json!({ "synced": synced })))
}

// GET /api/plugins/wasm-files (admin) --------------------------------------------
//
// Basenames of the compiled components the server loaded at startup, for
// the instance registration form.

pub async fn wasm_files(
    _admin: AdminUser,
    State(st): State<St>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(st.library.plugins().wasm_files()))
}

// POST /api/plugins/instances (admin) -------------------------------------------

#[derive(Deserialize)]
pub struct RegisterInstance {
    /// Optional explicit instance id (= source id in `book_files.source`;
    /// defaults to a generated uuid). Must be unique; selected by users.
    pub id: Option<String>,
    pub wasm_file: String,
    #[serde(default = "default_config")]
    pub config: serde_json::Value,
}

fn default_config() -> serde_json::Value {
    serde_json::json!({})
}

pub async fn register_instance(
    _admin: AdminUser,
    State(st): State<St>,
    Json(req): Json<RegisterInstance>,
) -> Result<impl IntoResponse, ApiError> {
    let id = req
        .id
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    if id.is_empty() {
        return Err(ApiError::bad_request("instance id must not be empty"));
    }
    if id.len() > 64 {
        return Err(ApiError::bad_request(
            "instance id must be at most 64 chars",
        ));
    }
    let info = st
        .library
        .plugins()
        .register(&id, &req.wasm_file, req.config)
        .await?;
    // A fresh declare-capable instance is synced right away.
    if info.enabled && info.capabilities.iter().any(|c| c == "declare") {
        let _ = st.library.sync_instance(&info.id).await?;
    }
    Ok((axum::http::StatusCode::CREATED, Json(info)))
}

// DELETE /api/plugins/instances/{id} (admin) -------------------------------------

pub async fn delete_instance(
    _admin: AdminUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    st.library.plugins().unregister(&id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// PUT /api/plugins/instances/{id}/enabled (admin) ---------------------------------

#[derive(Deserialize)]
pub struct EnableInstance {
    pub enabled: bool,
}

pub async fn set_instance_enabled(
    _admin: AdminUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<EnableInstance>,
) -> Result<impl IntoResponse, ApiError> {
    let info = st.library.plugins().set_enabled(&id, req.enabled).await?;
    // Enabling a declare-capable instance re-syncs its catalog.
    if req.enabled && info.capabilities.iter().any(|c| c == "declare") {
        let _ = st.library.sync_instance(&info.id).await?;
    }
    Ok(Json(info))
}

// GET /api/plugins/{id}/config-schema (admin) --------------------------------------

pub async fn plugin_config_schema(
    _admin: AdminUser,
    State(st): State<St>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let (fields, config) = st.library.plugins().config_schema(&id).await?;
    Ok(Json(
        serde_json::json!({ "fields": fields, "config": config }),
    ))
}

// PUT /api/plugins/{id}/config (admin) ---------------------------------------------

#[derive(Deserialize)]
pub struct PutConfig {
    #[serde(default = "default_config")]
    pub config: serde_json::Value,
}

pub async fn put_plugin_config(
    _admin: AdminUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<PutConfig>,
) -> Result<impl IntoResponse, ApiError> {
    let info = st.library.plugins().set_config(&id, req.config).await?;
    // Config changes trigger a re-sync for declare-capable instances.
    if info.enabled && info.capabilities.iter().any(|c| c == "declare") {
        let _ = st.library.sync_instance(&info.id).await?;
    }
    Ok(Json(info))
}

// GET /api/plugins/{id}/search ------------------------------------------------------

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_search_limit() -> u32 {
    20
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchItemKind {
    Book,
    Series,
}

/// One search hit, annotated with the library metadata entry it is
/// already synced into (when it is).
#[derive(Serialize)]
pub struct SearchItem {
    pub kind: SearchItemKind,
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub book_id: Option<String>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub total: u64,
    pub items: Vec<SearchItem>,
}

pub async fn search_plugins(
    _user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<impl IntoResponse, ApiError> {
    // any logged-in user may browse a source's catalog
    // Host caps (design R4): reject instead of clamping.
    if params.limit == 0 || params.limit > MAX_SEARCH_LIMIT {
        return Err(ApiError::bad_request(format!(
            "limit must be in 1..={MAX_SEARCH_LIMIT}"
        )));
    }
    if params.offset > MAX_SEARCH_OFFSET {
        return Err(ApiError::bad_request(format!(
            "offset must be <= {MAX_SEARCH_OFFSET}"
        )));
    }
    let (total, items) = st
        .library
        .search_plugin(
            &id,
            params.q.as_deref().unwrap_or(""),
            params.offset,
            params.limit,
        )
        .await?;
    let items: Vec<SearchItem> = items
        .into_iter()
        .map(|(entry, book_id)| search_item(entry, book_id))
        .collect();
    Ok(Json(SearchResponse { total, items }))
}

// POST /api/plugins/{id}/books ---------------------------------------------------------
//
// Materialize exactly one book of a searchable source into the library
// (metadata + virtual file + chapter-title placeholders; bodies stay
// lazy until first read).

#[derive(Deserialize)]
pub struct MaterializeBook {
    pub book_id: String,
}

pub async fn materialize_book(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<MaterializeBook>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    // Legacy endpoint (superseded by the unified acquisition endpoint);
    // each plugin book is one independently addressable volume.
    let (book, _file) = st
        .library
        .materialize_plugin_book(&user, &id, &req.book_id)
        .await?;
    let files = st.library.files_of_book(&book.id, &user).await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(crate::routes::books::BookDetail {
            series: None,
            book,
            files,
        }),
    ))
}
fn search_item(entry: mandara_plugin::CatalogEntry, book_id: Option<String>) -> SearchItem {
    match entry {
        mandara_plugin::CatalogEntry::Single(b) => SearchItem {
            kind: SearchItemKind::Book,
            id: b.id,
            title: b.title,
            authors: b.authors,
            description: b.description,
            cover_url: b.cover_url,
            book_id,
        },
        mandara_plugin::CatalogEntry::Series(s) => SearchItem {
            kind: SearchItemKind::Series,
            id: s.id,
            title: s.title,
            authors: s.authors,
            description: s.description,
            cover_url: s.cover_url,
            book_id: None,
        },
    }
}

#[derive(Serialize)]
pub struct SeriesLookupResponse {
    pub series: SearchItem,
    pub books: Vec<SearchItem>,
}

pub async fn lookup_series(
    _user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Query(params): Query<LookupParams>,
) -> Result<impl IntoResponse, ApiError> {
    if params.q.trim().is_empty() {
        return Err(ApiError::bad_request("series ID must not be empty"));
    }
    let result = st
        .library
        .plugins()
        .get_series(&id, &params.q)
        .await?
        .ok_or_else(|| ApiError::not_found("plugin series"))?;
    Ok(Json(SeriesLookupResponse {
        series: search_item(mandara_plugin::CatalogEntry::Series(result.series), None),
        books: result
            .books
            .into_iter()
            .map(|book| search_item(mandara_plugin::CatalogEntry::Single(book), None))
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct ImportSeries {
    pub series_id: String,
    #[serde(default)]
    pub book_ids: Vec<String>,
}

pub async fn import_series(
    user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Json(req): Json<ImportSeries>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user.into_inner();
    let series = st
        .library
        .import_plugin_series(&user, &id, &req.series_id, &req.book_ids)
        .await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(crate::routes::series::SeriesBrief::from_meta(&st, series).await?),
    ))
}

#[derive(Deserialize)]
pub struct LookupParams {
    pub q: String,
}

pub async fn lookup_plugin(
    _user: AuthenticatedUser,
    State(st): State<St>,
    Path(id): Path<String>,
    Query(params): Query<LookupParams>,
) -> Result<impl IntoResponse, ApiError> {
    if params.q.is_empty() {
        return Err(ApiError::bad_request("lookup ID must not be empty"));
    }
    let plugins = st.library.plugins();
    let books = if let Some(book) = plugins.get_book(&id, &params.q).await? {
        vec![book]
    } else if let Some(series) = plugins.get_series(&id, &params.q).await? {
        series.books
    } else {
        return Err(ApiError::not_found("plugin book or series"));
    };
    let mut items = Vec::with_capacity(books.len());
    for book in books {
        mandara_core::source::SourceBook::try_from(book.clone())?;
        items.push(search_item(
            mandara_plugin::CatalogEntry::Single(book),
            None,
        ));
    }
    Ok(Json(SearchResponse {
        total: items.len() as u64,
        items,
    }))
}
