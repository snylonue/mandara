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
    let server = common::MockServer::start(move |target, method, headers, body| {
        recorded
            .lock()
            .unwrap()
            .push((target.to_string(), headers.to_vec()));
        if target == "/api/v1/auth/login" {
            assert_eq!(method, "POST");
            assert!(!headers.iter().any(|(k, _)| k == "authorization"));
            let login: serde_json::Value = serde_json::from_slice(body).unwrap();
            assert_eq!(login["app"], "n");
            if login["username"] == "bad" {
                return (401, vec![], b"wrong password".to_vec());
            }
            if login["username"] == "invalid-token" {
                return (200, vec![], b"<html>error</html>".to_vec());
            }
            if login["username"] == "server-error" {
                return (503, vec![], b"unavailable".to_vec());
            }
            assert_eq!(login["username"], "test-account");
            assert_eq!(login["password"], " test-password ");
            return (200, vec![], b"test.token.signature".to_vec());
        }
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
            "/api/wenku/series" => json!({
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
    assert!(
        !schema
            .iter()
            .any(|f| ["access-token", "catalog", "volume-files"].contains(&f.key.as_str()))
    );
    let normalized = validate_config(&schema, &json!({"base-url": server.url(""), "auth-url": server.url(""), "providers": ["syosetu"], "username": "test-account", "password": " test-password "})).unwrap();
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
    let before = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| t == "/api/v1/auth/login")
        .count();
    let chapter = plugin
        .get_chapter(&config, "web:syosetu:n1", 0)
        .unwrap()
        .unwrap();
    let after = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(t, _)| t == "/api/v1/auth/login")
        .count();
    assert_eq!(after - before, 1); // Two API GETs and an image share one login.
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

    let first = "wenku-volume:series:%E7%AC%AC%E4%B8%80%E5%8D%B7.txt";
    let before = requests.lock().unwrap().len();
    assert!(
        plugin
            .get_series(&config, "wenku:series")
            .unwrap()
            .is_none()
    );
    assert!(plugin.get_book(&config, first).unwrap().is_none());
    assert!(
        plugin
            .get_book(&config, "wenku-asin:series:B000000001")
            .unwrap()
            .is_none()
    );
    assert_eq!(requests.lock().unwrap().len(), before); // No Wenku metadata lookups.
    let content = plugin.get_content(&config, first).unwrap().unwrap();
    assert_eq!(content.id, first);
    assert!(matches!(
        content.mode,
        mandara_core::model::ContentMode::File
    ));
    let file = plugin.get_book_file(&config, first).unwrap().unwrap();
    assert_eq!(file.filename, "第一卷.txt");
    assert!(file.mime.starts_with("text/plain"));
    assert_eq!(file.bytes, "第一章 开端\n正文".as_bytes());
    assert_eq!(
        plugin
            .get_book_file(&config, "wenku-volume:series:第二卷.epub")
            .unwrap()
            .unwrap()
            .mime,
        "application/epub+zip"
    );
    assert!(
        plugin
            .get_content(&config, "wenku-volume:series:未译.txt")
            .unwrap()
            .is_none()
    );
    assert!(
        plugin
            .get_content(&config, "wenku-volume:series:missing.txt")
            .unwrap()
            .is_none()
    );
    let mut fixed = config.clone();
    fixed[2] = ConfigValue::EnumIndex(4);
    assert!(plugin.get_content(&fixed, first).unwrap().is_none());
    fixed[2] = ConfigValue::EnumIndex(2);
    assert!(
        plugin
            .get_chapter(&fixed, "web:syosetu:n1", 0)
            .unwrap()
            .is_none()
    );

    for account in ["bad", "invalid-token", "server-error"] {
        let mut invalid = config.clone();
        invalid[3] = ConfigValue::Text(account.into());
        let before = requests.lock().unwrap().len();
        assert!(
            plugin
                .search_books(&invalid, "测试", 95, 10)
                .unwrap()
                .items
                .is_empty()
        );
        assert!(plugin.get_book_file(&invalid, first).unwrap().is_none());
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len() - before, 2);
        assert!(
            requests[before..]
                .iter()
                .all(|(target, _)| target == "/api/v1/auth/login")
        );
    }
    let mut incomplete = config.clone();
    incomplete[4] = ConfigValue::Text(String::new());
    let before = requests.lock().unwrap().len();
    assert!(plugin.get_content(&incomplete, first).unwrap().is_none());
    assert_eq!(requests.lock().unwrap().len(), before);

    let requests = requests.lock().unwrap();
    for (target, headers) in requests.iter() {
        let auth = headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str());
        if target == "/image.png" || target == "/api/v1/auth/login" {
            assert_eq!(auth, None);
        } else if target.starts_with("/api/") {
            assert_eq!(auth, Some("Bearer test.token.signature"));
        }
    }
}

#[test]
#[ignore = "calls live Novelia endpoints and downloads an EPUB"]
fn live_guest_reads_chapters_and_volume_id_content() {
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
    let username = std::env::var("NOVELIA_TEST_USERNAME").unwrap_or_default();
    let password = std::env::var("NOVELIA_TEST_PASSWORD").unwrap_or_default();
    let config = values_from_config(
        &schema,
        &validate_config(
            &schema,
            &json!({"username": username.as_str(), "password": password.as_str()}),
        )
        .unwrap(),
    );
    let id = "wenku-volume:6806f70f2d549f092c9ae862:筺底のエルピス5　－迷い子たちの一歩－.epub";
    assert!(
        plugin
            .get_series(&config, "wenku:6806f70f2d549f092c9ae862")
            .unwrap()
            .is_none()
    );
    assert!(plugin.get_book(&config, id).unwrap().is_none());
    assert!(plugin.get_content(&config, id).unwrap().is_some());
    let file = plugin
        .get_book_file(&config, id)
        .unwrap()
        .expect("volumeId resolves to a real EPUB download");
    assert_eq!(file.mime, "application/epub+zip");
    assert!(file.bytes.len() > 1000);
    if !username.is_empty() || !password.is_empty() {
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
