use super::*;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;

fn connection() -> SqliteConnection {
    let mut conn = SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute(
        r#"CREATE TABLE users (id TEXT, username TEXT, role TEXT, created_at TEXT);
        CREATE TABLE books (id TEXT, title TEXT, authors TEXT, description TEXT,
            cover_url TEXT, created_by TEXT, created_at TEXT, series_id TEXT,
            volume_no INTEGER, ext_meta TEXT, meta_source TEXT, meta_external_id TEXT);
        CREATE TABLE book_files (id TEXT, book_id TEXT, source TEXT, external_id TEXT,
            format TEXT, label TEXT, visibility TEXT, owner_id TEXT, chapter_count INTEGER,
            created_at TEXT, content_mode TEXT, orig_ext TEXT, orig_sha256 TEXT, orig_size INTEGER);
        CREATE TABLE chapters (file_id TEXT, idx INTEGER, title TEXT, content TEXT, linear INTEGER);
        CREATE TABLE shares (token TEXT, kind TEXT, file_id TEXT, session_id TEXT,
            created_by TEXT, expires_at TEXT, created_at TEXT);
        INSERT INTO users VALUES ('user', '读者', 'admin', '2026-08-26T20:34:56.789+08:00');
        INSERT INTO books VALUES ('book', '书名', '["作者"]', NULL, NULL, NULL,
            '2026-08-26T12:34:56.789Z', NULL, 2,
            '{"publisher":"出版社","future":{"nested":true}}', NULL, NULL);
        INSERT INTO book_files VALUES ('file', 'book', 'local', 'file', 'epub', '版本',
            'private', 'user', 3, '2026-08-26T12:34:56.789Z', 'file', 'epub', 'sha', 42);
        INSERT INTO chapters VALUES ('file', 0, '正文', '', 1), ('file', 1, '注释', NULL, 0);
        INSERT INTO shares VALUES ('link', 'book', 'file', NULL, 'user', NULL,
            '2026-08-26T12:34:56.789Z');"#,
    )
    .unwrap();
    conn
}

#[test]
fn selects_domain_values_without_manual_decoding() {
    use crate::schema::{book_files, books, chapters, shares, users};
    let conn = &mut connection();
    let user = users::table
        .select(UserRow::as_select())
        .first(conn)
        .unwrap();
    assert_eq!(user.role, Role::Admin);
    assert_eq!(
        user.created_at.to_rfc3339(),
        "2026-08-26T12:34:56.789+00:00"
    );
    let book = books::table
        .select(BookRow::as_select())
        .first(conn)
        .unwrap()
        .into_model();
    assert_eq!(book.authors, ["作者"]);
    assert_eq!(book.volume_no, 2);
    assert_eq!(book.ext.publisher.as_deref(), Some("出版社"));
    assert_eq!(book.ext.extra["future"]["nested"], true);
    let file = book_files::table
        .select(FileRow::as_select())
        .first(conn)
        .unwrap()
        .into_model();
    assert_eq!(file.format, FileFormat::Epub);
    assert_eq!(file.visibility, Visibility::Private);
    assert_eq!(file.content_mode, ContentMode::File);
    assert_eq!(file.chapter_count, 3);
    assert_eq!(file.original.unwrap().size, 42);
    let titles = chapters::table
        .order(chapters::idx)
        .select(ChapterTitleRow::as_select())
        .load::<ChapterTitleRow>(conn)
        .unwrap();
    assert!(titles[0].linear);
    assert!(!titles[1].linear);
    let chapters = chapters::table
        .order(chapters::idx)
        .select(ChapterRow::as_select())
        .load::<ChapterRow>(conn)
        .unwrap();
    let mut chapters = chapters.into_iter();
    assert_eq!(chapters.next().unwrap().into_model().unwrap().content, "");
    assert!(chapters.next().unwrap().into_model().is_none());
    let share = shares::table
        .select(ShareRow::as_select())
        .first(conn)
        .unwrap();
    assert_eq!(share.kind, ShareKind::Book);
    assert!(share.expires_at.is_none());
    diesel::update(shares::table)
        .set(shares::expires_at.eq("2026-08-27T12:34:56.789Z"))
        .execute(conn)
        .unwrap();
    let share = shares::table
        .select(ShareRow::as_select())
        .first(conn)
        .unwrap();
    assert!(share.expires_at.unwrap() > share.created_at);
}

#[test]
fn preserves_legacy_fallbacks_and_rejects_unknown_enums() {
    use crate::schema::{book_files, books, users};
    let conn = &mut connection();
    diesel::update(books::table)
        .set((
            books::authors.eq("invalid"),
            books::ext_meta.eq("invalid"),
            books::created_at.eq(""),
            books::volume_no.eq(-1_i64),
        ))
        .execute(conn)
        .unwrap();
    let book = books::table
        .select(BookRow::as_select())
        .first(conn)
        .unwrap();
    assert!(book.authors.is_empty());
    assert_eq!(book.ext_meta, BookExt::default());
    assert_eq!(book.created_at, DateTime::<Utc>::default());
    assert_eq!(book.volume_no, 0);
    diesel::update(users::table)
        .set(users::role.eq("unknown"))
        .execute(conn)
        .unwrap();
    assert!(matches!(
        users::table
            .select(UserRow::as_select())
            .first::<UserRow>(conn),
        Err(diesel::result::Error::DeserializationError(_))
    ));
    for (format, visibility, mode) in [
        ("unknown", "private", "file"),
        ("epub", "unknown", "file"),
        ("epub", "private", "unknown"),
    ] {
        diesel::update(book_files::table)
            .set((
                book_files::format.eq(format),
                book_files::visibility.eq(visibility),
                book_files::content_mode.eq(mode),
            ))
            .execute(conn)
            .unwrap();
        assert!(matches!(
            book_files::table
                .select(FileRow::as_select())
                .first::<FileRow>(conn),
            Err(diesel::result::Error::DeserializationError(_))
        ));
    }
}
