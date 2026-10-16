use super::*;
use diesel_async::{AsyncConnection as _, SimpleAsyncConnection as _};
use mandara_plugin::{BookEntry, CatalogEntry, SeriesEntry};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn count_selects(db: &DieselDb) -> Arc<AtomicUsize> {
    db.resize(1);
    let queries = Arc::new(AtomicUsize::new(0));
    let count = queries.clone();
    db.get().await.unwrap().set_instrumentation(
        move |event: diesel::connection::InstrumentationEvent<'_>| {
            if let diesel::connection::InstrumentationEvent::StartQuery { query, .. } = event
                && query.to_string().starts_with("SELECT ")
                && query.to_string().contains(" FROM ")
            {
                count.fetch_add(1, Ordering::SeqCst);
            }
        },
    );
    queries
}

fn book(id: &str) -> CatalogEntry {
    CatalogEntry::Single(BookEntry {
        id: id.into(),
        title: "书籍".into(),
        authors: vec![],
        description: None,
        cover_url: None,
        extra: None,
        series: None,
        volume_no: 0,
    })
}

#[tokio::test]
async fn metadata_only_acquisition_returns_one_book_without_a_file() {
    let (library, _, dir) = download_fixture().await;
    library.diesel_db.get().await.unwrap().batch_execute(
        "INSERT INTO users (id, username, password_hash, role) VALUES ('admin', '管理员', '', 'admin');",
    ).await.unwrap();
    let user = User {
        id: "admin".into(),
        username: "管理员".into(),
        role: Role::Admin,
        created_at: chrono::Utc::now(),
    };
    let overrides = MetadataOverrides {
        title: Some("独立元数据".into()),
        ..Default::default()
    };
    let outcome = library
        .acquire_book(
            &user,
            AcquireContent::None,
            AcquireMetadata::New {
                overrides: &overrides,
            },
            Visibility::Private,
            "",
        )
        .await
        .unwrap();
    assert_eq!(outcome.book.title, "独立元数据");
    assert_eq!(outcome.book.created_by.as_deref(), Some("admin"));
    assert!(outcome.file.is_none());
    assert!(outcome.series.is_none());
    let entry = library
        .get_book_entry(&outcome.book.id, &user)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(entry.book).unwrap(),
        serde_json::to_value(outcome.book).unwrap()
    );
    assert!(entry.files.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn series_by_ids_preserves_order_duplicates_and_missing_ids_across_batches() {
    let (library, _, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO series (id, title)
         SELECT printf('series-%04d', i), '系列' FROM n;",
        )
        .await
        .unwrap();
    let ids: Vec<_> = (0..=500).map(|i| format!("series-{i:04}")).collect();
    let requested: Vec<_> = ["missing-first"]
        .into_iter()
        .chain(ids.iter().rev().map(String::as_str))
        .chain(["missing-middle", "series-0500"])
        .chain(std::iter::repeat_n("series-0000", 1000))
        .chain(["missing-last"])
        .collect();
    let queries = count_selects(&library.diesel_db).await;
    let series = library
        .series_by_ids(requested.iter().copied())
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 4);
    assert_eq!(series.len(), requested.len());
    for (id, row) in requested.into_iter().zip(series) {
        assert_eq!(
            row.as_ref().map(|series| series.id.as_str()),
            (!id.starts_with("missing-")).then_some(id),
        );
    }
    let mut empty = std::pin::pin!(library.series_by_ids([]));
    assert!(empty.try_next().await.unwrap().is_none());
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn files_of_books_preserves_order_visibility_and_duplicates_across_batches() {
    let (library, _, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO books (id, title) SELECT printf('book-%04d', i), '书籍' FROM n;
         INSERT INTO book_files (id, book_id, source, external_id, visibility, created_at)
         SELECT 'file-' || id, id, 'local', 'file-' || id, 'public', '2024-01-01T00:00:00Z'
         FROM books WHERE id LIKE 'book-%' AND id != 'book-0250';
         INSERT INTO book_files (id, book_id, source, external_id, visibility, created_at)
         VALUES ('newer', 'book-0500', 'local', 'newer', 'public', '2025-01-01T00:00:00Z'),
                ('hidden', 'book-0000', 'local', 'hidden', 'private', '2025-01-01T00:00:00Z');",
        )
        .await
        .unwrap();
    let user = User {
        id: "viewer".into(),
        username: "读者".into(),
        role: Role::User,
        created_at: chrono::Utc::now(),
    };
    let ids: Vec<_> = (0..=500).map(|i| format!("book-{i:04}")).collect();
    let queries = count_selects(&library.diesel_db).await;
    let groups = library
        .files_of_books(
            ["missing-first"]
                .into_iter()
                .chain(ids.iter().rev().map(|id: &String| id.as_str()))
                .chain(["missing-last"]),
            &user,
        )
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(groups.len(), 503);
    assert!(groups[0].is_empty());
    assert!(groups[502].is_empty());
    for (id, files) in ids.iter().rev().zip(&groups[1..502]) {
        assert!(files.iter().all(|file| &file.book_id == id));
        assert_eq!(
            files.len(),
            if id == "book-0250" {
                0
            } else if id == "book-0500" {
                2
            } else {
                1
            }
        );
    }
    assert_eq!(groups[1][0].id, "newer");
    assert_eq!(groups[1][1].id, "file-book-0500");
    assert_eq!(groups[501][0].id, "file-book-0000");
    let expected = [&groups[1], &groups[501], &groups[251], &groups[0]];
    let repeated = library
        .files_of_books(
            ["book-0500", "book-0000", "book-0250", "missing"]
                .into_iter()
                .cycle()
                .take(QUERY_BATCH_SIZE + expected.len()),
            &user,
        )
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(repeated.len(), QUERY_BATCH_SIZE + expected.len());
    for (index, files) in repeated.iter().enumerate() {
        assert!(
            files
                .iter()
                .map(|file| &file.id)
                .eq(expected[index % expected.len()].iter().map(|file| &file.id))
        );
    }
    let mut empty = std::pin::pin!(library.files_of_books([], &user));
    assert!(empty.try_next().await.unwrap().is_none());
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn book_entries_share_one_read_model_and_reuse_the_resolved_series() {
    let (library, file, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "INSERT INTO series (id, title) VALUES ('parent', '系列');
         UPDATE books SET series_id = 'parent';
         INSERT INTO books (id, title) VALUES ('empty', '独立元数据');",
        )
        .await
        .unwrap();
    let user = User {
        id: "admin".into(),
        username: "管理员".into(),
        role: Role::Admin,
        created_at: chrono::Utc::now(),
    };
    let queries = count_selects(&library.diesel_db).await;
    let entry = library.get_book_entry(&file.book_id, &user).await.unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
    assert_eq!(entry.files.len(), 1);
    assert_eq!(entry.files[0].id, file.id);
    let expected = serde_json::to_value(&entry).unwrap();
    let entries = library.list_books(None, None, &user).await.unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
    assert_eq!(entries.len(), 2);
    assert_eq!(serde_json::to_value(&entries[0]).unwrap(), expected);
    assert_eq!(entries[1].book.id, "empty");
    assert!(entries[1].files.is_empty());
    assert!(entries[1].series.is_none());

    let series = entry.series.as_ref().unwrap();
    let members = library.list_series_books(series, &user).await.unwrap();
    assert_eq!(
        queries.swap(0, Ordering::SeqCst),
        2,
        "reuse the parent without querying it again"
    );
    assert_eq!(members.len(), 1);
    assert_eq!(serde_json::to_value(&members[0]).unwrap(), expected);

    let empty = library.get_book_entry("empty", &user).await.unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(
        serde_json::to_value(empty).unwrap(),
        serde_json::to_value(&entries[1]).unwrap()
    );
    let viewer = User {
        role: Role::User,
        ..user
    };
    assert!(matches!(
        library.get_book_entry("empty", &viewer).await,
        Err(ApiError::NotFound(_))
    ));
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn list_books_aligns_streams_across_batches_and_propagates_errors() {
    let (library, _, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "INSERT INTO series (id, title) VALUES ('first', '系列一'), ('last', '系列二');
         WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO books (id, title, series_id, volume_no)
         SELECT printf('book-%04d', i), '批次书籍', CASE WHEN i < 500 THEN 'first' ELSE 'last' END, i FROM n;
         INSERT INTO books (id, title, created_at)
         VALUES ('standalone', '批次独立书籍', '2025-01-01T00:00:00Z'),
                ('empty', '批次独立元数据', '2024-01-01T00:00:00Z');
         INSERT INTO book_files (id, book_id, source, external_id, visibility)
         SELECT 'file-' || id, id, 'local', 'file-' || id, 'public'
         FROM books WHERE title LIKE '批次%' AND id != 'empty';",
        )
        .await
        .unwrap();
    let user = User {
        id: "admin".into(),
        username: "管理员".into(),
        role: Role::Admin,
        created_at: chrono::Utc::now(),
    };
    let queries = count_selects(&library.diesel_db).await;
    let entries = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        library.list_books(Some("批次"), None, &user),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 5);
    assert_eq!(entries.len(), 503);
    for (index, entry) in entries[..501].iter().enumerate() {
        assert_eq!(entry.book.id, format!("book-{index:04}"));
        assert_eq!(entry.files.len(), 1);
        assert_eq!(entry.files[0].book_id, entry.book.id);
        assert_eq!(
            entry.series.as_ref().unwrap().id,
            if index < 500 { "first" } else { "last" }
        );
    }
    assert_eq!(entries[501].book.id, "standalone");
    assert_eq!(entries[501].files.len(), 1);
    assert!(entries[501].series.is_none());
    assert_eq!(entries[502].book.id, "empty");
    assert!(entries[502].files.is_empty());
    assert!(entries[502].series.is_none());

    let standalone = library.list_books(Some("独立"), None, &user).await.unwrap();
    assert_eq!(standalone.len(), 2);
    assert!(standalone.iter().all(|entry| entry.series.is_none()));
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert!(
        library
            .list_books(Some("无匹配"), None, &user)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(queries.swap(0, Ordering::SeqCst), 1);

    // Decoding failures in either later batch must not return a partial list.
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute("UPDATE series SET created_at = 'invalid' WHERE id = 'last';")
        .await
        .unwrap();
    assert!(library.list_books(Some("批次"), None, &user).await.is_err());
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "UPDATE series SET created_at = '2024-01-01T00:00:00Z';
         UPDATE book_files SET created_at = 'invalid' WHERE book_id = 'book-0500';",
        )
        .await
        .unwrap();
    assert!(library.list_books(Some("批次"), None, &user).await.is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn list_books_preserves_visibility_and_attaches_all_visible_sources() {
    let (library, _, dir) = download_fixture().await;
    library.diesel_db.get().await.unwrap().batch_execute(
        "INSERT INTO users (id, username, password_hash) VALUES ('viewer', '读者', '');
         INSERT INTO books (id, title, created_by, created_at)
         VALUES ('mixed', '筛选书籍', 'viewer', '2025-01-01T00:00:00Z'),
                ('empty', '筛选元数据', 'viewer', '2024-01-01T00:00:00Z'),
                ('hidden', '筛选隐藏书籍', 'viewer', '2023-01-01T00:00:00Z'),
                ('foreign-empty', '筛选他人元数据', NULL, '2022-01-01T00:00:00Z');
         INSERT INTO book_files (id, book_id, source, external_id, visibility, owner_id, created_at)
         VALUES ('public', 'mixed', 'source-a', 'public', 'public', NULL, '2024-01-01T00:00:00Z'),
                ('owned', 'mixed', 'source-b', 'owned', 'private', 'viewer', '2025-01-01T00:00:00Z'),
                ('secret', 'mixed', 'source-c', 'secret', 'private', NULL, '2026-01-01T00:00:00Z'),
                ('hidden-file', 'hidden', 'source-c', 'hidden-file', 'private', NULL, '2024-01-01T00:00:00Z');",
    ).await.unwrap();
    let user = User {
        id: "viewer".into(),
        username: "读者".into(),
        role: Role::User,
        created_at: chrono::Utc::now(),
    };
    let entries = library.list_books(Some("筛选"), None, &user).await.unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.book.id.as_str())
            .collect::<Vec<_>>(),
        ["mixed", "empty"]
    );
    assert!(entries[1].files.is_empty());
    for source in ["source-a", "source-b"] {
        let entries = library
            .list_books(Some("筛选"), Some(source), &user)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].book.id, "mixed");
        assert_eq!(
            entries[0]
                .files
                .iter()
                .map(|file| file.id.as_str())
                .collect::<Vec<_>>(),
            ["owned", "public"]
        );
    }
    assert!(
        library
            .list_books(Some("筛选"), Some("source-c"), &user)
            .await
            .unwrap()
            .is_empty()
    );
    let admin = User {
        role: Role::Admin,
        ..user
    };
    let entries = library
        .list_books(Some("筛选"), Some("source-c"), &admin)
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].book.id, "mixed");
    assert_eq!(entries[0].files.len(), 3);
    assert_eq!(entries[1].book.id, "hidden");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn series_stream_queries_on_demand_and_reports_later_batch_errors() {
    let (library, _, dir) = download_fixture().await;
    let queries = count_selects(&library.diesel_db).await;
    let consumed = Arc::new(AtomicUsize::new(0));
    let observed = consumed.clone();
    let ids = std::iter::repeat_n("missing", QUERY_BATCH_SIZE * 2 + 1).inspect(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    });
    {
        let mut series = std::pin::pin!(library.series_by_ids(ids));
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 0);
        assert!(matches!(series.try_next().await.unwrap(), Some(None)));
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);
        // A paused consumer must not keep the only pooled connection checked out.
        let conn = tokio::time::timeout(std::time::Duration::from_secs(1), library.diesel_db.get())
            .await
            .unwrap()
            .unwrap();
        drop(conn);
    }
    assert_eq!(queries.swap(0, Ordering::SeqCst), 1);
    assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);

    {
        let mut series = std::pin::pin!(
            library.series_by_ids(std::iter::repeat_n("missing", QUERY_BATCH_SIZE + 1))
        );
        for _ in 0..QUERY_BATCH_SIZE {
            assert!(matches!(series.try_next().await.unwrap(), Some(None)));
        }
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
        library
            .diesel_db
            .get()
            .await
            .unwrap()
            .batch_execute("DROP TABLE series;")
            .await
            .unwrap();
        assert!(series.try_next().await.is_err());
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 2);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn file_stream_does_not_prefetch_later_batches() {
    let (library, _, dir) = download_fixture().await;
    let user = User {
        id: "viewer".into(),
        username: "读者".into(),
        role: Role::User,
        created_at: chrono::Utc::now(),
    };
    let queries = count_selects(&library.diesel_db).await;
    let consumed = Arc::new(AtomicUsize::new(0));
    let observed = consumed.clone();
    let ids = std::iter::repeat_n("missing", QUERY_BATCH_SIZE + 1).inspect(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    });
    {
        let mut files = std::pin::pin!(library.files_of_books(ids, &user));
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 0);
        assert!(files.try_next().await.unwrap().unwrap().is_empty());
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);
    }
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
    assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn search_annotations_batch_matches_and_preserve_mixed_result_order() {
    let (library, _, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO books (id, title, meta_source, meta_external_id)
         SELECT printf('local-%04d', i), '书籍', 'source-a', printf('external-%04d', i) FROM n;
         INSERT INTO books (id, title, meta_source, meta_external_id)
         VALUES ('other-source', '其他来源', 'source-b', 'missing');",
        )
        .await
        .unwrap();
    let mut entries: Vec<_> = (0..=500)
        .rev()
        .map(|i| book(&format!("external-{i:04}")))
        .collect();
    entries.insert(
        1,
        CatalogEntry::Series(SeriesEntry {
            id: "external-0500".into(),
            title: "系列".into(),
            authors: vec![],
            description: None,
            cover_url: None,
            extra: None,
        }),
    );
    entries.push(book("missing"));
    entries.push(book("external-0500"));
    let mut items: Vec<_> = entries
        .into_iter()
        .map(SearchItem::try_from)
        .collect::<Result<_, _>>()
        .unwrap();
    let queries = count_selects(&library.diesel_db).await;
    let consumed = Arc::new(AtomicUsize::new(0));
    let observed = consumed.clone();
    {
        let ids = items
            .iter()
            .filter(|item| matches!(item.kind, SearchItemKind::Book))
            .map(|item| item.id.as_str())
            .inspect(move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
            });
        let mut matches = std::pin::pin!(library.book_ids_by_source_ids("source-a", ids));
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 0);
        assert!(matches.try_next().await.unwrap().is_some());
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);
        let connection =
            tokio::time::timeout(std::time::Duration::from_secs(1), library.diesel_db.get())
                .await
                .unwrap()
                .unwrap();
        drop(connection);
    }
    assert_eq!(queries.swap(0, Ordering::SeqCst), 1);
    let matching_rows = library
        .book_ids_by_source_ids(
            "source-a",
            items
                .iter()
                .filter(|item| matches!(item.kind, SearchItemKind::Book))
                .map(|item| item.id.as_str()),
        )
        .try_fold(0, |count, _| async move { Ok(count + 1) })
        .await
        .unwrap();
    assert_eq!(
        matching_rows, 502,
        "the query layer must not deduplicate IDs across batches"
    );
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    library
        .annotate_search_items("source-a", &mut items)
        .await
        .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(items.len(), 504);
    assert!(matches!(items[0].kind, SearchItemKind::Book));
    assert_eq!(items[0].id, "external-0500");
    assert_eq!(items[0].book_id.as_deref(), Some("local-0500"));
    assert!(matches!(items[1].kind, SearchItemKind::Series));
    assert_eq!(items[1].book_id, None);
    assert_eq!(items[2].book_id.as_deref(), Some("local-0499"));
    assert_eq!(items[502].book_id, None);
    assert_eq!(items[503].book_id.as_deref(), Some("local-0500"));
    let mut repeated: Vec<_> = (0..=QUERY_BATCH_SIZE * 2)
        .map(|_| SearchItem::try_from(book("external-0500")).unwrap())
        .collect();
    library
        .annotate_search_items("source-a", &mut repeated)
        .await
        .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 1);
    assert_eq!(repeated.len(), QUERY_BATCH_SIZE * 2 + 1);
    assert!(
        repeated
            .iter()
            .all(|item| item.book_id.as_deref() == Some("local-0500"))
    );
    library
        .annotate_search_items("source-a", &mut [])
        .await
        .unwrap();
    items.retain(|item| matches!(item.kind, SearchItemKind::Series));
    items[0].book_id = Some("stale".into());
    library
        .annotate_search_items("source-a", &mut items)
        .await
        .unwrap();
    assert_eq!(items[0].book_id, None);
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn image_ids_borrow_html_and_preserve_reference_order_and_duplicates() {
    let html = String::from(
        r#"正文<img src="/api/images/甲"><img src="https://example.com/remote"><img src="/api/images/b"><img src="/api/images/甲"><img src="/api/images/unfinished"#,
    );
    let ids: Vec<&str> = image_ids_in_html(&html).collect();
    assert_eq!(ids, ["甲", "b", "甲"]);
    for id in ids {
        assert!(html.as_bytes().as_ptr_range().contains(&id.as_ptr()));
    }
    assert!(image_ids_in_html("").next().is_none());
    assert!(image_ids_in_html("<p>正文</p>").next().is_none());
    assert!(
        image_ids_in_html(r#"<img src="/api/images/unfinished"#)
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn image_dimensions_stream_queries_lazily_without_cross_batch_deduplication() {
    let (library, _, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO images (id, mime, size, width, height)
         SELECT printf('%064x', i), 'image/png', 0, i + 1, i + 2 FROM n;",
        )
        .await
        .unwrap();
    let queries = count_selects(&library.diesel_db).await;
    let consumed = Arc::new(AtomicUsize::new(0));
    let observed = consumed.clone();
    let source_ids: Vec<_> = (0..=500).chain([0]).map(|i| format!("{i:064x}")).collect();
    let ids = source_ids.iter().map(String::as_str).inspect(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    });
    let dimensions = {
        let mut dimensions = std::pin::pin!(stored_image_dimensions(&library.diesel_db, ids));
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 0);
        let first = dimensions.try_next().await.unwrap().unwrap();
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);
        let mut dimensions = dimensions.try_collect::<Vec<_>>().await.unwrap();
        dimensions.insert(0, first);
        dimensions
    };
    assert_eq!(
        dimensions.len(),
        502,
        "the query layer must not deduplicate across batches"
    );
    let dimensions: HashMap<_, _> = dimensions.into_iter().collect();
    assert_eq!(dimensions.len(), 501);
    assert_eq!(dimensions[&format!("{:064x}", 0)], (1, 2));
    assert_eq!(dimensions[&format!("{:064x}", 500)], (501, 502));
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 502);
    let html = format!(r#"<img src="/api/images/{:064x}">"#, 0).repeat(QUERY_BATCH_SIZE * 2 + 1);
    let annotated = with_stored_dims(&library.diesel_db, &html).await;
    assert_eq!(
        queries.swap(0, Ordering::SeqCst),
        1,
        "the caller deduplicates references before querying"
    );
    assert_eq!(
        annotated.matches("width=\"1\" height=\"2\"").count(),
        QUERY_BATCH_SIZE * 2 + 1
    );
    assert_eq!(
        with_stored_dims(&library.diesel_db, "<p>正文</p>").await,
        "<p>正文</p>"
    );
    let mut empty = std::pin::pin!(stored_image_dimensions(&library.diesel_db, []));
    assert!(empty.try_next().await.unwrap().is_none());
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn cached_book_batches_shared_images_without_reordering_chapters() {
    let (library, mut file, dir) = download_fixture().await;
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO images (id, mime, size, width, height)
         SELECT printf('%064x', i), 'image/png', 0, i + 1, i + 2 FROM n;",
        )
        .await
        .unwrap();
    let html: String = (0..=500)
        .map(|i| format!("<img src=\"/api/images/{i:064x}\">"))
        .collect();
    let content = [html.as_str(), "", "<p>末章</p>", html.as_str()];
    diesel::delete(schema::chapters::table.filter(schema::chapters::file_id.eq(&file.id)))
        .execute(&mut library.diesel_db.get().await.unwrap())
        .await
        .unwrap();
    for (idx, content) in content.into_iter().enumerate().rev() {
        diesel::insert_into(schema::chapters::table)
            .values((
                schema::chapters::file_id.eq(&file.id),
                ChapterIndex { idx: idx as u32 },
                schema::chapters::title.eq(format!("第{}章", idx + 1)),
                schema::chapters::content.eq(content),
            ))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
    }
    file.chapter_count = 4;
    let queries = count_selects(&library.diesel_db).await;
    let result = library.load_book_file(&file).await.unwrap();
    assert_eq!(
        queries.swap(0, Ordering::SeqCst),
        3,
        "one chapter query and two image batches"
    );
    let crate::service::downloads::DownloadedContent::Book(chapters) = result else {
        panic!("expected a whole cached book");
    };
    assert_eq!(
        chapters.iter().map(|c| c.idx).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(chapters[0].content, chapters[3].content);
    assert_eq!(chapters[0].content.matches("width=").count(), 501);
    assert!(chapters[0].content.contains("width=\"501\" height=\"502\""));
    assert_eq!(chapters[1].content, "");
    assert_eq!(chapters[2].content, "<p>末章</p>");
    with_stored_dims_many(&library.diesel_db, &mut []).await;
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute("DROP TABLE images;")
        .await
        .unwrap();
    let mut chapters = [
        Chapter {
            idx: 0,
            title: "首章".into(),
            content: html.clone(),
        },
        Chapter {
            idx: 1,
            title: "末章".into(),
            content: String::new(),
        },
    ];
    with_stored_dims_many(&library.diesel_db, &mut chapters).await;
    assert_eq!(chapters[0].content, html);
    assert_eq!(chapters[1].content, "");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn plugin_collections_keep_creation_order_and_skip_disabled_catalogs() {
    let (library, _, dir) = download_fixture().await;
    let plugins = library.plugins();
    for id in ["last", "first"] {
        plugins.register(id, "fixture.wasm").await.unwrap();
        plugins
            .set_config(id, serde_json::json!({"site-name": "示例"}))
            .await
            .unwrap();
    }
    plugins.set_enabled("last", false).await.unwrap();
    library
        .diesel_db
        .get()
        .await
        .unwrap()
        .batch_execute(
            "UPDATE plugin_instances SET created_at = '2024-01-02T00:00:00Z' WHERE id = 'fixture';
         UPDATE plugin_instances SET created_at = '2024-01-01T00:00:00Z' WHERE id = 'first';
         UPDATE plugin_instances SET created_at = '2024-01-03T00:00:00Z' WHERE id = 'last';
         INSERT INTO plugin_instances (id, wasm_file, created_at)
         VALUES ('missing', 'missing.wasm', '2024-01-04T00:00:00Z');",
        )
        .await
        .unwrap();
    let infos = plugins.infos().await.unwrap();
    assert_eq!(
        infos
            .iter()
            .map(|info| info.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "fixture", "last", "missing"]
    );
    assert_eq!(infos[3].name, "missing.wasm");
    let catalogs = plugins.declared_catalogs().await.unwrap();
    assert_eq!(
        catalogs
            .iter()
            .map(|catalog| catalog.instance.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "fixture"]
    );
    assert_eq!(
        plugins
            .declared_catalog("first")
            .await
            .unwrap()
            .unwrap()
            .books
            .len(),
        catalogs[0].books.len()
    );
    for id in ["last", "missing", "unknown"] {
        assert!(plugins.declared_catalog(id).await.unwrap().is_none());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
