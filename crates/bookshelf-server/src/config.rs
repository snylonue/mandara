//! Server configuration (CLI args + environment variables).

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "bookshelf-server",
    version,
    about = "Bookshelf: self-hosted light-novel reading server"
)]
pub struct Config {
    /// Address to listen on.
    #[arg(long, env = "BOOKSHELF_ADDR", default_value = "127.0.0.1:8080")]
    pub addr: String,

    /// SQLite database file. Books, metadata and chapters are all stored
    /// here (unified storage).
    #[arg(long, env = "BOOKSHELF_DB", default_value = "data/bookshelf.db")]
    pub db: PathBuf,

    /// Runtime data directory (defaults to the directory of `--db`).
    #[arg(long, env = "BOOKSHELF_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Directory scanned for wasm plugins (*.wasm).
    #[arg(long, env = "BOOKSHELF_PLUGINS_DIR", default_value = "data/plugins")]
    pub plugins_dir: PathBuf,

    /// JWT signing secret. Set a long random value in production.
    #[arg(
        long,
        env = "BOOKSHELF_JWT_SECRET",
        default_value = "dev-only-change-me"
    )]
    pub jwt_secret: String,

    /// Enable authentication and permissions. When disabled the server runs
    /// as a single local admin user (single-user mode).
    #[arg(long, env = "BOOKSHELF_AUTH_ENABLED", default_value_t = true)]
    pub auth_enabled: bool,

    /// Allow new user registration.
    #[arg(long, env = "BOOKSHELF_ALLOW_REGISTER", default_value_t = true)]
    pub allow_register: bool,

    /// Maximum upload size in MiB.
    #[arg(long, env = "BOOKSHELF_MAX_UPLOAD_MB", default_value_t = 64)]
    pub max_upload_mb: u64,

    /// Directory of the web frontend; its `dist/` subdirectory is served at
    /// `/` when present.
    #[arg(long, env = "BOOKSHELF_FRONTEND_DIR", default_value = "frontend")]
    pub frontend_dir: PathBuf,
}

impl Config {
    /// Effective runtime data directory.
    pub fn data_dir(&self) -> PathBuf {
        if let Some(dir) = &self.data_dir {
            return dir.clone();
        }
        self.db
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("data"))
    }
}
