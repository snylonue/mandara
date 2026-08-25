//! Behavioral tests for the plugin `fetch` implementation: statuses,
//! headers, bodies, redirects, timeout and size caps. There is no
//! destination policy to test — the host provides plain network access.

use std::time::Duration;

use bookshelf_plugin::http_fetch::{FetchError, FetchPolicy, FetchRequest, FetchResponse, fetch};

mod common;

fn policy() -> FetchPolicy {
    FetchPolicy::default()
}

fn get(url: &str) -> Result<FetchResponse, FetchError> {
    fetch(
        &policy(),
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
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> Result<FetchResponse, FetchError> {
    fetch(
        &policy(),
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

fn router() -> Box<common::HttpHandler> {
    Box::new(
        move |target: &str, method: &str, headers: &[(String, String)], _body: &[u8]| {
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            match path {
                "/hello" => (
                    200,
                    vec![("Content-Type".into(), "text/plain".into())],
                    b"hello world".to_vec(),
                ),
                "/status/404" => (404, vec![], b"not found".to_vec()),
                "/status/500" => (500, vec![], b"boom".to_vec()),
                "/redirect" => (302, vec![("Location".into(), "/hello".into())], Vec::new()),
                "/loop" => (302, vec![("Location".into(), "/loop".into())], Vec::new()),
                "/echo" => {
                    // Reflect method + received headers as JSON, so tests
                    // can assert exactly what the host forwarded.
                    let mut json = String::from("{");
                    json.push_str(&format!(" \"method\": \"{method}\""));
                    for (name, value) in headers {
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
                    std::thread::sleep(Duration::from_millis(ms));
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
        },
    )
}

#[test]
fn application_level_statuses_are_not_errors() {
    let server = common::MockServer::start(router());
    let resp = get(&server.url("/status/404")).expect("404 is a response, not an error");
    assert_eq!(resp.status, 404);
    assert_eq!(resp.body, b"not found");

    let resp = get(&server.url("/status/500")).expect("500 is a response, not an error");
    assert_eq!(resp.status, 500);
}

#[test]
fn response_headers_and_body_are_returned() {
    let server = common::MockServer::start(router());
    let resp = get(&server.url("/hello")).expect("ok");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");
    assert!(
        resp.headers
            .iter()
            .any(|(n, v)| n.eq_ignore_ascii_case("content-type") && v == "text/plain")
    );
}

#[test]
fn post_body_and_method_are_sent() {
    let server = common::MockServer::start(router());
    let resp = request("POST", &server.url("/echo"), &[], Some(b"data".to_vec()))
        .expect("POST reaches the server");
    assert!(String::from_utf8_lossy(&resp.body).contains("\"method\": \"POST\""));
}

#[test]
fn slow_source_hits_the_timeout_cap() {
    let server = common::MockServer::start(router());
    let p = FetchPolicy {
        timeout_ms: 300,
        max_bytes: 64 * 1024 * 1024,
    };
    let err = fetch(
        &p,
        FetchRequest {
            method: "GET".into(),
            url: server.url("/slow?ms=2000"),
            headers: vec![],
            body: None,
            timeout_ms: None,
        },
    )
    .expect_err("must time out");
    assert!(matches!(err, FetchError::Timeout(300)));
}

#[test]
fn oversized_response_aborts_with_size_limit() {
    let server = common::MockServer::start(router());
    let p = FetchPolicy {
        timeout_ms: 5_000,
        max_bytes: 10,
    };
    let err = fetch(
        &p,
        FetchRequest {
            method: "GET".into(),
            url: server.url("/big?n=100"),
            headers: vec![],
            body: None,
            timeout_ms: None,
        },
    )
    .expect_err("must hit the size cap");
    assert!(matches!(err, FetchError::SizeLimit(10)));
}

#[test]
fn redirect_chains_are_followed_and_reported() {
    let server = common::MockServer::start(router());
    let resp = get(&server.url("/redirect")).expect("redirects followed");
    assert_eq!(resp.body, b"hello world");
    assert!(resp.final_url.ends_with("/hello"));
}

#[test]
fn redirect_loops_hit_the_hop_limit() {
    let server = common::MockServer::start(router());
    let err = get(&server.url("/loop")).expect_err("redirect loop must hit the limit");
    assert!(matches!(err, FetchError::RedirectLimit(5)));
}
