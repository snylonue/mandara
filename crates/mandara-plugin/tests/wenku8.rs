//! Build first: cargo build -p wenku8-plugin --release --target wasm32-unknown-unknown,
//! then package with wasm-tools component new into plugins-built/wenku8.wasm.
//! Run: cargo test -p mandara-plugin --test wenku8 -- --ignored.

use std::sync::{Arc, Mutex};

use mandara_plugin::host::{validate_config, values_from_config};
use mandara_plugin::{FetchPolicy, ImageStore, ImageStoreError, WasmPlugin};
use serde_json::json;

mod common;

#[derive(Default)]
struct Images(Mutex<Vec<Vec<u8>>>);

impl ImageStore for Images {
    fn store_image(&self, bytes: &[u8], mime: &str) -> Result<String, ImageStoreError> {
        assert_eq!(mime, "image/png");
        self.0.lock().unwrap().push(bytes.to_vec());
        Ok(format!("{:064x}", bytes.last().unwrap()))
    }
}

#[test]
#[ignore = "requires a freshly built plugins-built/wenku8.wasm"]
fn compiled_guest_fetches_only_referenced_prose_images_and_all_plate_images() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let server = common::MockServer::start(move |target, method, headers, _| {
        assert_eq!(method, "GET");
        recorded.lock().unwrap().push(target.to_string());
        let html = match target {
            "/modules/article/reader.php?aid=1" => r#"<td class="vcss">第一卷</td>
                <td class="ccss"><a href="reader.php?cid=10">序章</a></td>
                <td class="ccss"><a href="reader.php?cid=11">第一章</a></td>
                <td class="ccss"><a href="reader.php?cid=12">插图</a></td>"#
                .to_string(),
            "/modules/article/reader.php?aid=1&cid=10" => {
                r#"<div id="content">正文没有插图。</div>"#.to_string()
            }
            "/modules/article/reader.php?aid=1&cid=11" => {
                r#"<div id="content">前文（插图003）后文<br/>（插图003）</div>"#.to_string()
            }
            "/modules/article/reader.php?aid=1&cid=12" => {
                let host = &headers.iter().find(|(k, _)| k == "host").unwrap().1;
                let plates: String = (1..=3).map(|i| format!(
                    r#"<div class="divimage"><img src="http://{host}/image/{i}.png"/></div>"#,
                )).collect();
                format!(r#"<div id="content">{plates}</div>"#)
            }
            path if path.starts_with("/image/") => {
                let number: u8 = path
                    .trim_start_matches("/image/")
                    .trim_end_matches(".png")
                    .parse()
                    .unwrap();
                let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
                bytes.push(number);
                return (200, vec![], bytes);
            }
            _ => return (404, vec![], vec![]),
        };
        let (bytes, _, _) = encoding_rs::GBK.encode(&html);
        (200, vec![], bytes.into_owned())
    });
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins-built/wenku8.wasm"
    ))
    .expect("build the Wenku8 component first");
    let images = Arc::new(Images::default());
    let plugin = WasmPlugin::load(
        "wenku8.wasm",
        bytes,
        Arc::new(FetchPolicy::default()),
        images.clone(),
    )
    .unwrap();
    let schema = plugin.config_schema().unwrap();
    let normalized = validate_config(&schema, &json!({"base-url": server.url("")})).unwrap();
    let config = values_from_config(&schema, &normalized);

    let chapter = plugin
        .get_chapter(&config, "wenku8:volume:1:10", 0)
        .unwrap()
        .unwrap();
    assert!(chapter.content.contains("正文没有插图。"));
    assert_eq!(
        *requests.lock().unwrap(),
        [
            "/modules/article/reader.php?aid=1",
            "/modules/article/reader.php?aid=1&cid=10",
        ]
    );
    assert!(images.0.lock().unwrap().is_empty());

    requests.lock().unwrap().clear();
    let chapter = plugin
        .get_chapter(&config, "wenku8:volume:1:10", 1)
        .unwrap()
        .unwrap();
    assert_eq!(chapter.format, "html");
    assert_eq!(
        chapter
            .content
            .matches(&format!("/api/images/{:064x}", 3))
            .count(),
        2
    );
    assert_eq!(
        *requests.lock().unwrap(),
        [
            "/modules/article/reader.php?aid=1",
            "/modules/article/reader.php?aid=1&cid=11",
            "/modules/article/reader.php?aid=1&cid=12",
            "/image/3.png",
        ]
    );
    assert_eq!(images.0.lock().unwrap().len(), 1);

    requests.lock().unwrap().clear();
    let chapter = plugin
        .get_chapter(&config, "wenku8:volume:1:10", 2)
        .unwrap()
        .unwrap();
    assert_eq!(chapter.content.matches("<figure>").count(), 3);
    assert_eq!(
        *requests.lock().unwrap(),
        [
            "/modules/article/reader.php?aid=1",
            "/modules/article/reader.php?aid=1&cid=12",
            "/image/1.png",
            "/image/2.png",
            "/image/3.png",
        ]
    );
    assert_eq!(images.0.lock().unwrap().len(), 4);
}
