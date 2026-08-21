//! Plugin endpoints: list loaded wasm sources, browse a source's catalog
//! (for picking metadata at upload time) and re-sync all catalogs.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::error::ApiError;
use crate::routes::{current_user, St};

#[derive(Serialize)]
pub struct PluginInfo {
    pub id: String,
}

/// One entry of a plugin's catalog, annotated with the library metadata
/// entry it is already synced into (when it is).
#[derive(Serialize)]
pub struct PluginCatalogEntry {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Id of the metadata entry this plugin book is already synced into.
    pub book_id: Option<String>,
}

// GET /api/plugins ------------------------------------------------------------

pub async fn list_plugins(
    State(st): State<St>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    // any logged-in user may list sources
    current_user(&st, &headers).await?;
    let plugins: Vec<PluginInfo> = st
        .library
        .plugin_ids()
        .into_iter()
        .map(|id| PluginInfo { id })
        .collect();
    Ok(Json(plugins))
}

// GET /api/plugins/{id}/catalog ------------------------------------------------

pub async fn plugin_catalog(
    State(st): State<St>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    // any logged-in user may browse sources
    current_user(&st, &headers).await?;
    let catalog = st.library.plugin_catalog(&id).await?;
    let entries: Vec<PluginCatalogEntry> = catalog
        .into_iter()
        .map(|(book, book_id)| PluginCatalogEntry {
            plugin: id.clone(),
            id: book.id,
            title: book.title,
            authors: book.authors,
            description: book.description,
            cover_url: book.cover_url,
            book_id,
        })
        .collect();
    Ok(Json(entries))
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
