//! Server configuration (CLI args + environment variables).

use std::path::PathBuf;

use anyhow::Context as _;
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

    /// File holding the JWT signing secret (its content is trimmed). Takes
    /// precedence over `--jwt-secret`; pairs with systemd
    /// `LoadCredential=` or container secret files.
    #[arg(long, env = "BOOKSHELF_JWT_SECRET_FILE", value_name = "PATH")]
    pub jwt_secret_file: Option<PathBuf>,

    /// Allow new user registration.
    #[arg(long, env = "BOOKSHELF_ALLOW_REGISTER", default_value_t = true)]
    pub allow_register: bool,

    /// Mark the session cookie `Secure` (only sent over https). Enable
    /// when the server is served behind TLS.
    #[arg(long, env = "BOOKSHELF_COOKIE_SECURE", default_value_t = false)]
    pub cookie_secure: bool,

    /// Maximum upload size in MiB.
    #[arg(long, env = "BOOKSHELF_MAX_UPLOAD_MB", default_value_t = 64)]
    pub max_upload_mb: u64,

    /// Directory of the web frontend; its `dist/` subdirectory is served at
    /// `/` when present.
    #[arg(long, env = "BOOKSHELF_FRONTEND_DIR", default_value = "frontend")]
    pub frontend_dir: PathBuf,

    /// One-shot upgrade migration: re-run the current parser over every
    /// retained original (data/files/) and replace the stored chapters +
    /// toc, then exit. No HTTP endpoint — run manually after deploying a
    /// parser improvement.
    #[arg(long, env = "BOOKSHELF_REPARSE_ORIGINALS", default_value_t = false)]
    pub reparse_originals: bool,
}

impl Config {
    /// Effective JWT signing secret: the contents of `--jwt-secret-file`
    /// when set, else `--jwt-secret`.
    pub fn jwt_secret(&self) -> anyhow::Result<String> {
        let Some(path) = &self.jwt_secret_file else {
            return Ok(self.jwt_secret.clone());
        };
        let secret = std::fs::read_to_string(path)
            .with_context(|| format!("read JWT secret file {}", path.display()))?;
        let secret = secret.trim();
        if secret.is_empty() {
            anyhow::bail!("JWT secret file {} is empty", path.display());
        }
        Ok(secret.to_string())
    }

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
