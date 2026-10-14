//! Host unit tests against the checked-in `hello` component fixture.
//!
//! Rebuild the fixture after changing the WIT or the hello guest:
//! `./scripts/build-plugins.sh hello` (copies `plugins-built/hello.wasm`
//! into `tests/fixtures/`).

use std::sync::Arc;

use mandara_plugin::FetchPolicy;
use mandara_plugin::error::Error;
use mandara_plugin::host::{
    ConfigErrors, ConfigField, ConfigKind, ConfigValue, MAX_SEARCH_LIMIT, WasmPlugin,
    validate_config, values_from_config,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/hello.wasm");

#[derive(Default)]
struct TestState(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl mandara_plugin::StateStore for TestState {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, mandara_plugin::StateStoreError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), mandara_plugin::StateStoreError> {
        self.0.lock().unwrap().insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<bool, mandara_plugin::StateStoreError> {
        Ok(self.0.lock().unwrap().remove(key).is_some())
    }
}

fn state_probe(
    plugin: &WasmPlugin,
    store: Arc<dyn mandara_plugin::StateStore>,
    key: &str,
    index: u32,
    value: &str,
) -> String {
    plugin
        .get_chapter(
            store,
            &[ConfigValue::Text(value.into())],
            &format!("state:{key}"),
            index,
        )
        .unwrap()
        .unwrap()
        .content
}

#[test]
fn state_imports_survive_fresh_guest_calls_and_are_scoped() {
    let compiled = load();
    let state = Arc::new(TestState::default());
    let first = state.clone();
    let second = Arc::new(TestState::default());
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 0, ""),
        "Ok(None)"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 1, "a"),
        "Ok(())"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 0, ""),
        "Ok(Some([97]))"
    );
    assert_eq!(
        state_probe(&compiled, second.clone(), "目录", 0, ""),
        "Ok(None)"
    );
    // A new compiled component sees the same store supplied to each call.
    let compiled = load();
    let first = state;
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 0, ""),
        "Ok(Some([97]))"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 1, "b"),
        "Ok(())"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 0, ""),
        "Ok(Some([98]))"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 4, ""),
        "Ok(())"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 0, ""),
        "Ok(Some([]))"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 2, ""),
        "Ok(true)"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 2, ""),
        "Ok(false)"
    );
    assert_eq!(
        state_probe(&compiled, first.clone(), "目录", 0, ""),
        "Ok(None)"
    );
}

#[test]
fn state_imports_delegate_key_and_value_policy() {
    let compiled = load();
    let state = Arc::new(TestState::default());
    let plugin = state.clone();
    for key in [String::new(), "字".repeat(1024)] {
        assert_eq!(
            state_probe(&compiled, plugin.clone(), &key, 0, ""),
            "Ok(None)"
        );
        assert_eq!(
            state_probe(&compiled, plugin.clone(), &key, 1, "a"),
            "Ok(())"
        );
        assert_eq!(
            state_probe(&compiled, plugin.clone(), &key, 0, ""),
            "Ok(Some([97]))"
        );
        assert_eq!(
            state_probe(&compiled, plugin.clone(), &key, 2, ""),
            "Ok(true)"
        );
    }
    // The host forwards values without imposing the SQLite backend's cap.
    assert_eq!(
        state_probe(&compiled, plugin.clone(), "key", 3, ""),
        "Ok(())"
    );
    assert_eq!(state.0.lock().unwrap()["key"].len(), 256 * 1024 + 1);
}

struct FailingState(bool);
impl FailingState {
    fn error(&self) -> mandara_plugin::StateStoreError {
        if self.0 {
            mandara_plugin::StateStoreError::QuotaExceeded
        } else {
            anyhow::anyhow!("private backend failure details").into()
        }
    }
}
impl mandara_plugin::StateStore for FailingState {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>, mandara_plugin::StateStoreError> {
        Err(self.error())
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), mandara_plugin::StateStoreError> {
        Err(self.error())
    }
    fn delete(&self, _: &str) -> Result<bool, mandara_plugin::StateStoreError> {
        Err(self.error())
    }
}

#[test]
fn state_backend_errors_are_redacted_and_quota_is_reported() {
    let compiled = load();
    for (quota, message) in [(false, "storage unavailable"), (true, "quota exceeded")] {
        let plugin = Arc::new(FailingState(quota));
        for operation in [0, 1, 2] {
            let result = state_probe(&compiled, plugin.clone(), "key", operation, "value");
            assert!(result.starts_with("Err("));
            assert!(result.contains(message));
            assert!(!result.contains("private backend"));
        }
    }
}

fn state() -> Arc<dyn mandara_plugin::StateStore> {
    Arc::new(TestState::default())
}

/// No-op image store for tests: records calls, returns a fake id.
#[derive(Default)]
struct TestImages {
    count: std::sync::atomic::AtomicUsize,
}

impl mandara_plugin::host::ImageStore for TestImages {
    fn store_image(
        &self,
        bytes: &[u8],
        _mime: &str,
    ) -> Result<String, mandara_plugin::ImageStoreError> {
        let n = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(format!("{:064x}", bytes.len() as u128 + n as u128))
    }
}

fn load() -> Arc<WasmPlugin> {
    Arc::new(
        WasmPlugin::load(
            "hello.wasm",
            FIXTURE.to_vec(),
            Arc::new(FetchPolicy::default()),
            Arc::new(TestImages::default()),
        )
        .expect("fixture loads"),
    )
}

#[test]
fn fixture_is_valid() {
    let plugin = load();
    assert_eq!(plugin.file(), "hello.wasm");
    assert_eq!(plugin.name(state()).unwrap(), "hello");
}

#[test]
fn capabilities_stub_declares_supported_patterns() {
    let plugin = load();
    let caps = plugin.capabilities(state()).unwrap();
    assert_eq!(caps, vec!["declare", "identify", "content"]);
}

#[test]
fn config_schema_roundtrip() {
    let plugin = load();
    let fields = plugin.config_schema(state()).unwrap();
    let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, vec!["site-name", "mode", "verbose", "spin", "tags"]);
    let site = &fields[0];
    assert!(matches!(site.kind, ConfigKind::Text));
    assert!(site.required);
    assert_eq!(site.default.as_deref(), Some("Mandara 示例库"));
    assert!(matches!(fields[1].kind, ConfigKind::EnumOptions(ref o) if o == &["default", "alt"]));
    assert!(matches!(fields[3].kind, ConfigKind::Boolean));
    assert!(matches!(fields[4].kind, ConfigKind::ListOfString));
}

#[test]
fn config_validation_accepts_defaults_and_rejects_bad_values() {
    let plugin = load();
    let fields = plugin.config_schema(state()).unwrap();

    // Empty config -> healthy defaults (a default satisfies `required`).
    let normalized = validate_config(&fields, &serde_json::json!({})).unwrap();
    assert_eq!(normalized["site-name"], "Mandara 示例库");
    assert_eq!(normalized["spin"], false);

    // A required field *without* a default must be provided.
    let synthetic = vec![ConfigField {
        key: "token".into(),
        label: "Token".into(),
        kind: ConfigKind::Text,
        default: None,
        required: true,
        hint: None,
    }];
    let err = validate_config(&synthetic, &serde_json::json!({})).unwrap_err();
    assert_eq!(err.0[0].field, "token");
    let unsaved = values_from_config(&synthetic, &serde_json::json!({}));
    assert!(matches!(&unsaved[0], ConfigValue::Text(value) if value.is_empty()));
    let defaults = values_from_config(&fields, &serde_json::json!({}));
    assert!(matches!(&defaults[0], ConfigValue::Text(value) if value == "Mandara 示例库"));

    // Unknown key + wrong types + out-of-range enum index.
    let err = validate_config(
        &fields,
        &serde_json::json!({
            "site-name": 42,
            "mode": 9,
            "tags": "not-a-list",
            "bogus": true,
        }),
    )
    .unwrap_err();
    let by_field: Vec<(&str, &str)> = err
        .0
        .iter()
        .map(|e| (e.field.as_str(), e.message.as_str()))
        .collect();
    assert!(by_field.contains(&("site-name", "expected string")));
    assert!(by_field.contains(&("mode", "must be an index in 0..2 (enum options)")));
    assert!(by_field.contains(&("tags", "expected array of strings")));
    assert!(by_field.contains(&("bogus", "unknown config key")));

    // Full valid config -> normalized (defaults filled in, unknown keys
    // gone).
    let normalized = validate_config(
        &fields,
        &serde_json::json!({
            "site-name": "我的书库",
            "mode": 1,
            "verbose": true,
            "tags": ["a", "b"],
        }),
    )
    .unwrap();
    assert_eq!(normalized["site-name"], "我的书库");
    assert_eq!(normalized["mode"], 1);
    assert_eq!(normalized["verbose"], true);
    assert_eq!(normalized["tags"], serde_json::json!(["a", "b"]));
    // `spin` was missing but optional -> default applied.
    assert_eq!(normalized["spin"], false);

    let values = values_from_config(
        &fields,
        &validate_config(
            &fields,
            &serde_json::json!({ "site-name": "我的书库", "mode": 1, "tags": ["a"] }),
        )
        .unwrap(),
    );
    assert_eq!(values.len(), 5);
    assert!(matches!(&values[0], ConfigValue::Text(s) if s == "我的书库"));
    assert!(matches!(values[1], ConfigValue::EnumIndex(1)));
    assert!(matches!(values[2], ConfigValue::Boolean(false)));
    assert!(matches!(&values[4], ConfigValue::StringList(l) if l == &["a".to_string()]));
}

#[test]
fn config_is_injected_into_calls() {
    let plugin = load();
    let fields = plugin.config_schema(state()).unwrap();
    let config = validate_config(
        &fields,
        &serde_json::json!({ "site-name": "测试站点", "tags": ["tag1", "tag2"] }),
    )
    .unwrap();
    let values = values_from_config(&fields, &config);

    // The declared catalog is shaped by the injected configuration.
    let declared = plugin
        .declare(state(), &values)
        .unwrap()
        .expect("some books");
    assert_eq!(declared.len(), 2);
    let first = &declared[0];
    assert_eq!(first.book.id, "hello-1");
    let content = first.content.as_ref().unwrap();
    assert_eq!(content.id, "hello-content-1");
    assert_ne!(content.id, first.book.id);
    assert!(matches!(
        content.mode,
        mandara_plugin::ContentMode::Chapters
    ));
    let loaded = plugin
        .get_content(state(), &values, &content.id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.id, content.id);
    assert_eq!(loaded.mode, mandara_core::model::ContentMode::Chapters);
    assert_eq!(loaded.chapters.len(), 2);
    assert!(
        loaded
            .chapters
            .iter()
            .all(|chapter| chapter.content.is_none())
    );
    assert!(
        plugin
            .get_content(state(), &values, &first.book.id)
            .unwrap()
            .is_none()
    );
    assert!(first.book.title.contains("测试站点"));
    assert!(
        first
            .book
            .description
            .as_deref()
            .unwrap()
            .contains("tag1、tag2")
    );
    assert_eq!(content.chapters.len(), 2);
    assert!(
        content.chapters[0]
            .content
            .as_deref()
            .unwrap()
            .contains("测试站点")
    );

    // Chapters serve the injected site name and mode variant.
    let ch = plugin
        .get_chapter(state(), &values, "hello-content-1", 0)
        .unwrap()
        .expect("chapter");
    assert!(ch.content.contains("wasm 插件"));
    let alt = values_from_config(
        &fields,
        &validate_config(&fields, &serde_json::json!({ "site-name": "x", "mode": 1 })).unwrap(),
    );
    let ch = plugin
        .get_chapter(state(), &alt, "hello-content-1", 0)
        .unwrap()
        .expect("chapter");
    assert!(ch.title.contains("变体"));

    // Unknown book -> None.
    assert!(
        plugin
            .get_book(state(), &values, "hello-1")
            .unwrap()
            .is_none()
    );
    // Stubbed search returns an empty result.
    let result = plugin
        .search_books(state(), &values, "", 0, MAX_SEARCH_LIMIT)
        .unwrap();
    assert_eq!(result.items.len(), 0);
}

#[test]
fn declared_bodies_preserve_absence_and_empty_content() {
    let plugin = load();
    let declared = plugin.declare(state(), &[]).unwrap().unwrap();
    let content = declared[1].content.as_ref().unwrap();
    let chapters = &content.chapters;
    assert_eq!(chapters.len(), 3);
    assert!(chapters[0].content.is_some());
    assert_eq!(chapters[1].content, None);
    assert_eq!(chapters[2].content.as_deref(), Some(""));
    let declared_content = plugin
        .get_content(state(), &[], &content.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        declared_content.mode,
        mandara_core::model::ContentMode::Chapters
    );
    assert_eq!(declared_content.chapters[1].content, None);
    assert_eq!(declared_content.chapters[2].content.as_deref(), Some(""));
    let empty = plugin
        .get_chapter(state(), &[], &content.id, 2)
        .unwrap()
        .unwrap();
    assert_eq!(empty.content, "");
    let lazy = plugin
        .get_chapter(state(), &[], &content.id, 1)
        .unwrap()
        .unwrap();
    assert!(lazy.content.contains("正文"));
}

#[test]
fn upload_identification() {
    let plugin = load();
    let fields = plugin.config_schema(state()).unwrap();
    let config = validate_config(&fields, &serde_json::json!({ "site-name": "示例库" })).unwrap();
    let values = values_from_config(&fields, &config);
    let hit = plugin
        .identify_upload(state(), &values, "hello-novel.txt", "abc")
        .unwrap();
    assert_eq!(hit.unwrap().id, "hello-1");
    let miss = plugin
        .identify_upload(state(), &values, "其他书名.epub", "abc")
        .unwrap();
    assert!(miss.is_none());
}

#[test]
fn runaway_plugin_is_trapped_by_fuel() {
    // Guest computation is bounded independently of the HTTP timeout.
    let plugin = Arc::new(
        WasmPlugin::load(
            "hello.wasm",
            FIXTURE.to_vec(),
            Arc::new(FetchPolicy {
                timeout_ms: 100,
                ..FetchPolicy::default()
            }),
            Arc::new(TestImages::default()),
        )
        .expect("fixture loads"),
    );
    let fields = plugin.config_schema(state()).unwrap();
    // `spin: true` makes the guest loop forever inside get_chapter.
    let config = validate_config(
        &fields,
        &serde_json::json!({ "site-name": "x", "spin": true }),
    )
    .unwrap();
    let values = values_from_config(&fields, &config);
    let err = plugin
        .get_chapter(state(), &values, "hello-content-1", 0)
        .expect_err("spin must trap");
    match err {
        Error::Runtime(source) => {
            let msg = format!("{source:#}");
            assert!(msg.contains("fuel"), "unexpected message: {msg}");
        }
        other => panic!("expected plugin error, got {other:?}"),
    }
}

#[test]
fn config_errors_are_serializable() {
    let errors = ConfigErrors(vec![mandara_plugin::host::ConfigFieldError {
        field: "site-name".into(),
        message: "required field is missing".into(),
    }]);
    let json = serde_json::to_value(&errors.0).unwrap();
    assert_eq!(json[0]["field"], "site-name");
}
