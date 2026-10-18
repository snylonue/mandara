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

/// Introspection must return the same answers every time: repeated calls
/// cannot consume the instance's metered fuel budget.
#[test]
fn introspection_is_stable_under_fuel_metering() {
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
    let name = runtime.name(&plugin).unwrap();
    let capabilities = runtime.capabilities(&plugin).unwrap();
    let config_fields = runtime.config_schema(&plugin).unwrap().len();
    for round in 0..10 {
        assert_eq!(runtime.name(&plugin).unwrap(), name, "round {round}");
        assert_eq!(
            runtime.capabilities(&plugin).unwrap(),
            capabilities,
            "round {round}"
        );
        assert_eq!(
            runtime.config_schema(&plugin).unwrap().len(),
            config_fields,
            "round {round}"
        );
    }
}
