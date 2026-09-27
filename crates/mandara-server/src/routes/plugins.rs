//! Plugin endpoints: instances (admin), catalog search + materialization
//! (any logged-in user), config schema/update (admin), re-sync (admin).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use mandara_plugin::{MAX_SEARCH_LIMIT, MAX_SEARCH_OFFSET};

use crate::error::ApiError;
use crate::routes::{St, current_user};

// GET /api/plugins ------------------------------------------------------------

pub async fn list_plugins(
    State(st): State<St>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    // any logged-in user may list instances (for the source browser and
    // upload picker)
    current_user(&st, &headers).await?;
    let instances = st.library.plugins().infos().await?;
    Ok(Json(instances))
}

// POST /api/plugins/sync (admin) ----------------------------------------------

pub async fn sync_plugins(
    State(st): State<St>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
    let synced = st.library.sync_plugins().await?;
    Ok(Json(serde_json::json!({ "synced": synced })))
}

// POST /api/plugins/instances/{id}/sync (admin) ----------------------------------
//
// Re-catalogue ONE instance (the admin page's per-instance 重同步).

pub async fn sync_instance(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
    let synced = st.library.sync_instance(&id).await?;
    Ok(Json(serde_json::json!({ "synced": synced })))
}

// GET /api/plugins/wasm-files (admin) --------------------------------------------
//
// Basenames of the compiled components the server loaded at startup, for
// the instance registration form.

pub async fn wasm_files(
    State(st): State<St>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
    Ok(Json(st.library.plugins().wasm_files()))
}

// POST /api/plugins/instances (admin) -------------------------------------------

#[derive(Deserialize)]
pub struct RegisterInstance {
    /// Optional explicit instance id (= source id in `book_files.source`;
    /// defaults to a generated uuid). Stable ids like `"wiki"` let a
    /// metadata plugin point its `content-source` at a content plugin's
    /// instance. Must be unique.
    pub id: Option<String>,
    pub wasm_file: String,
    #[serde(default = "default_config")]
    pub config: serde_json::Value,
}

fn default_config() -> serde_json::Value {
    serde_json::json!({})
}

pub async fn register_instance(
    State(st): State<St>,
    headers: HeaderMap,
    Json(req): Json<RegisterInstance>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
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
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
    st.library.plugins().unregister(&id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

// PUT /api/plugins/instances/{id}/enabled (admin) ---------------------------------

#[derive(Deserialize)]
pub struct EnableInstance {
    pub enabled: bool,
}

pub async fn set_instance_enabled(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<EnableInstance>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
    let info = st.library.plugins().set_enabled(&id, req.enabled).await?;
    // Enabling a declare-capable instance re-syncs its catalog.
    if req.enabled && info.capabilities.iter().any(|c| c == "declare") {
        let _ = st.library.sync_instance(&info.id).await?;
    }
    Ok(Json(info))
}

// GET /api/plugins/{id}/config-schema (admin) --------------------------------------

pub async fn plugin_config_schema(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
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
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PutConfig>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    crate::auth::AuthService::require_admin(&user)?;
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

/// One search hit, annotated with the library metadata entry it is
/// already synced into (when it is).
#[derive(Serialize)]
pub struct SearchItem {
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
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<impl IntoResponse, ApiError> {
    // any logged-in user may browse a source's catalog
    current_user(&st, &headers).await?;
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
        .map(|(book, book_id)| SearchItem {
            id: book.id,
            title: book.title,
            authors: book.authors,
            description: book.description,
            cover_url: book.cover_url,
            book_id,
        })
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
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<MaterializeBook>,
) -> Result<impl IntoResponse, ApiError> {
    let user = current_user(&st, &headers).await?;
    // Legacy endpoint (superseded by the unified acquisition endpoint);
    // the first created book is returned when the entry was split.
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
