//! Exercise persistent URL caches in the actual Wenku8 and Novelia components.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mandara_plugin::{FetchPolicy, ImageStore, ImageStoreError, PluginRuntime};
use serde_json::json;

mod common;

#[derive(Default)]
struct Images(Mutex<usize>);
impl ImageStore for Images {
    fn store_image(&self, _: &[u8], mime: &str) -> Result<String, ImageStoreError> {
        assert_eq!(mime, "image/png");
        *self.0.lock().unwrap() += 1;
        Ok("a".repeat(64))
    }
}

fn check_cache(name: &str, id: &str) {
    let requests = Arc::new(AtomicUsize::new(0));
    let invalid = Arc::new(AtomicBool::new(true));
    let hits = requests.clone();
    let fail = invalid.clone();
    let server = common::MockServer::start(move |target, _, headers, _| {
        let host = &headers.iter().find(|(k, _)| k == "host").unwrap().1;
        let image = format!("http://{host}/image.png");
        let body = if target == "/image.png" {
            hits.fetch_add(1, Ordering::SeqCst);
            if fail.load(Ordering::SeqCst) {
                b"not an image".to_vec()
            } else {
                b"\x89PNG\r\n\x1a\nmock".to_vec()
            }
        } else if target.contains("reader.php") {
            if target.contains("cid=") {
                format!(r#"<div id="content"><div class="divimage"><img src="{image}"></div><div class="divimage"><img src="{image}"></div></div>"#).into_bytes()
            } else {
                br#"<td class="vcss">Volume</td><td class="ccss"><a href="reader.php?cid=1">&#25554;&#22270;</a></td><td class="ccss"><a href="reader.php?cid=2">&#25554;&#22270;</a></td>"#.to_vec()
            }
        } else if target.contains("/chapter/") {
            json!({"titleJp": "插图", "paragraphs": [format!("<图片>{image}"), format!("<图片>{image}")]}).to_string().into_bytes()
        } else {
            json!({"titleJp": "测试", "authors": [], "introductionJp": "简介", "toc": [{"titleJp": "插图", "chapterId": "1"}, {"titleJp": "插图", "chapterId": "2"}]}).to_string().into_bytes()
        };
        (200, vec![], body)
    });
    let bytes = std::fs::read(format!(
        "{}/../../plugins-built/{name}.wasm",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("build the guest component first");
    let images = Arc::new(Images::default());
    let runtime = PluginRuntime::new(FetchPolicy::default(), images.clone()).unwrap();
    let compiled = runtime.compile(format!("{name}.wasm"), &bytes).unwrap();
    let state = common::state();
    let config = json!({"base-url": server.url("")});
    let plugin = runtime
        .instance(name, compiled, config.clone(), state.clone())
        .unwrap();
    let failed = runtime.get_chapter(&plugin, id, 0).unwrap().unwrap();
    assert!(failed.content.contains(&server.url("/image.png")));
    assert_eq!(*images.0.lock().unwrap(), 0);
    invalid.store(false, Ordering::SeqCst);
    let before = requests.load(Ordering::SeqCst);
    let first = runtime.get_chapter(&plugin, id, 0).unwrap().unwrap();
    assert!(
        first
            .content
            .contains(&format!("/api/images/{}", "a".repeat(64)))
    );
    runtime.get_chapter(&plugin, id, 1).unwrap().unwrap();
    assert_eq!(requests.load(Ordering::SeqCst) - before, 1);
    assert_eq!(*images.0.lock().unwrap(), 1);
    // A new runtime and registration bound to the same state simulate a restart.
    let runtime = PluginRuntime::new(FetchPolicy::default(), images.clone()).unwrap();
    let compiled = runtime.compile(format!("{name}.wasm"), &bytes).unwrap();
    let restored = runtime
        .instance(name, compiled.clone(), config.clone(), state)
        .unwrap();
    runtime.get_chapter(&restored, id, 0).unwrap().unwrap();
    assert_eq!(requests.load(Ordering::SeqCst) - before, 1);
    let isolated = runtime
        .instance("other", compiled, config, common::state())
        .unwrap();
    runtime.get_chapter(&isolated, id, 0).unwrap().unwrap();
    assert_eq!(requests.load(Ordering::SeqCst) - before, 2);
}

#[test]
#[ignore = "requires built Wenku8 component"]
fn compiled_guest_wenku8_image_cache() {
    check_cache("wenku8", "wenku8:volume:1:1");
}

#[test]
#[ignore = "requires built Novelia component"]
fn compiled_guest_novelia_image_cache() {
    check_cache("novelia", "web:syosetu:n1");
}
