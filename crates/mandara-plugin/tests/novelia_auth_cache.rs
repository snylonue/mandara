//! Build the Novelia component before running these opt-in mock tests.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mandara_plugin::{
    FetchPolicy, ImageStore, ImageStoreError, PluginRuntime, StateStore, StateStoreError,
};
use serde_json::json;

mod common;

struct NoImages;
impl ImageStore for NoImages {
    fn store_image(&self, _: &[u8], _: &str) -> Result<String, ImageStoreError> {
        panic!("metadata must not store images");
    }
}

#[test]
#[ignore = "requires built Novelia component"]
fn compiled_guest_reads_state_between_requests_in_one_export() {
    let state = common::state();
    let shared_state = state.clone();
    let logins = Arc::new(AtomicUsize::new(0));
    let login_count = logins.clone();
    let api = Arc::new(Mutex::new(Vec::new()));
    let calls = api.clone();
    let server = common::MockServer::start(move |target, _, headers, _| {
        if target == "/api/v1/auth/login" {
            login_count.fetch_add(1, Ordering::SeqCst);
            return (200, vec![], b"initial-token".to_vec());
        }
        let token = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone());
        calls.lock().unwrap().push(token.clone());
        if target == "/api/novel/syosetu/n1" {
            let mut entry: serde_json::Value =
                serde_json::from_slice(&shared_state.get("auth:v1").unwrap().unwrap()).unwrap();
            entry["token"] = json!("updated-token");
            shared_state
                .set("auth:v1", &serde_json::to_vec(&entry).unwrap())
                .unwrap();
            return (
                200,
                vec![],
                json!({
                    "titleJp": "测试", "authors": [], "introductionJp": "简介",
                    "toc": [{"titleJp": "第一章", "chapterId": "1"}]
                })
                .to_string()
                .into_bytes(),
            );
        }
        assert_eq!(target, "/api/novel/syosetu/n1/chapter/1");
        let code = if token.as_deref() == Some("Bearer updated-token") {
            200
        } else {
            401
        };
        (
            code,
            vec![],
            json!({"titleJp": "第一章", "paragraphs": ["正文"]})
                .to_string()
                .into_bytes(),
        )
    });
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins-built/novelia.wasm"
    ))
    .expect("build Novelia first");
    let runtime = PluginRuntime::new(FetchPolicy::default(), Arc::new(NoImages)).unwrap();
    let compiled = runtime.compile("novelia.wasm", &bytes).unwrap();
    let config = json!({"base-url": server.url(""), "auth-url": server.url(""), "username": "账号", "password": "密码"});
    let plugin = runtime
        .instance("novelia", compiled, config, state)
        .unwrap();
    let chapter = runtime
        .get_chapter(&plugin, "web:syosetu:n1", 0)
        .unwrap()
        .unwrap();
    assert!(chapter.content.contains("正文"));
    assert_eq!(logins.load(Ordering::SeqCst), 1);
    assert_eq!(
        *api.lock().unwrap(),
        [
            Some("Bearer initial-token".into()),
            Some("Bearer updated-token".into())
        ]
    );
}

struct UnavailableState;
impl StateStore for UnavailableState {
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>, StateStoreError> {
        Err(StateStoreError::QuotaExceeded)
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), StateStoreError> {
        Err(StateStoreError::QuotaExceeded)
    }
    fn delete(&self, _: &str) -> Result<bool, StateStoreError> {
        Err(StateStoreError::QuotaExceeded)
    }
}

#[test]
#[ignore = "requires built Novelia component"]
fn compiled_guest_caches_auth_and_bounds_refreshes() {
    let logins = Arc::new(AtomicUsize::new(0));
    let api = Arc::new(Mutex::new(Vec::new()));
    let minimum_token = Arc::new(AtomicUsize::new(0));
    let status = Arc::new(AtomicUsize::new(200));
    let fail_login = Arc::new(AtomicBool::new(false));
    let (count, calls, minimum, response_status, fail) = (
        logins.clone(),
        api.clone(),
        minimum_token.clone(),
        status.clone(),
        fail_login.clone(),
    );
    let server = common::MockServer::start(move |target, method, headers, body| {
        if target.ends_with("/api/v1/auth/login") {
            assert_eq!(method, "POST");
            let login: serde_json::Value = serde_json::from_slice(body).unwrap();
            assert_eq!(login["app"], "n");
            let n = count.fetch_add(1, Ordering::SeqCst) + 1;
            return if fail.load(Ordering::SeqCst) {
                (401, vec![], vec![])
            } else {
                (200, vec![], format!("token-{n}").into_bytes())
            };
        }
        assert_eq!(method, "GET");
        let auth = headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.clone());
        calls.lock().unwrap().push(auth.clone());
        let mut code = response_status.load(Ordering::SeqCst) as u16;
        if let Some(auth) = auth {
            let n: usize = auth.strip_prefix("Bearer token-").unwrap().parse().unwrap();
            if n < minimum.load(Ordering::SeqCst) {
                code = 401;
            }
        }
        (
            code,
            vec![],
            json!({"titleJp": "测试", "authors": [], "introductionJp": "简介", "toc": []})
                .to_string()
                .into_bytes(),
        )
    });
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins-built/novelia.wasm"
    ))
    .expect("build Novelia first");
    let runtime = PluginRuntime::new(FetchPolicy::default(), Arc::new(NoImages)).unwrap();
    let compiled = runtime.compile("novelia.wasm", &bytes).unwrap();
    let state = common::state();
    let config = json!({"base-url": server.url(""), "auth-url": server.url(""), "username": "账号", "password": " 密码 "});
    let plugin = runtime
        .instance("novelia", compiled, config.clone(), state.clone())
        .unwrap();
    let id = "web:syosetu:n1";
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    assert_eq!(logins.load(Ordering::SeqCst), 1);
    let cached = state.get("auth:v1").unwrap().unwrap();
    let entry: serde_json::Value = serde_json::from_slice(&cached).unwrap();
    assert_eq!(entry["token"], "token-1");
    assert_eq!(entry["scope"].as_str().unwrap().len(), 64);
    assert!(!String::from_utf8(cached).unwrap().contains("密码"));

    // Rebind persisted state to a new runtime, as after a server restart.
    let runtime = PluginRuntime::new(FetchPolicy::default(), Arc::new(NoImages)).unwrap();
    let compiled = runtime.compile("novelia.wasm", &bytes).unwrap();
    let plugin = runtime
        .instance("novelia", compiled.clone(), config.clone(), state.clone())
        .unwrap();
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    let mut translation = config.clone();
    translation["translation"] = json!(4);
    assert!(
        runtime
            .get_book(&plugin.with_config(translation), id)
            .unwrap()
            .is_some()
    );
    assert_eq!(logins.load(Ordering::SeqCst), 1);

    minimum_token.store(2, Ordering::SeqCst);
    let before = api.lock().unwrap().len();
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    assert_eq!(api.lock().unwrap().len() - before, 2);
    assert_eq!(logins.load(Ordering::SeqCst), 2);
    status.store(401, Ordering::SeqCst);
    let before = api.lock().unwrap().len();
    assert!(runtime.get_book(&plugin, id).unwrap().is_none());
    assert_eq!(api.lock().unwrap().len() - before, 2);
    assert_eq!(logins.load(Ordering::SeqCst), 3);
    assert!(state.get("auth:v1").unwrap().is_none());
    // A freshly issued rejected token must not cause another login.
    let before = api.lock().unwrap().len();
    assert!(runtime.get_book(&plugin, id).unwrap().is_none());
    assert_eq!(api.lock().unwrap().len() - before, 1);
    assert_eq!(logins.load(Ordering::SeqCst), 4);
    status.store(200, Ordering::SeqCst);
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    for (code, expected_calls) in [(403, 1), (503, 2)] {
        status.store(code, Ordering::SeqCst);
        let before = api.lock().unwrap().len();
        let before_logins = logins.load(Ordering::SeqCst);
        assert!(runtime.get_book(&plugin, id).unwrap().is_none());
        assert_eq!(api.lock().unwrap().len() - before, expected_calls);
        assert_eq!(logins.load(Ordering::SeqCst), before_logins);
    }
    status.store(200, Ordering::SeqCst);
    // An expired cached token followed by a failed login is not replayed.
    minimum_token.store(logins.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
    fail_login.store(true, Ordering::SeqCst);
    let before = logins.load(Ordering::SeqCst);
    let before_api = api.lock().unwrap().len();
    assert!(runtime.get_book(&plugin, id).unwrap().is_none());
    assert_eq!(logins.load(Ordering::SeqCst), before + 1);
    assert_eq!(api.lock().unwrap().len(), before_api + 1);
    assert!(state.get("auth:v1").unwrap().is_none());
    fail_login.store(false, Ordering::SeqCst);
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    for (field, value) in [
        ("username", json!("另一账号")),
        ("password", json!("新密码")),
        ("base-url", json!(server.url("/alternate"))),
        ("auth-url", json!(server.url("/alternate"))),
    ] {
        let mut changed = config.clone();
        changed[field] = value;
        let before = logins.load(Ordering::SeqCst);
        assert!(
            runtime
                .get_book(&plugin.with_config(changed), id)
                .unwrap()
                .is_some()
        );
        assert_eq!(logins.load(Ordering::SeqCst), before + 1);
    }
    fail_login.store(true, Ordering::SeqCst);
    let before = logins.load(Ordering::SeqCst);
    let before_api = api.lock().unwrap().len();
    for _ in 0..2 {
        assert!(runtime.get_book(&plugin, id).unwrap().is_none());
    }
    assert_eq!(logins.load(Ordering::SeqCst) - before, 2);
    assert_eq!(api.lock().unwrap().len(), before_api);
    fail_login.store(false, Ordering::SeqCst);
    state.set("auth:v1", b"invalid cache entry").unwrap();
    assert!(runtime.get_book(&plugin, id).unwrap().is_some());
    let before = logins.load(Ordering::SeqCst);
    let isolated = runtime
        .instance("other", compiled.clone(), config.clone(), common::state())
        .unwrap();
    assert!(runtime.get_book(&isolated, id).unwrap().is_some());
    assert_eq!(logins.load(Ordering::SeqCst), before + 1);
    let unavailable = runtime
        .instance(
            "unavailable",
            compiled,
            config.clone(),
            Arc::new(UnavailableState),
        )
        .unwrap();
    assert!(runtime.get_book(&unavailable, id).unwrap().is_some());
    assert_eq!(logins.load(Ordering::SeqCst), before + 2);
    let mut anonymous = config;
    anonymous["username"] = json!("");
    anonymous["password"] = json!("");
    assert!(
        runtime
            .get_book(&plugin.with_config(anonymous), id)
            .unwrap()
            .is_some()
    );
    assert_eq!(api.lock().unwrap().last(), Some(&None));
    assert_eq!(logins.load(Ordering::SeqCst), before + 2);
}
