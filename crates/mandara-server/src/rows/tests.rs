use super::*;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::result::Error::DeserializationError;
use mandara_core::{
    Authors, BookExt, BookMeta, ChapterMeta, ContentMode, FileFormat, FileMeta, Role, SeriesExt,
    SeriesMeta, Share, ShareKind, User, Visibility,
};

fn connection() -> SqliteConnection {
    let mut conn = SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute(
        r#"CREATE TABLE users (id TEXT, username TEXT, role TEXT, created_at TEXT);
        CREATE TABLE books (id TEXT, title TEXT, authors TEXT, description TEXT,
            cover_url TEXT, created_by TEXT, created_at TEXT, series_id TEXT,
            volume_no INTEGER, ext TEXT, meta_source TEXT, meta_external_id TEXT);
        CREATE TABLE series (id TEXT, title TEXT, authors TEXT, description TEXT,
            cover_url TEXT, created_by TEXT, created_at TEXT, ext TEXT);
        CREATE TABLE book_files (id TEXT, book_id TEXT, source TEXT, external_id TEXT,
            format TEXT, label TEXT, visibility TEXT, owner_id TEXT, chapter_count INTEGER,
            created_at TEXT, content_mode TEXT, orig_ext TEXT, orig_sha256 TEXT, orig_size INTEGER);
        CREATE TABLE chapters (file_id TEXT, idx INTEGER, title TEXT, content TEXT, linear INTEGER);
        CREATE TABLE sessions (id TEXT, user_id TEXT, file_id TEXT, label TEXT,
            chapter_idx INTEGER, offset INTEGER, fraction REAL, updated_at TEXT,
            reading_session_id TEXT, position_kind TEXT);
        CREATE TABLE shares (token TEXT, kind TEXT, file_id TEXT, session_id TEXT,
            created_by TEXT, expires_at TEXT, created_at TEXT);
        INSERT INTO users VALUES ('user', '读者', 'admin', '2026-08-26 12:34:56.789+00:00');
        INSERT INTO books VALUES ('book', '书名', '["作者"]', NULL, NULL, NULL,
            '2026-08-26 12:34:56.789+00:00', NULL, 2,
            '{"publisher":"出版社","future":{"nested":true}}', NULL, NULL);
        INSERT INTO series VALUES ('series', '系列', '[]', NULL, NULL, NULL,
            '2026-08-26 12:34:56.789+00:00', '{}');
        INSERT INTO book_files VALUES ('file', 'book', 'local', 'file', 'epub', '版本',
            'private', 'user', 3, '2026-08-26 12:34:56.789+00:00', 'file', 'epub', 'sha', 42);
        INSERT INTO chapters VALUES ('file', 0, '正文', '', 1), ('file', 1, '注释', NULL, 0);
        INSERT INTO sessions VALUES ('progress', 'user', 'file', '默认', 1, 12, 0.5,
            '2026-08-26 12:34:56.789+00:00', 'context', 'fraction');
        INSERT INTO shares VALUES ('link', 'book', 'file', NULL, 'user', NULL,
            '2026-08-26 12:34:56.789+00:00');"#,
    )
    .unwrap();
    conn
}

#[test]
fn reads_domain_values_without_field_adapters() {
    use crate::schema::{book_files, books, chapters, shares, users};
    let conn = &mut connection();
    let user = users::table
        .select(USER_COLUMNS)
        .first::<User>(conn)
        .unwrap();
    assert_eq!(user.role, Role::Admin);
    assert_eq!(
        user.created_at.to_rfc3339(),
        "2026-08-26T12:34:56.789+00:00"
    );
    let book: BookMeta = books::table
        .select(BOOK_COLUMNS)
        .first::<BookMeta>(conn)
        .unwrap();
    assert_eq!(book.authors.0, ["作者"]);
    assert_eq!(
        serde_json::to_value(&book.authors).unwrap(),
        serde_json::json!(["作者"])
    );
    assert_eq!(book.volume_no, 2);
    assert_eq!(book.ext.publisher.as_deref(), Some("出版社"));
    assert_eq!(book.ext.extra["future"]["nested"], true);
    let file: FileMeta = book_files::table
        .select(FILE_COLUMNS)
        .first::<FileMeta>(conn)
        .unwrap();
    assert_eq!(file.format, FileFormat::Epub);
    assert_eq!(file.visibility, Visibility::Private);
    assert_eq!(file.content_mode, ContentMode::File);
    assert_eq!(file.chapter_count, 3);
    assert_eq!(file.original.unwrap().size, 42);
    let titles = chapters::table
        .order(chapters::idx)
        .select(CHAPTER_META_COLUMNS)
        .load::<ChapterMeta>(conn)
        .unwrap();
    assert!(titles[0].linear);
    assert!(!titles[1].linear);
    let chapters = chapters::table
        .order(chapters::idx)
        .select(ChapterRow::as_select())
        .load::<ChapterRow>(conn)
        .unwrap();
    let mut chapters = chapters.into_iter();
    assert_eq!(
        Option::<Chapter>::from(chapters.next().unwrap())
            .unwrap()
            .content,
        ""
    );
    assert!(Option::<Chapter>::from(chapters.next().unwrap()).is_none());
    let share = shares::table
        .select(SHARE_COLUMNS)
        .first::<Share>(conn)
        .unwrap();
    assert_eq!(share.kind, ShareKind::Book);
    assert!(share.expires_at.is_none());
}

#[test]
fn writes_and_filters_with_domain_values() {
    use crate::schema::{book_files, books, series, shares, users};
    let conn = &mut connection();
    let authors: Authors = serde_json::from_value(serde_json::json!(["作者甲", "作者乙"])).unwrap();
    let ext =
        BookExt::from_value(serde_json::json!({"publisher":"新出版社", "future": [1,2]})).unwrap();
    let series_ext =
        SeriesExt::from_value(serde_json::json!({"status":"completed", "future":true})).unwrap();
    diesel::update(books::table)
        .set((books::authors.eq(&authors), books::ext.eq(&ext)))
        .execute(conn)
        .unwrap();
    let book = books::table
        .select(BOOK_COLUMNS)
        .first::<BookMeta>(conn)
        .unwrap();
    assert_eq!(book.authors, authors);
    assert_eq!(book.ext, ext);
    diesel::update(series::table)
        .set((
            series::authors.eq(Authors::default()),
            series::ext.eq(&series_ext),
        ))
        .execute(conn)
        .unwrap();
    let series = series::table
        .select(SERIES_COLUMNS)
        .first::<SeriesMeta>(conn)
        .unwrap();
    assert!(series.authors.is_empty());
    assert_eq!(series.ext, series_ext);
    for role in [Role::User, Role::Admin] {
        diesel::update(users::table)
            .set(users::role.eq(role))
            .execute(conn)
            .unwrap();
        assert_eq!(
            users::table
                .filter(users::role.eq(role))
                .select(users::role)
                .first::<Role>(conn)
                .unwrap(),
            role
        );
    }
    for format in [FileFormat::Txt, FileFormat::Plugin, FileFormat::Epub] {
        diesel::update(book_files::table)
            .set(book_files::format.eq(format))
            .execute(conn)
            .unwrap();
        assert_eq!(
            book_files::table
                .select(book_files::format)
                .first::<FileFormat>(conn)
                .unwrap(),
            format
        );
    }
    for visibility in [Visibility::Public, Visibility::Private] {
        diesel::update(book_files::table)
            .set(book_files::visibility.eq(visibility))
            .execute(conn)
            .unwrap();
        assert_eq!(
            book_files::table
                .select(book_files::visibility)
                .first::<Visibility>(conn)
                .unwrap(),
            visibility
        );
    }
    for mode in [ContentMode::Chapters, ContentMode::File] {
        diesel::update(book_files::table)
            .set(book_files::content_mode.eq(mode))
            .execute(conn)
            .unwrap();
        assert_eq!(
            book_files::table
                .select(book_files::content_mode)
                .first::<ContentMode>(conn)
                .unwrap(),
            mode
        );
    }
    for kind in [ShareKind::Session, ShareKind::Book] {
        diesel::update(shares::table)
            .set(shares::kind.eq(kind))
            .execute(conn)
            .unwrap();
        assert_eq!(
            shares::table
                .select(shares::kind)
                .first::<ShareKind>(conn)
                .unwrap(),
            kind
        );
    }
}

#[test]
fn native_timestamps_round_trip_and_nullable_expiry_clears() {
    use crate::schema::{shares, users};
    let conn = &mut connection();
    let timestamp = DateTime::parse_from_rfc3339("2026-08-26T20:34:56.123456789+08:00")
        .unwrap()
        .with_timezone(&Utc);
    diesel::update(users::table)
        .set(users::created_at.eq(timestamp))
        .execute(conn)
        .unwrap();
    let actual = users::table
        .select(users::created_at)
        .first::<DateTime<Utc>>(conn)
        .unwrap();
    assert_eq!(actual, timestamp);
    let stored = users::table
        .select(diesel::dsl::sql::<diesel::sql_types::Text>("created_at"))
        .first::<String>(conn)
        .unwrap();
    assert_eq!(stored, "2026-08-26 12:34:56.123456789+00:00");
    let expires = timestamp + chrono::Duration::days(1);
    diesel::update(shares::table)
        .set(shares::expires_at.eq(Some(expires)))
        .execute(conn)
        .unwrap();
    assert_eq!(
        shares::table
            .select(shares::expires_at)
            .first::<Option<DateTime<Utc>>>(conn)
            .unwrap(),
        Some(expires)
    );
    diesel::update(shares::table)
        .set(shares::expires_at.eq(None::<DateTime<Utc>>))
        .execute(conn)
        .unwrap();
    assert_eq!(
        shares::table
            .select(shares::expires_at)
            .first::<Option<DateTime<Utc>>>(conn)
            .unwrap(),
        None
    );
}

#[test]
fn rejects_invalid_json_timestamps_and_enum_values() {
    use crate::schema::{book_files, books, series, shares, users};
    let conn = &mut connection();
    for raw in ["invalid", "{}", r#"[1]"#] {
        diesel::update(books::table)
            .set(books::authors.eq(raw))
            .execute(conn)
            .unwrap();
        assert!(matches!(
            books::table.select(books::authors).first::<Authors>(conn),
            Err(DeserializationError(_))
        ));
    }
    diesel::update(books::table)
        .set(books::ext.eq("invalid"))
        .execute(conn)
        .unwrap();
    assert!(matches!(
        books::table.select(books::ext).first::<BookExt>(conn),
        Err(DeserializationError(_))
    ));
    diesel::update(series::table)
        .set(series::ext.eq("invalid"))
        .execute(conn)
        .unwrap();
    assert!(matches!(
        series::table.select(series::ext).first::<SeriesExt>(conn),
        Err(DeserializationError(_))
    ));
    conn.batch_execute(
        "UPDATE users SET role='unknown', created_at=''; UPDATE shares SET kind='unknown';",
    )
    .unwrap();
    assert!(matches!(
        users::table.select(users::role).first::<Role>(conn),
        Err(DeserializationError(_))
    ));
    assert!(matches!(
        users::table
            .select(users::created_at)
            .first::<DateTime<Utc>>(conn),
        Err(DeserializationError(_))
    ));
    assert!(matches!(
        shares::table.select(shares::kind).first::<ShareKind>(conn),
        Err(DeserializationError(_))
    ));
    for sql in [
        "UPDATE book_files SET format='unknown'",
        "UPDATE book_files SET format='epub', visibility='unknown'",
        "UPDATE book_files SET visibility='private', content_mode='unknown'",
    ] {
        conn.batch_execute(sql).unwrap();
        assert!(matches!(
            book_files::table
                .select(FILE_COLUMNS)
                .first::<FileMeta>(conn),
            Err(DeserializationError(_))
        ));
    }
}

#[test]
fn metadata_column_rename_preserves_values() {
    use crate::schema::{books, series};
    let conn = &mut connection();
    conn.batch_execute("ALTER TABLE books RENAME COLUMN ext TO ext_meta; ALTER TABLE series RENAME COLUMN ext TO ext_meta;").unwrap();
    conn.batch_execute(
        r#"UPDATE series SET ext_meta='{"status":"completed","future":{"nested":true}}'"#,
    )
    .unwrap();
    conn.batch_execute(include_str!("../../migrations/0024_metadata_ext_name.sql"))
        .unwrap();
    let book = books::table
        .select(books::ext)
        .first::<BookExt>(conn)
        .unwrap();
    assert_eq!(book.publisher.as_deref(), Some("出版社"));
    assert_eq!(book.extra["future"]["nested"], true);
    let series = series::table
        .select(series::ext)
        .first::<SeriesExt>(conn)
        .unwrap();
    assert_eq!(series.status, Some(mandara_core::SeriesStatus::Completed));
    assert_eq!(series.extra["future"]["nested"], true);
}

#[test]
fn orm_rejects_invalid_integer_ranges_and_incomplete_originals() {
    use crate::schema::{book_files, books, chapters};
    let conn = &mut connection();
    for volume in [-1, i64::from(u32::MAX) + 1] {
        diesel::update(books::table)
            .set(books::volume_no.eq(volume))
            .execute(conn)
            .unwrap();
        assert!(matches!(
            books::table.select(BOOK_COLUMNS).first::<BookMeta>(conn),
            Err(DeserializationError(_))
        ));
        assert!(matches!(
            books::table
                .select((books::volume_no,))
                .first::<U32Value>(conn),
            Err(DeserializationError(_))
        ));
        assert!(matches!(
            books::table
                .select((books::series_id, books::volume_no))
                .first::<SeriesCount>(conn),
            Err(DeserializationError(_))
        ));
    }
    for sql in [
        "UPDATE book_files SET chapter_count=-1",
        "UPDATE book_files SET chapter_count=4294967296",
        "UPDATE book_files SET chapter_count=3, orig_size=-1",
        "UPDATE book_files SET orig_size=NULL",
        "UPDATE book_files SET orig_ext=NULL",
        "UPDATE book_files SET orig_sha256=NULL",
    ] {
        conn.batch_execute("UPDATE book_files SET chapter_count=3, orig_ext='epub', orig_sha256='sha', orig_size=42").unwrap();
        conn.batch_execute(sql).unwrap();
        assert!(matches!(
            book_files::table
                .select(FILE_COLUMNS)
                .first::<FileMeta>(conn),
            Err(DeserializationError(_))
        ));
    }
    conn.batch_execute("UPDATE book_files SET orig_ext=NULL, orig_sha256=NULL, orig_size=NULL")
        .unwrap();
    assert!(
        book_files::table
            .select(FILE_COLUMNS)
            .first::<FileMeta>(conn)
            .unwrap()
            .original
            .is_none()
    );
    for idx in [-1, i64::from(u32::MAX) + 1] {
        diesel::update(chapters::table)
            .set(chapters::idx.eq(idx))
            .execute(conn)
            .unwrap();
        assert!(matches!(
            chapters::table
                .select(ChapterRow::as_select())
                .first::<ChapterRow>(conn),
            Err(DeserializationError(_))
        ));
        assert!(matches!(
            chapters::table
                .select(CHAPTER_META_COLUMNS)
                .first::<ChapterMeta>(conn),
            Err(DeserializationError(_))
        ));
    }
}

#[test]
fn position_discriminator_round_trips_and_rejects_unknown_values() {
    use crate::schema::sessions;
    let conn = &mut connection();
    for kind in [PositionKind::Fraction, PositionKind::Offset] {
        diesel::update(sessions::table)
            .set(sessions::position_kind.eq(kind))
            .execute(conn)
            .unwrap();
        let row = sessions::table
            .filter(sessions::position_kind.eq(kind))
            .select(SessionRow::as_select())
            .first(conn)
            .unwrap();
        let progress = ReadingProgress::try_from(row).unwrap();
        assert_eq!(progress.session_id, "context");
        match (kind, progress.position.in_chapter) {
            (PositionKind::Fraction, InChapter::Fraction { fraction }) => assert_eq!(fraction, 0.5),
            (PositionKind::Offset, InChapter::CharOffset { offset }) => assert_eq!(offset, 12),
            _ => panic!("position discriminator selected the wrong variant"),
        }
    }
    conn.batch_execute("UPDATE sessions SET position_kind='unknown'")
        .unwrap();
    assert!(matches!(
        sessions::table
            .select(SessionRow::as_select())
            .first::<SessionRow>(conn),
        Err(DeserializationError(_))
    ));
}

#[test]
fn orm_rejects_invalid_progress_integers() {
    use crate::schema::sessions;
    let conn = &mut connection();
    for sql in [
        "UPDATE sessions SET chapter_idx=-1",
        "UPDATE sessions SET chapter_idx=4294967296",
        "UPDATE sessions SET chapter_idx=1, offset=-1",
        "UPDATE sessions SET offset=4294967296",
    ] {
        conn.batch_execute(sql).unwrap();
        assert!(
            matches!(
                sessions::table
                    .select(SessionRow::as_select())
                    .first::<SessionRow>(conn),
                Err(DeserializationError(_))
            ),
            "{sql}"
        );
    }
}

#[test]
fn progress_conversion_validates_context_and_fraction() {
    use crate::schema::sessions;
    let conn = &mut connection();
    for sql in [
        "UPDATE sessions SET reading_session_id=NULL",
        "UPDATE sessions SET reading_session_id='context', fraction=-0.1",
        "UPDATE sessions SET fraction=1.1",
    ] {
        conn.batch_execute(sql).unwrap();
        let row = sessions::table
            .select(SessionRow::as_select())
            .first(conn)
            .unwrap();
        assert!(ReadingProgress::try_from(row).is_err(), "{sql}");
    }
}

#[test]
fn unsigned_boundaries_round_trip_through_orm() {
    use crate::schema::{book_files, books, chapters, sessions};
    let conn = &mut connection();
    for value in [0_u32, u32::MAX] {
        diesel::update(books::table)
            .set(books::volume_no.eq(i64::from(value)))
            .execute(conn)
            .unwrap();
        assert_eq!(
            books::table
                .select(BOOK_COLUMNS)
                .first::<BookMeta>(conn)
                .unwrap()
                .volume_no,
            value
        );
        diesel::update(book_files::table)
            .set(book_files::chapter_count.eq(i64::from(value)))
            .execute(conn)
            .unwrap();
        assert_eq!(
            book_files::table
                .select(FILE_COLUMNS)
                .first::<FileMeta>(conn)
                .unwrap()
                .chapter_count,
            value
        );
        diesel::update(chapters::table)
            .set(chapters::idx.eq(i64::from(value)))
            .execute(conn)
            .unwrap();
        assert_eq!(
            chapters::table
                .select(CHAPTER_META_COLUMNS)
                .first::<ChapterMeta>(conn)
                .unwrap()
                .idx,
            value
        );
        diesel::update(sessions::table)
            .set((
                sessions::chapter_idx.eq(i64::from(value)),
                sessions::offset.eq(i64::from(value)),
            ))
            .execute(conn)
            .unwrap();
        let row = sessions::table
            .select(SessionRow::as_select())
            .first::<SessionRow>(conn)
            .unwrap();
        assert_eq!((row.chapter_idx, row.offset), (value, value));
    }
    for size in [0_i64, i64::MAX] {
        diesel::update(book_files::table)
            .set(book_files::orig_size.eq(size))
            .execute(conn)
            .unwrap();
        let original = book_files::table
            .select(FILE_COLUMNS)
            .first::<FileMeta>(conn)
            .unwrap()
            .original
            .unwrap();
        assert_eq!(original.size, u64::try_from(size).unwrap());
    }
}
