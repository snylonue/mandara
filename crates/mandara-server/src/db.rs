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
    async fn nullable_content_migration_preserves_rows_and_cache_presence() {
        use diesel::{ExpressionMethods as _, QueryDsl as _};
        use diesel_async::{RunQueryDsl as _, SimpleAsyncConnection as _};
        let dir =
            std::env::temp_dir().join(format!("mandara-null-content-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = connect_diesel(&dir.join("test.db")).unwrap();
        let mut conn = pool.get().await.unwrap();
        conn.batch_execute("CREATE TABLE book_files(id TEXT PRIMARY KEY NOT NULL,source TEXT NOT NULL);
            CREATE TABLE chapters(file_id TEXT NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,idx INTEGER NOT NULL,title TEXT NOT NULL,content TEXT NOT NULL,linear INTEGER NOT NULL DEFAULT 1 CHECK(linear IN(0,1)),PRIMARY KEY(file_id,idx));
            CREATE TABLE sessions(id TEXT PRIMARY KEY,file_id TEXT REFERENCES book_files(id));
            CREATE TABLE shares(token TEXT PRIMARY KEY,file_id TEXT REFERENCES book_files(id));
            INSERT INTO book_files VALUES('remote','fixture'),('local','local');
            INSERT INTO chapters VALUES('remote',0,'正文','<p>正文</p>',1),('remote',1,'占位','',1),('remote',2,'空白','   ',0),('local',0,'空章','',0);
            INSERT INTO sessions VALUES('session','remote'); INSERT INTO shares VALUES('share','remote');").await.unwrap();
        conn.batch_execute(include_str!(
            "../migrations/0018_nullable_chapter_content.sql"
        ))
        .await
        .unwrap();
        let rows = crate::schema::chapters::table
            .order((
                crate::schema::chapters::file_id.asc(),
                crate::schema::chapters::idx.asc(),
            ))
            .select((
                crate::schema::chapters::file_id,
                crate::schema::chapters::idx,
                crate::schema::chapters::title,
                crate::schema::chapters::content,
                crate::schema::chapters::linear,
            ))
            .load::<(String, i64, String, Option<String>, bool)>(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("local".into(), 0, "空章".into(), Some("".into()), false),
                (
                    "remote".into(),
                    0,
                    "正文".into(),
                    Some("<p>正文</p>".into()),
                    true
                ),
                ("remote".into(), 1, "占位".into(), None, true),
                ("remote".into(), 2, "空白".into(), Some("   ".into()), false),
            ]
        );
        assert_eq!(
            crate::schema::sessions::table
                .select(crate::schema::sessions::file_id)
                .first::<String>(&mut conn)
                .await
                .unwrap(),
            "remote"
        );
        assert_eq!(
            crate::schema::shares::table
                .select(crate::schema::shares::file_id)
                .first::<String>(&mut conn)
                .await
                .unwrap(),
            "remote"
        );
        // New empty cached bodies are preserved; only explicit NULL means missing.
        diesel::update(crate::schema::chapters::table.find(("remote", 1_i64)))
            .set(crate::schema::chapters::content.eq(""))
            .execute(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            crate::schema::chapters::table
                .filter(crate::schema::chapters::content.is_not_null())
                .count()
                .get_result::<i64>(&mut conn)
                .await
                .unwrap(),
            4
        );
        assert!(
            conn.batch_execute("INSERT INTO chapters VALUES('missing',0,'未知',NULL,1)")
                .await
                .is_err()
        );
        assert!(
            conn.batch_execute("UPDATE chapters SET linear=2 WHERE file_id='local'")
                .await
                .is_err()
        );
        conn.batch_execute("DELETE FROM book_files WHERE id='local'")
            .await
            .unwrap();
        assert_eq!(
            crate::schema::chapters::table
                .count()
                .get_result::<i64>(&mut conn)
                .await
                .unwrap(),
            3
        );
        drop(conn);
        drop(pool);
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn independent_content_migration_preserves_legacy_records() {
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::{RunQueryDsl as _, SimpleAsyncConnection as _};
        let dir = std::env::temp_dir().join(format!(
            "mandara-content-migration-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = connect_diesel(&dir.join("test.db")).unwrap();
        let mut conn = pool.get().await.unwrap();
        conn.batch_execute("CREATE TABLE books(id TEXT PRIMARY KEY, meta_source TEXT, meta_external_id TEXT);
            CREATE TABLE book_files(id TEXT PRIMARY KEY, book_id TEXT REFERENCES books(id), source TEXT, external_id TEXT,
                content_source TEXT, content_external_id TEXT, volume_no INTEGER, volume_offset INTEGER, orig_ext TEXT);
            CREATE UNIQUE INDEX idx_book_files_source_ext_vol ON book_files(source, external_id, volume_no);
            CREATE TABLE chapters(file_id TEXT REFERENCES book_files(id), idx INTEGER, content TEXT);
            CREATE TABLE sessions(id TEXT PRIMARY KEY, file_id TEXT REFERENCES book_files(id));
            CREATE TABLE shares(token TEXT PRIMARY KEY, file_id TEXT REFERENCES book_files(id));
            INSERT INTO books VALUES('book', 'metadata', 'meta-id');
            INSERT INTO book_files VALUES('first','book','old','old-id','reader','opaque-id',1,8,'epub');
            INSERT INTO book_files VALUES('second','book','reader','opaque-id',NULL,NULL,0,0,NULL);
            INSERT INTO chapters VALUES('first',0,'缓存正文'),('second',0,'另一正文');
            INSERT INTO sessions VALUES('session','first');
            INSERT INTO shares VALUES('share','first');").await.unwrap();
        conn.batch_execute(include_str!("../migrations/0017_independent_content.sql"))
            .await
            .unwrap();
        let sources: Vec<(String, String, String)> = crate::schema::book_files::table
            .order(crate::schema::book_files::id)
            .select((
                crate::schema::book_files::id,
                crate::schema::book_files::source,
                crate::schema::book_files::external_id,
            ))
            .load(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            sources,
            vec![
                ("first".into(), "reader".into(), "opaque-id".into()),
                ("second".into(), "reader".into(), "opaque-id".into())
            ]
        );
        let content: Option<String> = crate::schema::chapters::table
            .filter(crate::schema::chapters::file_id.eq("first"))
            .select(crate::schema::chapters::content)
            .first(&mut conn)
            .await
            .unwrap();
        assert_eq!(content.as_deref(), Some("缓存正文"));
        let meta: String = crate::schema::books::table
            .select(crate::schema::books::meta_source)
            .first::<Option<String>>(&mut conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(meta, "metadata");
        let session: String = crate::schema::sessions::table
            .select(crate::schema::sessions::file_id)
            .first(&mut conn)
            .await
            .unwrap();
        let share: String = crate::schema::shares::table
            .select(crate::schema::shares::file_id)
            .first(&mut conn)
            .await
            .unwrap();
        assert_eq!((session, share), ("first".into(), "first".into()));
        let original: Option<String> = crate::schema::book_files::table
            .find("first")
            .select(crate::schema::book_files::orig_ext)
            .first(&mut conn)
            .await
            .unwrap();
        assert_eq!(original.as_deref(), Some("epub"));
        drop(conn);
        drop(pool);
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn diesel_pool_applies_pragmas_and_runs_queries() {
        let dir = std::env::temp_dir().join(format!(
            "mandara-diesel-test-{}-{}",
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
            "mandara-join-test-{}",
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
