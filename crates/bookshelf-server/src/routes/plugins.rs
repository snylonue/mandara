//! Plugin endpoints: list loaded wasm sources and re-sync their catalogs.

use axum::extract::State;
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