//! SQLite connection and migrations.

use std::path::Path;

use anyhow::Context;
use diesel::sqlite::SqliteConnection;
use diesel_async::pooled_connection::{AsyncDieselConnectionManager, ManagerConfig, deadpool};
use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
use sqlx::sqlite::SqliteConnectOptions;

/// The Diesel connection pool (diesel-async over sync SQLite connections
/// dispatched to the blocking pool). Runs alongside the legacy sqlx pool
/// during the module-by-module query translation; sqlx goes away once the
/// last hand-written query is migrated.
pub type DieselDb = deadpool::Pool<SyncConnectionWrapper<SqliteConnection>>;

/// Build the Diesel pool for `db`. Every pooled connection gets the same
/// PRAGMA tuning as the sqlx pool (WAL, FKs on, busy timeout) plus
/// `synchronous = NORMAL` (the plan's P6 item, applied here from day one
/// because it costs nothing and this pool is new).
pub fn connect_diesel(db: &Path) -> anyhow::Result<DieselDb> {
    let url = format!("sqlite://{}", db.display());
    use diesel_async::AsyncConnection as _;
    use diesel_async::SimpleAsyncConnection as _;
    let mut manager_config = ManagerConfig::default();
    manager_config.custom_setup = Box::new(|url| {
        Box::pin(async move {
            let mut conn = SyncConnectionWrapper::<SqliteConnection>::establish(url).await?;
            // Each PRAGMA in its own statement (per the diesel docs);
            // order matters (busy_timeout before WAL).
            for pragma in [
                "PRAGMA busy_timeout = 5000;",
                "PRAGMA journal_mode = WAL;",
                "PRAGMA foreign_keys = ON;",
                "PRAGMA synchronous = NORMAL;",
            ] {
                conn.batch_execute(pragma).await.map_err(|e| {
                    diesel::ConnectionError::CouldntSetupConfiguration(
                        diesel::result::Error::QueryBuilderError(Box::new(e)),
                    )
                })?;
            }
            Ok::<_, diesel::ConnectionError>(conn)
        })
    });
    let manager = AsyncDieselConnectionManager::new_with_config(url, manager_config);
    let pool = deadpool::Pool::builder(manager)
        .max_size(10)
        .build()
        .context("build diesel pool")?;
    Ok(pool)
}

/// Run the SQL migrations. The only remaining sqlx usage: the migration
/// runner needs a connection; the app itself runs on Diesel exclusively.
pub async fn run_migrations(db: &Path) -> anyhow::Result<()> {
    if let Some(parent) = db.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let opts = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(true);
    use sqlx::Connection as _;
    let mut conn = sqlx::SqliteConnection::connect_with(&opts)
        .await
        .context("connect to sqlite")?;
    sqlx::migrate!("./migrations")
        .run(&mut conn)
        .await
        .context("run migrations")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diesel_pool_applies_pragmas_and_runs_queries() {
        let dir = std::env::temp_dir().join(format!(
            "bookshelf-diesel-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");

        // Create the schema through the Diesel connection (raw batch —
        // the migrations runner owns the real schema; this is a test).
        {
            use diesel_async::SimpleAsyncConnection as _;
            let pool0 = connect_diesel(&dir.join("t.db")).unwrap();
            let mut c = pool0.get().await.unwrap();
            c.batch_execute(
                "CREATE TABLE users (id TEXT PRIMARY KEY NOT NULL, username TEXT NOT NULL UNIQUE, \
                 password_hash TEXT NOT NULL DEFAULT '', role TEXT NOT NULL DEFAULT 'user');",
            )
            .await
            .unwrap();
        }

        let pool = connect_diesel(&db).unwrap();
        {
            let mut conn = pool.get().await.unwrap();
            use diesel::QueryDsl as _;
            use diesel_async::RunQueryDsl as _;
            let count: i64 = crate::schema::users::table
                .select(diesel::dsl::count_star())
                .first(&mut conn)
                .await
                .unwrap();
            assert_eq!(count, 0);

            // The custom setup ran the PRAGMAs on this pooled connection.
            let mode: String = diesel::dsl::sql::<diesel::sql_types::Text>(
                "SELECT journal_mode FROM pragma_journal_mode()",
            )
            .get_result(&mut conn)
            .await
            .unwrap();
            assert_eq!(mode, "wal");
            let fks: i64 = diesel::dsl::sql::<diesel::sql_types::BigInt>(
                "SELECT foreign_keys FROM pragma_foreign_keys()",
            )
            .get_result(&mut conn)
            .await
            .unwrap();
            assert_eq!(fks, 1);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod join_tests {
    use super::*;
    use diesel::ExpressionMethods as _;
    use diesel::OptionalExtension as _;
    use diesel::QueryDsl as _;
    use diesel::expression_methods::NullableExpressionMethods as _;
    use diesel_async::RunQueryDsl as _;

    #[tokio::test]
    async fn left_join_sessions_users_compiles() {
        let dir = std::env::temp_dir().join(format!(
            "bookshelf-join-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = connect_diesel(&dir.join("t.db")).unwrap();
        let mut conn = pool.get().await.unwrap();
        use diesel_async::SimpleAsyncConnection as _;
        conn.batch_execute(
            "CREATE TABLE users (id TEXT PRIMARY KEY NOT NULL, username TEXT NOT NULL UNIQUE);\
             CREATE TABLE sessions (id TEXT PRIMARY KEY NOT NULL, user_id TEXT NOT NULL, \
             file_id TEXT NOT NULL, label TEXT NOT NULL DEFAULT '');",
        )
        .await
        .unwrap();
        let r: Option<(String, Option<String>)> = crate::schema::sessions::table
            .left_join(crate::schema::users::table)
            .select((
                crate::schema::sessions::label,
                crate::schema::users::username.nullable(),
            ))
            .filter(crate::schema::sessions::id.eq("nope"))
            .first(&mut conn)
            .await
            .optional()
            .unwrap();
        assert!(r.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
