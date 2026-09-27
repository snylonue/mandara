/// No-op image store: the introspection calls never touch images.
struct NoImages;

impl mandara_plugin::host::ImageStore for NoImages {
    fn store_image(&self, _bytes: &[u8], _mime: &str) -> Result<String, String> {
        Err("not available in this test".into())
    }
}

#[test]
fn introspection_is_stable_under_epoch_pump() {
    for file in [
        "../../plugins-built/hello.wasm",
        "../../plugins-built/wiki.wasm",
        "../../plugins-built/reader.wasm",
        "../../plugins-built/wenku8.wasm",
        "../../plugins-built/bangumi.wasm",
    ] {
        let wasm = std::fs::read(file).expect("read");
        let plugin = mandara_plugin::WasmPlugin::load(
            "t.wasm",
            wasm,
            std::sync::Arc::new(mandara_plugin::FetchPolicy::default()),
            std::sync::Arc::new(NoImages),
        )
        .expect("load");
        let mut fails = 0;
        let mut last = String::new();
        for _ in 0..30 {
            let r = plugin.name();
            if r.is_err() {
                fails += 1;
                last = format!("{r:?}");
            }
            let c = plugin.capabilities();
            if c.is_err() && fails < 1 {
                fails += 1;
                last = format!("{c:?}");
            }
            let s = plugin.config_schema();
            if s.is_err() && fails < 1 {
                fails += 1;
                last = format!("{s:?}");
            }
        }
        eprintln!("{file}: {fails} failures, last={last}");
        assert!(fails == 0, "{file} failed {fails} times, last={last}");
    }
}
