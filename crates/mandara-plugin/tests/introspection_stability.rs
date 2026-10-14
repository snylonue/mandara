#[allow(dead_code)]
mod common;

/// No-op image store: the introspection calls never touch images.
struct NoImages;

impl mandara_plugin::host::ImageStore for NoImages {
    fn store_image(
        &self,
        _bytes: &[u8],
        _mime: &str,
    ) -> Result<String, mandara_plugin::ImageStoreError> {
        Err(mandara_plugin::ImageStoreError::Backend(Box::new(
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "not available in this test",
            ),
        )))
    }
}

#[test]
fn introspection_is_stable_under_fuel_metering() {
    {
        let file = "fixture.wasm";
        let wasm = include_bytes!("fixtures/hello.wasm").to_vec();
        let runtime = std::sync::Arc::new(
            mandara_plugin::PluginRuntime::new(
                mandara_plugin::FetchPolicy::default(),
                std::sync::Arc::new(NoImages),
            )
            .unwrap(),
        );
        let compiled = runtime.compile("t.wasm", &wasm).expect("load");
        let plugin = runtime
            .instance("test", compiled, serde_json::json!({}), common::state())
            .unwrap();
        let mut fails = 0;
        let mut last = String::new();
        for _ in 0..30 {
            let r = runtime.name(&plugin);
            if r.is_err() {
                fails += 1;
                last = format!("{r:?}");
            }
            let c = runtime.capabilities(&plugin);
            if c.is_err() && fails < 1 {
                fails += 1;
                last = format!("{c:?}");
            }
            let s = runtime.config_schema(&plugin);
            if s.is_err() && fails < 1 {
                fails += 1;
                last = format!("{s:?}");
            }
        }
        eprintln!("{file}: {fails} failures, last={last}");
        assert!(fails == 0, "{file} failed {fails} times, last={last}");
    }
}
