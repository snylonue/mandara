//! Wenku8 adapts a website series into stable, independently readable books.
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
fn volume_ids_survive_toc_reordering_and_chapter_zero_is_local() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let changed = Arc::new(AtomicBool::new(false));
    let state = changed.clone();
    let mock = common::MockServer::start(move |path, _, _, _| {
        let body = if path == "/book/1.htm" {
            r#"<div id="content"><b>Series</b></div>"#.to_string()
        } else if path.contains("cid=20") {
            r#"<div id="content">Volume Two Body</div>"#.to_string()
        } else if path.contains("reader.php") && !path.contains("cid=") {
            let prefix = if state.load(Ordering::SeqCst) {
                r#"<td class="vcss">New Volume</td><td class="ccss"><a href="reader.php?aid=1&amp;cid=5">New Chapter</a></td>"#
            } else {
                ""
            };
            format!(
                r#"{prefix}<td class="vcss">Volume One</td><td class="ccss"><a href="reader.php?aid=1&amp;cid=10">One</a></td><td class="vcss">Volume Two</td><td class="ccss"><a href="reader.php?aid=1&amp;cid=20">Two</a></td>"#
            )
        } else {
            return (404, vec![], vec![]);
        };
        (200, vec![], body.into_bytes())
    });
    let plugin = WasmPlugin::load(
        "wenku8.wasm",
        std::fs::read("../../plugins-built/wenku8.wasm").expect("build plugins first"),
        Arc::new(FetchPolicy::default()),
        Arc::new(NoImages),
    )
    .unwrap();
    let config = [
        ConfigValue::Text(mock.url("")),
        ConfigValue::Text("".into()),
        ConfigValue::EnumIndex(0),
    ];
    assert!(plugin.get_book(&config, "1").unwrap().is_none());
    let initial = plugin.get_series(&config, "1").unwrap().unwrap();
    let id = initial.books[1].id.clone();
    assert_eq!(id, "wenku8:volume:1:20");
    assert_eq!(plugin.chapter_titles(&config, &id).unwrap(), ["Two"]);
    let chapter = plugin.get_chapter(&config, &id, 0).unwrap().unwrap();
    assert!(chapter.content.contains("Volume Two Body"));
    assert!(plugin.get_chapter(&config, &id, 1).unwrap().is_none());
    changed.store(true, Ordering::SeqCst);
    let volume = plugin.get_book(&config, &id).unwrap().unwrap();
    assert_eq!(volume.id, id);
    assert_eq!(volume.volume_no, 3);
    assert_eq!(plugin.chapter_titles(&config, &id).unwrap(), ["Two"]);
}
