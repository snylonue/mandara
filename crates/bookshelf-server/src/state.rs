//! Shared application state.

use std::sync::Arc;

use crate::auth::AuthService;
use crate::config::Config;
use crate::db::DieselDb;
use crate::service::library::Library;

pub struct AppState {
    pub cfg: Config,
    pub diesel_db: DieselDb,
    pub auth: AuthService,
    pub library: Library,
}

pub type St = Arc<AppState>;
