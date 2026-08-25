//! SQLite connection and migrations.

use anyhow::Context;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

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
    // Storage unification: legacy plain-text chapters must be converted
    // to canonical HTML *before* migration 0008 drops the `format`
    // column (the conversion is real code, not SQL expressions). On a
    // fresh database the table doesn't exist yet — ignore that error.
    match crate::service::library::backfill_text_chapters(&pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(converted = n, "backfilled text chapters to html"),
        Err(e) => {
            // Fresh database: the table doesn't exist yet. Already-
            // migrated database: migration 0008 dropped the format column.
            let root = e.root_cause().to_string();
            if !root.contains("no such table") && !root.contains("no such column") {
                return Err(e);
            }
        }
    }
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
