//! Route handlers.

pub mod auth;
pub mod books;
pub mod plugins;
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

use bookshelf_core::model::{BookMeta, User};

use crate::error::ApiError;

/// Re-exported so submodules can write `use crate::routes::St`.
pub use crate::state::St;

/// Shared helpers ----------------------------------------------------------

/// Resolve the current user (or the local admin when auth is disabled).
pub async fn current_user(st: &St, headers: &HeaderMap) -> Result<User, ApiError> {
    st.auth.require_user(headers, &st.db).await
}

pub fn can_view(user: &User, book: &BookMeta) -> bool {
    user.role == bookshelf_core::model::Role::Admin
        || book.visibility == bookshelf_core::model::Visibility::Public
        || book.owner_id.as_deref() == Some(user.id.as_str())
}

pub fn can_manage(user: &User, book: &BookMeta) -> bool {
    user.role == bookshelf_core::model::Role::Admin
        || book.owner_id.as_deref() == Some(user.id.as_str())
}

/// Load a book the user is allowed to see.
pub async fn load_visible_book(st: &St, user: &User, id: &str) -> Result<BookMeta, ApiError> {
    let book = st
        .library
        .get_book(id)
        .await?
        .ok_or_else(|| ApiError::not_found("book"))?;
    if !can_view(user, &book) {
        return Err(ApiError::Forbidden);
    }
    Ok(book)
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
        .route("/api/books", get(books::list_books).post(books::upload_book))
        .route(
            "/api/books/{id}",
            get(books::get_book)
                .patch(patch(books::patch_book))
                .delete(delete(books::delete_book)),
        )
        .route("/api/books/{id}/chapters/{idx}", get(books::get_chapter))
        .route(
            "/api/books/{id}/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route(
            "/api/sessions/{id}",
            put(sessions::update_session).delete(delete(sessions::delete_session)),
        )
        .route("/api/books/{id}/shares", post(shares::create_share))
        .route(
            "/api/shares/{token}",
            get(shares::get_share).delete(delete(shares::delete_share)),
        )
        .route("/api/shares/{token}/book", get(shares::share_book))
        .route(
            "/api/shares/{token}/chapters/{idx}",
            get(shares::share_chapter),
        )
        .layer(DefaultBodyLimit::max(state.cfg.max_upload_mb as usize * 1024 * 1024))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let mut app = Router::new().merge(api);

    // Serve the built frontend (frontend/dist) at `/` when present.
    let dist = state.cfg.frontend_dir.join("dist");
    if dist.is_dir() {
        let files = ServeDir::new(&dist)
            .not_found_service(ServeFile::new(dist.join("index.html")));
        app = app.fallback_service(files);
    }

    app
}