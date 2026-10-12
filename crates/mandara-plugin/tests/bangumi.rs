//! Real-series members remain individually addressable over HTTP.
use mandara_plugin::host::ConfigValue;
use mandara_plugin::{FetchPolicy, ImageStore, WasmPlugin};
use std::sync::Arc;
mod common;
struct NoImages;
impl ImageStore for NoImages {
    fn store_image(&self, _: &[u8], _: &str) -> Result<String, mandara_plugin::ImageStoreError> {
        Err(mandara_plugin::ImageStoreError::Backend(Box::new(
            std::io::Error::other("test does not store images"),
        )))
    }
}

#[test]
fn series_members_are_real_books_and_relation_failure_does_not_fabricate_them() {
    let plugin = WasmPlugin::load(
        "bangumi.wasm",
        std::fs::read("../../plugins-built/bangumi.wasm").expect("build plugins first"),
        Arc::new(FetchPolicy::default()),
        Arc::new(NoImages),
    )
    .unwrap();
    for status in [200, 503] {
        let mock = common::MockServer::start(move |path, _, headers, _| {
            assert!(
                headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && v == "Bearer test-token")
            );
            let body = match path {
                "/v0/subjects/1" => {
                    serde_json::json!({"id": 1, "name": "系列", "series": true, "volumes": 50})
                }
                "/v0/subjects/1/subjects" => {
                    if status != 200 {
                        return (status, vec![], vec![]);
                    }
                    serde_json::json!([{"id": 2, "type": 1, "relation": "单行本", "name": "第一卷"}, {"id": 3, "type": 1, "relation": "单行本", "name": "第二卷"}, {"id":4,"type":2,"relation":"动画"}])
                }
                "/v0/subjects/2" => serde_json::json!({"id":2, "name":"第一卷", "series":false}),
                _ => return (404, vec![], vec![]),
            };
            (200, vec![], serde_json::to_vec(&body).unwrap())
        });
        let config = [
            ConfigValue::Text(mock.url("")),
            ConfigValue::Text("test-token".into()),
        ];
        assert!(plugin.get_book(&config, "1").unwrap().is_none());
        let series = plugin.get_series(&config, "1").unwrap();
        if status != 200 {
            assert!(series.is_none());
            continue;
        }
        let series = series.unwrap();
        assert_eq!(series.series.id, "bgm:series:1");
        assert_eq!(
            series.books.len(),
            2,
            "volume count must not create imaginary books"
        );
        assert_eq!(series.books[0].id, "bgm:volume:1:2");
        let volume = plugin
            .get_book(&config, &series.books[0].id)
            .unwrap()
            .unwrap();
        assert_eq!(volume.title, "第一卷");
        assert_eq!(volume.series.unwrap().id, series.series.id);
        assert_eq!(volume.volume_no, 1);
    }
}
