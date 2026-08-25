//! Shared application state.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::auth::AuthService;
use crate::config::Config;
use crate::db::DieselDb;
use crate::service::library::Library;

pub struct AppState {
    pub cfg: Config,
    /// Legacy sqlx pool — still used by the not-yet-translated services;
    /// removed when the Diesel migration completes (P6).
    #[allow(dead_code)]
    pub db: SqlitePool,
    pub diesel_db: DieselDb,
    pub auth: AuthService,
    pub library: Library,
}

pub type St = Arc<AppState>;
