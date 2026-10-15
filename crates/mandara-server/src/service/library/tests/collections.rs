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
            id: "series".into(),
            title: "系列".into(),
            authors: vec![],
            description: None,
            cover_url: None,
            extra: None,
        }),
    );
    entries.push(book("missing"));
    entries.push(book("external-0500"));
    let queries = count_selects(&library.diesel_db).await;
    let items = library
        .annotate_search_items("source-a", entries)
        .await
        .unwrap();
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(items.len(), 504);
    assert!(matches!(&items[0].0, CatalogEntry::Single(b) if b.id == "external-0500"));
    assert_eq!(items[0].1.as_deref(), Some("local-0500"));
    assert!(matches!(&items[1].0, CatalogEntry::Series(s) if s.id == "series"));
    assert_eq!(items[1].1, None);
    assert_eq!(items[2].1.as_deref(), Some("local-0499"));
    assert_eq!(items[502].1, None);
    assert_eq!(items[503].1.as_deref(), Some("local-0500"));
    assert!(
        library
            .annotate_search_items("source-a", vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    let mut invalid = match book("") {
        CatalogEntry::Single(b) => b,
        _ => unreachable!(),
    };
    assert!(
        library
            .annotate_search_items("source-a", vec![CatalogEntry::Single(invalid.clone())])
            .await
            .is_err()
    );
    invalid.id = "valid".into();
    invalid.extra = Some("[]".into());
    assert!(
        library
            .annotate_search_items("source-a", vec![CatalogEntry::Single(invalid)])
            .await
            .is_err()
    );
    assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn image_dimensions_stream_queries_lazily_and_deduplicates_across_batches() {
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
    let ids = (0..=500)
        .chain([0])
        .inspect(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        })
        .map(|i| format!("{i:064x}"));
    let dimensions = {
        let mut dimensions = std::pin::pin!(stored_image_dimensions(&library.diesel_db, ids));
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 0);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 0);
        let first = dimensions.try_next().await.unwrap().unwrap();
        assert_eq!(queries.as_ref().load(Ordering::SeqCst), 1);
        assert_eq!(consumed.as_ref().load(Ordering::SeqCst), QUERY_BATCH_SIZE);
        let mut dimensions = dimensions.try_collect::<HashMap<_, _>>().await.unwrap();
        dimensions.insert(first.0, first.1);
        dimensions
    };
    assert_eq!(dimensions.len(), 501);
    assert_eq!(dimensions[&format!("{:064x}", 0)], (1, 2));
    assert_eq!(dimensions[&format!("{:064x}", 500)], (501, 502));
    assert_eq!(queries.swap(0, Ordering::SeqCst), 2);
    assert_eq!(consumed.as_ref().load(Ordering::SeqCst), 502);
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
