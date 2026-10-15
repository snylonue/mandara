use diesel::connection::SimpleConnection;
use diesel::prelude::*;

use crate::schema::{book_files, chapters};

const MIGRATION: &str = include_str!("../../../migrations/0025_original_metadata_atomic.sql");

fn connection() -> SqliteConnection {
    let mut conn = SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute(
        "PRAGMA foreign_keys=ON;
        CREATE TABLE book_files (id TEXT PRIMARY KEY NOT NULL, label TEXT NOT NULL,
            orig_ext TEXT, orig_sha256 TEXT, orig_size INTEGER);
        CREATE TABLE chapters (file_id TEXT REFERENCES book_files(id) ON DELETE CASCADE);
        INSERT INTO book_files VALUES ('complete', '完整', 'epub', 'sha', 42),
            ('empty', '无原件', NULL, NULL, NULL);
        INSERT INTO chapters VALUES ('complete');",
    )
    .unwrap();
    conn
}

fn migrate(conn: &mut SqliteConnection) -> QueryResult<()> {
    conn.transaction(|conn| conn.batch_execute(MIGRATION))
}

fn metadata(
    conn: &mut SqliteConnection,
    id: &str,
) -> (Option<String>, Option<String>, Option<i64>) {
    book_files::table
        .find(id)
        .select((
            book_files::orig_ext,
            book_files::orig_sha256,
            book_files::orig_size,
        ))
        .first(conn)
        .unwrap()
}

#[test]
fn migration_preserves_metadata_and_foreign_key_dependents() {
    let conn = &mut connection();
    let before = metadata(conn, "complete");
    migrate(conn).unwrap();
    assert_eq!(metadata(conn, "complete"), before);
    assert_eq!(metadata(conn, "empty"), (None, None, None));
    assert_eq!(chapters::table.count().get_result::<i64>(conn).unwrap(), 1);
    diesel::update(book_files::table.find("complete"))
        .set(book_files::label.eq("重命名"))
        .execute(conn)
        .unwrap();
    assert_eq!(metadata(conn, "complete"), before);
    diesel::delete(book_files::table.find("complete"))
        .execute(conn)
        .unwrap();
    assert_eq!(chapters::table.count().get_result::<i64>(conn).unwrap(), 0);
}

#[test]
fn inserts_and_updates_allow_only_complete_or_absent_originals() {
    let conn = &mut connection();
    migrate(conn).unwrap();
    for mask in 0_u8..8 {
        let ext = (mask & 1 != 0).then_some("epub");
        let sha = (mask & 2 != 0).then_some("sha");
        let size = (mask & 4 != 0).then_some(42_i64);
        let valid = mask == 0 || mask == 7;
        let result = diesel::insert_into(book_files::table)
            .values((
                book_files::id.eq(format!("candidate-{mask}")),
                book_files::label.eq("测试"),
                book_files::orig_ext.eq(ext),
                book_files::orig_sha256.eq(sha),
                book_files::orig_size.eq(size),
            ))
            .execute(conn);
        assert_eq!(result.is_ok(), valid, "insert mask {mask}: {result:?}");
        if !valid {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("original metadata must be all NULL or all non-NULL")
            );
        }
        for baseline in [None, Some(("epub", "sha", 42_i64))] {
            diesel::update(book_files::table.find("complete"))
                .set((
                    book_files::orig_ext.eq(baseline.map(|value| value.0)),
                    book_files::orig_sha256.eq(baseline.map(|value| value.1)),
                    book_files::orig_size.eq(baseline.map(|value| value.2)),
                ))
                .execute(conn)
                .unwrap();
            let before = metadata(conn, "complete");
            let result = diesel::update(book_files::table.find("complete"))
                .set((
                    book_files::orig_ext.eq(ext),
                    book_files::orig_sha256.eq(sha),
                    book_files::orig_size.eq(size),
                ))
                .execute(conn);
            assert_eq!(result.is_ok(), valid, "update mask {mask}: {result:?}");
            let after = metadata(conn, "complete");
            if valid {
                assert_eq!(
                    after,
                    (ext.map(str::to_owned), sha.map(str::to_owned), size)
                );
            } else {
                assert_eq!(after, before);
            }
        }
    }
    // Empty strings and a zero size are values, not NULL.
    diesel::update(book_files::table.find("complete"))
        .set((
            book_files::orig_ext.eq(""),
            book_files::orig_sha256.eq(""),
            book_files::orig_size.eq(0_i64),
        ))
        .execute(conn)
        .unwrap();
}

#[test]
fn migration_rejects_partial_existing_rows_without_rewriting_them() {
    for mask in 1_u8..7 {
        let conn = &mut connection();
        diesel::update(book_files::table.find("complete"))
            .set((
                book_files::orig_ext.eq((mask & 1 != 0).then_some("epub")),
                book_files::orig_sha256.eq((mask & 2 != 0).then_some("sha")),
                book_files::orig_size.eq((mask & 4 != 0).then_some(42_i64)),
            ))
            .execute(conn)
            .unwrap();
        let before = metadata(conn, "complete");
        let error = migrate(conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("book_files_original_all_or_none")
        );
        assert_eq!(metadata(conn, "complete"), before);
        // A failed migration rolls back its temporary schema and can be retried after repair.
        diesel::update(book_files::table.find("complete"))
            .set((
                book_files::orig_ext.eq("epub"),
                book_files::orig_sha256.eq("sha"),
                book_files::orig_size.eq(42_i64),
            ))
            .execute(conn)
            .unwrap();
        migrate(conn).unwrap();
    }
}
