//! Host unit tests against the checked-in `hello` component fixture.
//!
//! Rebuild the fixture after changing the WIT or the hello guest:
//! `./scripts/build-plugins.sh hello` (copies `plugins-built/hello.wasm`
//! into `tests/fixtures/`).

use std::sync::Arc;

use bookshelf_core::error::Error;
use bookshelf_plugin::FetchPolicy;
use bookshelf_plugin::host::{
    ConfigErrors, ConfigField, ConfigKind, ConfigValue, MAX_SEARCH_LIMIT, WasmPlugin,
    validate_config, values_from_config,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/hello.wasm");

fn load() -> Arc<WasmPlugin> {
    Arc::new(
        WasmPlugin::load(
            "hello.wasm",
            FIXTURE.to_vec(),
            Arc::new(FetchPolicy::default()),
        )
        .expect("fixture loads"),
    )
}

#[test]
fn fixture_is_valid() {
    let plugin = load();
    assert_eq!(plugin.file(), "hello.wasm");
    assert_eq!(plugin.name().unwrap(), "hello");
}

#[test]
fn capabilities_stub_declares_supported_patterns() {
    let plugin = load();
    let caps = plugin.capabilities().unwrap();
    assert_eq!(caps, vec!["declare", "identify", "content"]);
}

#[test]
fn config_schema_roundtrip() {
    let plugin = load();
    let fields = plugin.config_schema().unwrap();
    let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, vec!["site-name", "mode", "verbose", "spin", "tags"]);
    let site = &fields[0];
    assert!(matches!(site.kind, ConfigKind::Text));
    assert!(site.required);
    assert_eq!(site.default.as_deref(), Some("Bookshelf 示例库"));
    assert!(matches!(fields[1].kind, ConfigKind::EnumOptions(ref o) if o == &["default", "alt"]));
    assert!(matches!(fields[3].kind, ConfigKind::Boolean));
    assert!(matches!(fields[4].kind, ConfigKind::ListOfString));
}

#[test]
fn config_validation_accepts_defaults_and_rejects_bad_values() {
    let plugin = load();
    let fields = plugin.config_schema().unwrap();

    // Empty config -> healthy defaults (a default satisfies `required`).
    let normalized = validate_config(&fields, &serde_json::json!({})).unwrap();
    assert_eq!(normalized["site-name"], "Bookshelf 示例库");
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
    let fields = plugin.config_schema().unwrap();
    let config = validate_config(
        &fields,
        &serde_json::json!({ "site-name": "测试站点", "tags": ["tag1", "tag2"] }),
    )
    .unwrap();
    let values = values_from_config(&fields, &config);

    // The declared catalog is shaped by the injected configuration.
    let declared = plugin.declare(&values).unwrap().expect("some books");
    assert_eq!(declared.len(), 2);
    let first = &declared[0];
    assert_eq!(first.book.id, "hello-1");
    assert!(first.book.title.contains("测试站点"));
    assert!(
        first
            .book
            .description
            .as_deref()
            .unwrap()
            .contains("tag1、tag2")
    );
    assert_eq!(first.chapters.len(), 2);
    assert!(first.chapters[0].content.contains("测试站点"));

    // Chapters serve the injected site name and mode variant.
    let ch = plugin
        .get_chapter(&values, "hello-1", 0)
        .unwrap()
        .expect("chapter");
    assert!(ch.content.contains("wasm 插件"));
    let alt = values_from_config(
        &fields,
        &validate_config(&fields, &serde_json::json!({ "site-name": "x", "mode": 1 })).unwrap(),
    );
    let ch = plugin
        .get_chapter(&alt, "hello-1", 0)
        .unwrap()
        .expect("chapter");
    assert!(ch.title.contains("变体"));

    // Unknown book -> None.
    assert!(plugin.get_book(&values, "hello-1").unwrap().is_none());
    // Stubbed search returns an empty result.
    let result = plugin
        .search_books(&values, "", 0, MAX_SEARCH_LIMIT)
        .unwrap();
    assert_eq!(result.items.len(), 0);
}

#[test]
fn upload_identification() {
    let plugin = load();
    let fields = plugin.config_schema().unwrap();
    let config = validate_config(&fields, &serde_json::json!({ "site-name": "示例库" })).unwrap();
    let values = values_from_config(&fields, &config);
    let hit = plugin
        .identify_upload(&values, "hello-novel.txt", "abc")
        .unwrap();
    assert_eq!(hit.unwrap().id, "hello-1");
    let miss = plugin
        .identify_upload(&values, "其他书名.epub", "abc")
        .unwrap();
    assert!(miss.is_none());
}

#[test]
fn runaway_plugin_is_trapped_by_epoch_deadline() {
    // A tight fetch-timeout cap keeps the call deadline small (the guard
    // scales with the network budget, see `call_deadline_ticks`), so the
    // spin loop traps after a few pump intervals instead of ~30 s.
    let plugin = Arc::new(
        WasmPlugin::load(
            "hello.wasm",
            FIXTURE.to_vec(),
            Arc::new(FetchPolicy {
                timeout_ms: 100,
                ..FetchPolicy::default()
            }),
        )
        .expect("fixture loads"),
    );
    let fields = plugin.config_schema().unwrap();
    // `spin: true` makes the guest loop forever inside get_chapter.
    let config = validate_config(
        &fields,
        &serde_json::json!({ "site-name": "x", "spin": true }),
    )
    .unwrap();
    let values = values_from_config(&fields, &config);
    let err = plugin
        .get_chapter(&values, "hello-1", 0)
        .expect_err("spin must trap");
    match err {
        Error::Plugin(msg) => {
            assert!(
                msg.contains("interrupt")
                    || msg.contains("epoch")
                    || msg.contains("deadline")
                    || msg.contains("backtrace"),
                "unexpected message: {msg}"
            );
        }
        other => panic!("expected plugin error, got {other:?}"),
    }
}

#[test]
fn config_errors_are_serializable() {
    let errors = ConfigErrors(vec![bookshelf_plugin::host::ConfigFieldError {
        field: "site-name".into(),
        message: "required field is missing".into(),
    }]);
    let json = serde_json::to_value(&errors.0).unwrap();
    assert_eq!(json[0]["field"], "site-name");
}
