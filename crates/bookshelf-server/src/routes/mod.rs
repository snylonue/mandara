//! Route handlers.

pub mod auth;
pub mod books;
pub mod files;
pub mod images;
pub mod plugins;
pub mod series;
pub mod sessions;
pub mod shares;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use bookshelf_core::model::{FileMeta, User, Visibility};

use crate::error::ApiError;

/// Re-exported so submodules can write `use crate::routes::St`.
pub use crate::state::St;

/// Resolve the current user (or the local admin when auth is disabled).
pub async fn current_user(st: &St, headers: &HeaderMap) -> Result<User, ApiError> {
    st.auth.require_user(headers, &st.diesel_db).await
}

pub fn can_view_file(user: &User, file: &FileMeta) -> bool {
    user.role == bookshelf_core::model::Role::Admin
        || file.visibility == Visibility::Public
        || file.owner_id.as_deref() == Some(user.id.as_str())
}

pub fn can_manage_file(user: &User, file: &FileMeta) -> bool {
    user.role == bookshelf_core::model::Role::Admin
        || file.owner_id.as_deref() == Some(user.id.as_str())
}

/// Load a file the user is allowed to see.
pub async fn load_visible_file(st: &St, user: &User, id: &str) -> Result<FileMeta, ApiError> {
    let file = st
        .library
        .get_file(id)
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    if !can_view_file(user, &file) {
        return Err(ApiError::Forbidden);
    }
    Ok(file)
}

// Router -------------------------------------------------------------------

/// GET /api/health
pub async fn health(State(st): State<St>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "auth_enabled": st.auth.enabled(),
        "allow_register": st.auth.allow_register(),
    }))
}

#[derive(Deserialize)]
pub struct ListParams {
    pub q: Option<String>,
    pub source: Option<String>,
}

pub fn router(state: St) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/me", get(auth::me))
        .route("/api/plugins", get(plugins::list_plugins))
        .route("/api/plugins/sync", post(plugins::sync_plugins))
        .route("/api/plugins/wasm-files", get(plugins::wasm_files))
        .route("/api/plugins/instances", post(plugins::register_instance))
        .route(
            "/api/plugins/instances/{id}",
            delete(delete(plugins::delete_instance)),
        )
        .route(
            "/api/plugins/instances/{id}/enabled",
            put(plugins::set_instance_enabled),
        )
        .route(
            "/api/plugins/{id}/config-schema",
            get(plugins::plugin_config_schema),
        )
        .route("/api/plugins/{id}/config", put(plugins::put_plugin_config))
        .route("/api/plugins/{id}/search", get(plugins::search_plugins))
        .route("/api/plugins/{id}/books", post(plugins::materialize_book))
        .route(
            "/api/series",
            get(series::list_series).post(series::create_series),
        )
        .route(
            "/api/series/{id}",
            get(series::get_series_detail)
                .patch(patch(series::patch_series))
                .delete(delete(series::delete_series)),
        )
        .route("/api/series/{id}/members", put(series::put_series_members))
        .route(
            "/api/books",
            get(books::list_books).post(books::upload_book),
        )
        .route(
            "/api/books/{id}",
            get(books::get_book)
                .patch(patch(books::patch_book))
                .delete(delete(books::delete_book)),
        )
        .route("/api/books/{id}/files", post(books::attach_file))
        .route("/api/books/{id}/refresh", post(books::refresh_book))
        .route("/api/books/{id}/cover", get(books::get_book_cover))
        .route(
            "/api/files/{id}",
            get(files::get_file)
                .patch(patch(files::patch_file))
                .delete(delete(files::delete_file)),
        )
        .route("/api/files/{id}/chapters/{idx}", get(files::get_chapter))
        .route("/api/files/{id}/download", get(files::download_file))
        .route("/api/images/{id}", get(images::get_image))
        .route(
            "/api/files/{id}/content-source",
            post(files::set_content_source),
        )
        .route(
            "/api/files/{id}/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route(
            "/api/sessions/{id}",
            put(sessions::update_session).delete(delete(sessions::delete_session)),
        )
        .route("/api/files/{id}/shares", post(shares::create_share))
        .route(
            "/api/shares/{token}",
            get(shares::get_share).delete(delete(shares::delete_share)),
        )
        .route("/api/shares/{token}/book", get(shares::share_book))
        .route(
            "/api/shares/{token}/chapters/{idx}",
            get(shares::share_chapter),
        )
        .layer(DefaultBodyLimit::max(
            state.cfg.max_upload_mb as usize * 1024 * 1024,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let mut app = Router::new().merge(api);

    // Serve the built frontend (frontend/dist) at `/` when present.
    let dist = state.cfg.frontend_dir.join("dist");
    if dist.is_dir() {
        let files = ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("index.html")));
        app = app.fallback_service(files);
    }

    app
}
