//! Shared application state.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::auth::AuthService;
use crate::config::Config;
use crate::service::library::Library;

pub struct AppState {
    pub cfg: Config,
    pub db: SqlitePool,
    pub auth: AuthService,
    pub library: Library,
}

pub type St = Arc<AppState>;
