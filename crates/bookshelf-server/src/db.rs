//! SQLite connection and migrations.

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use crate::config::Config;

pub async fn connect(cfg: &Config) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = cfg.db.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let opts = SqliteConnectOptions::new()
        .filename(&cfg.db)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(opts)
        .await
        .context("connect to sqlite")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;
    Ok(pool)
}

/// When auth is disabled the server runs as a single local admin user.
/// Make sure that user row exists so FKs (sessions, shares) keep working.
pub async fn seed_local_user(pool: &SqlitePool, auth_enabled: bool) -> anyhow::Result<()> {
    if auth_enabled {
        return Ok(());
    }
    sqlx::query(
        "INSERT OR IGNORE INTO users (id, username, password_hash, role) \
         VALUES ('local', 'local', 'auth-disabled', 'admin')",
    )
    .execute(pool)
    .await?;
    Ok(())
}