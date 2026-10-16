use diesel::connection::SimpleConnection;
use diesel::prelude::*;

use crate::schema::{book_files, chapters, sessions, shares};

#[test]
fn migration_publicizes_only_plugin_content_and_preserves_dependents() {
    let conn = &mut SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute("PRAGMA foreign_keys=ON;
        CREATE TABLE book_files(id TEXT PRIMARY KEY, source TEXT NOT NULL, visibility TEXT NOT NULL, format TEXT);
        CREATE TABLE chapters(file_id TEXT REFERENCES book_files(id) ON DELETE CASCADE);
        CREATE TABLE sessions(file_id TEXT REFERENCES book_files(id) ON DELETE CASCADE);
        CREATE TABLE shares(file_id TEXT REFERENCES book_files(id) ON DELETE CASCADE);
        INSERT INTO book_files VALUES('upload','local','private','epub'),
            ('plugin','fixture','private','plugin'),('parsed','fixture','private','epub');
        INSERT INTO chapters VALUES('plugin');
        INSERT INTO sessions VALUES('plugin');
        INSERT INTO shares VALUES('plugin');").unwrap();
    conn.batch_execute(include_str!(
        "../../../migrations/0027_public_plugin_content.sql"
    ))
    .unwrap();
    let rows: Vec<(String, mandara_core::Visibility)> = book_files::table
        .order(book_files::id)
        .select((book_files::id, book_files::visibility))
        .load(conn)
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("parsed".into(), mandara_core::Visibility::Public),
            ("plugin".into(), mandara_core::Visibility::Public),
            ("upload".into(), mandara_core::Visibility::Private),
        ]
    );
    assert_eq!(chapters::table.count().get_result::<i64>(conn).unwrap(), 1);
    assert_eq!(sessions::table.count().get_result::<i64>(conn).unwrap(), 1);
    assert_eq!(shares::table.count().get_result::<i64>(conn).unwrap(), 1);
    for sql in [
        "INSERT INTO book_files VALUES('invalid','fixture','private','plugin')",
        "UPDATE book_files SET visibility='private' WHERE id='plugin'",
        "UPDATE book_files SET visibility='private' WHERE id='parsed'",
        "UPDATE book_files SET source='fixture' WHERE id='upload'",
    ] {
        assert!(
            conn.batch_execute(sql)
                .unwrap_err()
                .to_string()
                .contains("plugin content must be public")
        );
    }
    conn.batch_execute(
        "INSERT INTO book_files VALUES('public','fixture','public','txt');
        UPDATE book_files SET visibility='public' WHERE id='upload';
        UPDATE book_files SET visibility='private' WHERE id='upload';",
    )
    .unwrap();
}
