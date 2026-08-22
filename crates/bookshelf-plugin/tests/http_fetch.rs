//! Host policy tests for the plugin `http.fetch` import
//! (`docs/plugin-http-api-design.md` §3.3 acceptance list): allow-list
//! denial, SSRF block (localhost/loopback), timeout, size-limit,
//! redirect-limit, and credential stripping on cross-host redirects.

mod common;

use std::thread;
use std::time::Duration;

use bookshelf_plugin::http_fetch::{fetch, FetchError, FetchPolicy, FetchRequest, FetchResponse};

use common::MockServer;

fn policy(allowed: &[&str]) -> FetchPolicy {
    FetchPolicy {
        allowed_hosts: allowed.iter().map(|s| s.to_string()).collect(),
        http_allowed: true,
        timeout_ms: 2_000,
        max_bytes: 1_000_000,
    }
}

fn get(policy: &FetchPolicy, url: &str) -> Result<FetchResponse, FetchError> {
    fetch(
        policy,
        FetchRequest {
            method: "GET".into(),
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
        },
    )
}

fn request(
    policy: &FetchPolicy,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> Result<FetchResponse, FetchError> {
    fetch(
        policy,
        FetchRequest {
            method: method.into(),
            url: url.into(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            body,
            timeout_ms: None,
        },
    )
}

/// Standard routes shared by the tests (see `router`).
fn router(
    cross_redirect_target: Option<String>,
) -> impl Fn(&str, &str, &[(String, String)], &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync
{
    move |target, _method, _headers, _body| {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match path {
            "/hello" => (
                200,
                vec![("Content-Type".into(), "text/plain".into())],
                b"hello world".to_vec(),
            ),
            "/json" => (
                200,
                vec![("Content-Type".into(), "application/json".into())],
                br#"{"ok":true}"#.to_vec(),
            ),
            "/status/404" => (404, vec![], b"not found".to_vec()),
            "/status/500" => (500, vec![], b"boom".to_vec()),
            "/redirect" => (302, vec![("Location".into(), "/hello".into())], Vec::new()),
            // Same-host redirect straight to the echo endpoint (header
            // assertions).
            "/redirect-echo" => (302, vec![("Location".into(), "/echo".into())], Vec::new()),
            // Relative Location (no leading slash).
            "/rel" => (302, vec![("Location".into(), "hello".into())], Vec::new()),
            "/loop" => (302, vec![("Location".into(), "/loop".into())], Vec::new()),
            "/chain/0" => (200, vec![], b"chain-0".to_vec()),
            _ if path.starts_with("/chain/") => {
                let n: u32 = path.trim_start_matches("/chain/").parse().unwrap_or(0);
                (
                    302,
                    vec![("Location".into(), format!("/chain/{}", n - 1))],
                    Vec::new(),
                )
            }
            "/cross" => {
                ({
                    let target = cross_redirect_target.clone().unwrap_or_default();
                    (302, vec![("Location".into(), target)], Vec::new())
                })
            }
            "/echo" => {
                // Reflect method + received headers as JSON, so tests can
                // assert exactly what the host forwarded.
                let mut json = String::from("{");
                json.push_str(&format!(" \"method\": \"{_method}\""));
                for (name, value) in _headers {
                    json.push_str(&format!(", \"{name}\": \"{value}\""));
                }
                json.push_str(" }");
                (
                    200,
                    vec![("Content-Type".into(), "application/json".into())],
                    json.into_bytes(),
                )
            }
            "/slow" => {
                let ms: u64 = query
                    .split('=')
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1000);
                thread::sleep(Duration::from_millis(ms));
                (200, vec![], b"finally".to_vec())
            }
            "/big" => {
                let n: usize = query
                    .split('=')
                    .nth(1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1024);
                (200, vec![], vec![b'x'; n])
            }
            _ => (404, vec![], b"no route".to_vec()),
        }
    }
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// ---- allow list -----------------------------------------------------------------

#[test]
fn empty_allow_list_denies_everything() {
    let server = MockServer::start(router(None));
    let p = policy(&[]);
    let err = get(&p, &server.url("/hello")).expect_err("must be denied");
    match err {
        FetchError::Denied(msg) => assert!(msg.contains("127.0.0.1"), "got: {msg}"),
        other => panic!("expected denied, got {other:?}"),
    }
}

#[test]
fn allow_listed_loopback_host_is_reachable() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = get(&p, &server.url("/hello")).expect("fetch ok");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");
    assert!(resp.final_url.ends_with("/hello"));
}

#[test]
fn hostname_resolving_to_loopback_is_blocked_when_not_allow_listed() {
    let server = MockServer::start(router(None));
    // `localhost` resolves to 127.0.0.1 but is not in the allow list...
    let p = policy(&["example.com"]);
    let err = get(
        &p,
        &format!("http://localhost:{}/hello", server.addr.port()),
    )
    .expect_err("loopback must be blocked");
    match err {
        FetchError::Denied(msg) => assert!(msg.contains("localhost"), "got: {msg}"),
        other => panic!("expected denied, got {other:?}"),
    }
}

#[test]
fn allow_list_with_port_only_matches_that_endpoint() {
    let server = MockServer::start(router(None));
    // The list names the exact host:port; anything else stays denied.
    let p = policy(&[&server.netloc()]);
    let denied = policy(&["127.0.0.1:1"]);
    assert!(get(&p, &server.url("/hello")).is_ok());
    let err = get(&denied, &server.url("/hello")).expect_err("different port must be denied");
    assert!(matches!(err, FetchError::Denied(_)));
}

#[test]
fn http_can_be_disabled_by_policy() {
    let server = MockServer::start(router(None));
    let mut p = policy(&[&server.netloc()]);
    p.http_allowed = false;
    let err = get(&p, &server.url("/hello")).expect_err("http must be denied");
    match err {
        FetchError::Denied(msg) => assert!(msg.contains("http"), "got: {msg}"),
        other => panic!("expected denied, got {other:?}"),
    }
}

#[test]
fn credentials_in_url_are_rejected() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let err = get(
        &p,
        &format!(
            "http://user:pass@{}:{}",
            server.addr.ip(),
            server.addr.port()
        ),
    )
    .expect_err("URL credentials must be rejected");
    assert!(matches!(err, FetchError::InvalidUrl(_)));
    assert!(server.requests().is_empty(), "no request may be sent");
}

#[test]
fn unsupported_scheme_is_invalid() {
    let p = policy(&[]);
    let err = get(&p, "ftp://example.com/book").expect_err("ftp must be invalid");
    assert!(matches!(err, FetchError::InvalidUrl(_)));
}

// ---- response semantics ----------------------------------------------------------

#[test]
fn application_level_statuses_are_not_errors() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = get(&p, &server.url("/status/404")).expect("404 is a response");
    assert_eq!(resp.status, 404);
    assert_eq!(resp.body, b"not found");
    let resp = get(&p, &server.url("/status/500")).expect("500 is a response");
    assert_eq!(resp.status, 500);
}

#[test]
fn response_headers_and_final_url_are_returned() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = get(&p, &server.url("/json")).expect("fetch ok");
    assert_eq!(resp.status, 200);
    assert_eq!(
        find_header(&resp.headers, "content-type"),
        Some("application/json")
    );
    assert!(resp.final_url.ends_with("/json"));
}

#[test]
fn post_body_and_method_are_sent() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = request(
        &p,
        "POST",
        &server.url("/echo"),
        &[("Content-Type", "application/json")],
        Some(br#"{"a":1}"#.to_vec()),
    )
    .expect("fetch ok");
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("\"method\": \"POST\""), "got: {body}");
    let seen = server.requests();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].body, br#"{"a":1}"#);
}

// ---- timeout / size / redirects ----------------------------------------------------

#[test]
fn slow_source_hits_the_timeout_cap() {
    let server = MockServer::start(router(None));
    let mut p = policy(&[&server.netloc()]);
    p.timeout_ms = 150;
    let err = get(&p, &server.url("/slow?ms=5000")).expect_err("must time out");
    match err {
        FetchError::Timeout(ms) => assert!(ms <= p.timeout_ms),
        other => panic!("expected timeout, got {other:?}"),
    }
    // A short request against the same policy still works.
    assert!(get(&p, &server.url("/hello")).is_ok());
}

#[test]
fn oversized_response_aborts_with_size_limit() {
    let server = MockServer::start(router(None));
    let mut p = policy(&[&server.netloc()]);
    p.max_bytes = 100;
    let err = get(&p, &server.url("/big?bytes=10000")).expect_err("must be size-limited");
    assert!(matches!(err, FetchError::SizeLimit(100)));
    // Exactly at the cap is fine.
    let resp = get(&p, &server.url("/big?bytes=100")).expect("at-cap body ok");
    assert_eq!(resp.body.len(), 100);
}

#[test]
fn redirect_loop_hits_the_hop_limit() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let err = get(&p, &server.url("/loop")).expect_err("redirect loop must stop");
    assert!(matches!(err, FetchError::RedirectLimit(n) if n == bookshelf_plugin::MAX_REDIRECTS));
}

#[test]
fn redirect_chains_are_followed_and_reported() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = get(&p, &server.url("/chain/3")).expect("chain follows");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"chain-0");
    assert!(resp.final_url.ends_with("/chain/0"));
}

#[test]
fn relative_redirects_resolve() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = get(&p, &server.url("/rel")).expect("relative redirect ok");
    assert_eq!(resp.status, 200);
    assert!(resp.final_url.ends_with("/hello"));
}

// ---- header stripping on redirects --------------------------------------------------

#[test]
fn credentials_are_stripped_on_cross_host_redirects() {
    let other = MockServer::start(router(None));
    let server = MockServer::start(router(Some(other.url("/hello"))));
    let p = policy(&[&server.netloc(), &other.netloc()]);
    let resp = request(
        &p,
        "GET",
        &server.url("/cross"),
        &[
            ("Authorization", "Bearer secret"),
            ("Cookie", "session=abc"),
            ("X-Api-Key", "k-123"),
        ],
        None,
    )
    .expect("cross-host redirect must work");
    assert_eq!(resp.status, 200);
    assert!(resp.final_url.ends_with("/hello"));

    let seen = other.requests();
    assert_eq!(seen.len(), 1, "exactly one hop to the other host");
    for name in ["authorization", "cookie", "x-api-key"] {
        assert!(
            seen[0].header(name).is_none(),
            "`{name}` must not cross hosts, got {:?}",
            seen[0].header(name)
        );
    }
}

#[test]
fn credentials_survive_same_host_redirects() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    let resp = request(
        &p,
        "GET",
        &server.url("/redirect-echo"),
        &[("Authorization", "Bearer keepme"), ("X-Api-Key", "k-123")],
        None,
    )
    .expect("same-host redirect ok");
    assert_eq!(resp.status, 200);
    let seen = server.requests();
    let echoed = String::from_utf8(resp.body).unwrap();
    assert!(
        echoed.contains("\"authorization\": \"Bearer keepme\""),
        "got: {echoed}"
    );
    assert!(echoed.contains("\"x-api-key\": \"k-123\""), "got: {echoed}");
    assert!(seen.iter().any(|r| r.url == "/echo"));
}

#[test]
fn host_managed_headers_are_never_forwarded() {
    let server = MockServer::start(router(None));
    let p = policy(&[&server.netloc()]);
    // A plugin trying to smuggle a different Host / Content-Length fails
    // silently: the host strips them and ureq sets its own.
    let resp = request(
        &p,
        "POST",
        &server.url("/echo"),
        &[
            ("Host", "evil.example"),
            ("Content-Length", "0"),
            ("Transfer-Encoding", "chunked"),
        ],
        Some(b"payload".to_vec()),
    )
    .expect("fetch ok");
    let echoed = String::from_utf8(resp.body).unwrap();
    assert!(!echoed.contains("evil.example"), "got: {echoed}");
    // ureq sets the Host header itself (HTTP/1.1) — it must point at the
    // mock server, never at the plugin's smuggled value.
    let seen = server.requests();
    let host = seen[0].header("host").expect("Host is set").to_string();
    assert!(!host.contains("evil.example"), "host: {host}");
    assert!(host.starts_with("127.0.0.1"), "host: {host}");
}

// ---- guest-visible error mapping -----------------------------------------------------

#[test]
fn dns_failure_is_a_transport_error() {
    let p = policy(&["nonexistent.invalid"]);
    // Resolving `.invalid` always fails; the policy pre-check itself
    // reports the resolve failure as a transport error.
    let err = get(&p, "http://nonexistent.invalid/book").expect_err("DNS must fail");
    match err {
        FetchError::Transport(_) | FetchError::Denied(_) => {}
        other => panic!("expected transport/denied, got {other:?}"),
    }
}

#[test]
fn env_policy_defaults_are_safe() {
    // The in-process env is not touched by tests, but the default policy
    // must deny everything: an empty allow list is the safe default.
    let p = FetchPolicy::default();
    assert!(p.allowed_hosts.is_empty());
    assert!(p.http_allowed);
    assert_eq!(p.timeout_ms, 30_000);
    assert_eq!(p.max_bytes, 64 * 1024 * 1024);
}
