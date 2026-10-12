//! Exercise the built Bangumi component over HTTP, including relation fallback.
//! Build it first with scripts/build-plugins.sh bangumi.

use std::sync::Arc;

use mandara_plugin::FetchPolicy;
use mandara_plugin::host::{ConfigValue, ImageStore, WasmPlugin};

mod common;

struct NoImages;

impl ImageStore for NoImages {
    fn store_image(&self, _bytes: &[u8], _mime: &str) -> Result<String, String> {
        Err("not available in this test".into())
    }
}

#[test]
fn series_lookup_resolves_relations_and_preserves_metadata_on_failure() {
    let plugin = WasmPlugin::load(
        "bangumi.wasm",
        std::fs::read("../../plugins-built/bangumi.wasm").expect("build Bangumi component first"),
        Arc::new(FetchPolicy::default()),
        Arc::new(NoImages),
    )
    .unwrap();
    for (status, body, expected) in [
        (
            200,
            serde_json::to_vec(&serde_json::json!([
                {"id": 2, "type": 1, "relation": "单行本"},
                {"id": 3, "type": 1, "relation": "单行本"},
                {"id": 4, "type": 2, "relation": "动画"},
            ]))
            .unwrap(),
            Some(2),
        ),
        (503, b"unavailable".to_vec(), None),
        (200, b"invalid JSON".to_vec(), None),
    ] {
        let mock = common::MockServer::start(move |path, method, headers, _body| {
            assert_eq!(method, "GET");
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && v == "Bearer test-token")
            );
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "user-agent" && v.contains("mandara"))
            );
            match path {
                "/v0/subjects/1" => (
                    200,
                    vec![],
                    serde_json::to_vec(&serde_json::json!({
                        "id": 1, "name": "驭兽使系列", "series": true, "volumes": 0,
                        "summary": "系列简介", "infobox": [{"key": "作者", "value": "深山鈴"}],
                    }))
                    .unwrap(),
                ),
                "/v0/subjects/1/subjects" => (status, vec![], body.clone()),
                _ => (404, vec![], Vec::new()),
            }
        });
        let book = plugin
            .get_book(
                &[
                    ConfigValue::Text(mock.url("")),
                    ConfigValue::Text("test-token".into()),
                ],
                "1",
            )
            .unwrap()
            .unwrap();
        assert_eq!(book.id, "1");
        assert_eq!(book.title, "驭兽使系列");
        assert_eq!(book.description.as_deref(), Some("系列简介"));
        assert_eq!(book.authors, ["深山鈴"]);
        assert_eq!(book.volumes.as_ref().map(Vec::len), expected);
    }
}
