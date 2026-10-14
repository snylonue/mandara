//! Build first, then run mock tests: cargo test -p mandara-plugin --test novelia compiled_guest -- --ignored.
//! Live validation is opt-in: cargo test -p mandara-plugin --test novelia live_guest -- --ignored.
use std::sync::{Arc, Mutex};

use mandara_plugin::host::{validate_config, values_from_config};
use mandara_plugin::{
    CatalogEntry, ConfigValue, FetchPolicy, ImageStore, ImageStoreError, WasmPlugin,
};
use serde_json::json;

mod common;

#[derive(Default)]
struct Images(Mutex<Vec<Vec<u8>>>);
impl ImageStore for Images {
    fn store_image(&self, bytes: &[u8], mime: &str) -> Result<String, ImageStoreError> {
        assert_eq!(mime, "image/png");
        self.0.lock().unwrap().push(bytes.to_vec());
        Ok("a".repeat(64))
    }
}

#[test]
#[ignore = "requires scripts/build-plugins.sh novelia"]
fn compiled_guest_adapts_web_chapters_and_wenku_downloads() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let server = common::MockServer::start(move |target, method, headers, _| {
        recorded
            .lock()
            .unwrap()
            .push((target.to_string(), headers.to_vec()));
        assert_eq!(method, "GET");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let payload = match path {
            "/api/novel" => {
                assert!(query.contains("pageSize=100"));
                assert!(query.contains("provider=syosetu"));
                assert!(query.contains("query=%E6%B5%8B%E8%AF%95"));
                json!({"pageNumber": 1, "items": [{"providerId": "syosetu", "novelId": "n1", "titleJp": "原书名", "titleZh": "译书名"}]})
            }
            "/api/novel/syosetu/n1" => json!({
                "titleJp": "原书名", "titleZh": "译书名", "authors": [{"name": "作者"}],
                "introductionJp": "简介", "toc": [
                    {"titleJp": "第一部", "chapterId": null},
                    {"titleJp": "序章", "titleZh": "序幕", "chapterId": "1"},
                    {"titleJp": "空章节", "chapterId": "2"}
                ]
            }),
            "/api/novel/syosetu/n1/chapter/1" => {
                // Discover this connection's host from its HTTP Host header.
                let host = headers.iter().find(|(k, _)| k == "host").unwrap().1.clone();
                json!({"titleJp": "序章", "titleZh": "序幕", "paragraphs": ["原文", format!("<图片>http://{host}/image.png")],
                    "gptParagraphs": ["译文 <>&", ""]})
            }
            "/api/novel/syosetu/n1/chapter/2" => json!({"titleJp": "空章节", "paragraphs": []}),
            "/image.png" => {
                return (
                    200,
                    vec![("Content-Type".into(), "image/png".into())],
                    b"\x89PNG\r\n\x1a\nmock".to_vec(),
                );
            }
            "/api/wenku" => {
                json!({"pageNumber": 1, "items": [{"id": "series", "title": "原系列", "titleZh": "译系列", "cover": null}]})
            }
            "/api/wenku/series" => json!({
                "title": "原系列", "titleZh": "译系列", "cover": null, "authors": ["作者"], "artists": ["画师"], "introduction": "简介",
                "volumes": [{"asin": "B000000001", "title": "第一卷"}, {"asin": "B000000002", "title": "第二卷"}, {"asin": "B000000003", "title": "未译"}],
                "volumeJp": [{"volumeId": "第二卷.epub", "sakura": 1}, {"volumeId": "第一卷.txt", "gpt": 1}, {"volumeId": "未译.txt", "gpt": 0}]
            }),
            p if p.starts_with("/api/wenku/series/file/") => {
                assert!(query.contains("mode=zh"));
                assert!(query.contains("translationsMode=priority"));
                let location = if p.ends_with(".epub") {
                    "/download.epub"
                } else {
                    "/download.txt"
                };
                return (302, vec![("Location".into(), location.into())], vec![]);
            }
            "/download.txt" => return (200, vec![], "第一章 开端\n正文".as_bytes().to_vec()),
            "/download.epub" => return (200, vec![], b"PK\x03\x04mock".to_vec()),
            "/api/novel/syosetu/bad-json" => {
                return (200, vec![], b"<html>blocked</html>".to_vec());
            }
            _ => return (404, vec![], vec![]),
        };
        (
            200,
            vec![("Content-Type".into(), "application/json".into())],
            payload.to_string().into_bytes(),
        )
    });
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins-built/novelia.wasm"
    ))
    .expect("build the Novelia component first");
    let images = Arc::new(Images::default());
    let plugin = WasmPlugin::load(
        "novelia.wasm",
        bytes,
        Arc::new(FetchPolicy::default()),
        images.clone(),
    )
    .unwrap();
    assert_eq!(
        plugin.capabilities().unwrap(),
        ["search", "lookup", "content", "book-file"]
    );
    assert_eq!(plugin.source_info().unwrap().kind, "search");
    let schema = plugin.config_schema().unwrap();
    let normalized = validate_config(&schema, &json!({"base-url": server.url(""), "providers": ["syosetu"], "access-token": "test-token"})).unwrap();
    let config = values_from_config(&schema, &normalized);
    let result = plugin.search_books(&config, "测试", 0, 10).unwrap();
    assert_eq!(result.total, 1);
    assert!(matches!(&result.items[0], CatalogEntry::Single(b) if b.id == "web:syosetu:n1"));
    assert_eq!(
        plugin
            .get_book(&config, "web:syosetu:n1")
            .unwrap()
            .unwrap()
            .authors,
        ["作者"]
    );
    let content = plugin
        .get_content(&config, "web:syosetu:n1")
        .unwrap()
        .unwrap();
    assert_eq!(content.chapters.len(), 2);
    assert!(content.chapters.iter().all(|c| c.content.is_none()));
    let chapter = plugin
        .get_chapter(&config, "web:syosetu:n1", 0)
        .unwrap()
        .unwrap();
    assert_eq!(chapter.title, "序幕");
    assert!(chapter.content.contains("译文 &lt;&gt;&amp;"));
    assert!(
        chapter
            .content
            .contains(&format!("/api/images/{}", "a".repeat(64)))
    );
    assert_eq!(images.0.lock().unwrap().len(), 1);
    assert_eq!(
        plugin
            .get_chapter(&config, "web:syosetu:n1", 1)
            .unwrap()
            .unwrap()
            .content,
        ""
    );
    assert!(
        plugin
            .get_chapter(&config, "web:syosetu:n1", 9)
            .unwrap()
            .is_none()
    );
    assert!(
        plugin
            .get_book(&config, "web:syosetu:missing")
            .unwrap()
            .is_none()
    );
    assert!(
        plugin
            .get_book(&config, "web:syosetu:bad-json")
            .unwrap()
            .is_none()
    );

    let mut wenku = config.clone();
    wenku[1] = ConfigValue::EnumIndex(1);
    let result = plugin.search_books(&wenku, "", 0, 10).unwrap();
    assert!(matches!(&result.items[0], CatalogEntry::Series(s) if s.id == "wenku:series"));
    let series = plugin.get_series(&wenku, "wenku:series").unwrap().unwrap();
    assert_eq!(series.books.len(), 3);
    let first = &series.books[0].id;
    assert_eq!(first, "wenku-asin:series:B000000001");
    assert!(
        plugin
            .get_book(&wenku, "wenku-volume:series:第一卷.txt")
            .unwrap()
            .is_none()
    );
    assert_eq!(plugin.get_book(&wenku, first).unwrap().unwrap().id, *first);
    let content = plugin.get_content(&wenku, first).unwrap().unwrap();
    assert!(matches!(
        content.mode,
        mandara_core::model::ContentMode::File
    ));
    let file = plugin.get_book_file(&wenku, first).unwrap().unwrap();
    assert_eq!(file.filename, "第一卷.txt");
    assert!(file.mime.starts_with("text/plain"));
    assert_eq!(file.bytes, "第一章 开端\n正文".as_bytes());
    assert_eq!(
        plugin
            .get_book_file(&wenku, &series.books[1].id)
            .unwrap()
            .unwrap()
            .mime,
        "application/epub+zip"
    );
    assert!(
        plugin
            .get_content(&wenku, &series.books[2].id)
            .unwrap()
            .is_none()
    );
    wenku[3] = ConfigValue::EnumIndex(4);
    assert!(plugin.get_content(&wenku, first).unwrap().is_none());
    let mut fixed = config.clone();
    fixed[3] = ConfigValue::EnumIndex(2);
    assert!(
        plugin
            .get_chapter(&fixed, "web:syosetu:n1", 0)
            .unwrap()
            .is_none()
    );

    let mut mapped = wenku.clone();
    mapped[3] = ConfigValue::EnumIndex(0);
    mapped[5] = ConfigValue::Text(
        r#"[{"novel-id":"series","asin":"B000000001","filename":"第一卷.txt"}]"#.into(),
    );
    assert!(plugin.get_content(&mapped, first).unwrap().is_some());
    mapped[5] = ConfigValue::Text("invalid-json".into());
    assert!(plugin.get_content(&mapped, first).unwrap().is_none());

    let requests = requests.lock().unwrap();
    for (target, headers) in requests.iter() {
        let auth = headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str());
        if target == "/image.png" {
            assert_eq!(auth, None);
        } else if target.starts_with("/api/") {
            assert_eq!(auth, Some("Bearer test-token"));
        }
    }
}

#[test]
#[ignore = "calls live Novelia endpoints and downloads an EPUB"]
fn live_guest_reads_metadata_chapters_and_asin_downloads() {
    struct NoImages;
    impl ImageStore for NoImages {
        fn store_image(&self, _bytes: &[u8], _mime: &str) -> Result<String, ImageStoreError> {
            Ok("a".repeat(64))
        }
    }
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins-built/novelia.wasm"
    ))
    .unwrap();
    let plugin = WasmPlugin::load(
        "novelia.wasm",
        bytes,
        Arc::new(FetchPolicy::default()),
        Arc::new(NoImages),
    )
    .unwrap();
    let schema = plugin.config_schema().unwrap();
    let token = std::env::var("NOVELIA_TEST_ACCESS_TOKEN").unwrap_or_default();
    let mut config = values_from_config(
        &schema,
        &validate_config(
            &schema,
            &json!({"catalog": 1, "access-token": token.as_str()}),
        )
        .unwrap(),
    );
    assert!(
        !plugin
            .search_books(&config, "", 0, 2)
            .unwrap()
            .items
            .is_empty()
    );
    let series = plugin
        .get_series(&config, "wenku:6806f70f2d549f092c9ae862")
        .unwrap()
        .unwrap();
    assert!(
        series
            .books
            .iter()
            .any(|b| b.id == "wenku-asin:6806f70f2d549f092c9ae862:B00R4JX10Q")
    );
    let id = "wenku-asin:6806f70f2d549f092c9ae862:B074W7THKZ";
    assert!(plugin.get_book(&config, id).unwrap().is_some());
    assert!(plugin.get_content(&config, id).unwrap().is_some());
    let file = plugin
        .get_book_file(&config, id)
        .unwrap()
        .expect("ASIN resolves to a real EPUB download");
    assert_eq!(file.mime, "application/epub+zip");
    assert!(file.bytes.len() > 1000);
    if !token.is_empty() {
        config[1] = ConfigValue::EnumIndex(0);
        assert!(
            !plugin
                .search_books(&config, "无职转生", 0, 2)
                .unwrap()
                .items
                .is_empty()
        );
    }
    let web_id = "web:syosetu:n9669bk";
    assert!(plugin.get_book(&config, web_id).unwrap().is_some());
    assert!(
        !plugin
            .get_content(&config, web_id)
            .unwrap()
            .unwrap()
            .chapters
            .is_empty()
    );
    assert!(
        !plugin
            .get_chapter(&config, web_id, 0)
            .unwrap()
            .unwrap()
            .content
            .is_empty()
    );
}
