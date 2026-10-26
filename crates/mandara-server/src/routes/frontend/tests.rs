//! Observable contracts for the frontend service, independent of its cache layout.

use super::router;
use crate::test_support::tempdir;
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use std::fs::{self, File, FileTimes};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};
use tower::ServiceExt;

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

fn write(root: &Path, name: &str, bytes: &[u8]) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn site(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Router) {
    let dir = tempdir("mandara-frontend-");
    for &(name, bytes) in files {
        write(dir.path(), name, bytes);
    }
    let app = router(dir.path()).unwrap();
    (dir, app)
}

async fn request(app: &Router, method: Method, path: &str, fields: &[(&str, &str)]) -> Reply {
    let mut req = Request::builder().method(method).uri(path);
    for &(name, value) in fields {
        req = req.header(name, value);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = res.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, usize::MAX).await.unwrap(),
    }
}

fn assert_same_representation_headers(left: &Reply, right: &Reply, context: &str) {
    // Request-specific fields (e.g. Date or trace IDs) are not representation identity.
    for name in [
        header::ETAG,
        header::CONTENT_LENGTH,
        header::CONTENT_TYPE,
        header::CACHE_CONTROL,
        header::ACCEPT_RANGES,
    ] {
        assert_eq!(
            left.headers.get(&name),
            right.headers.get(&name),
            "{context}: {name}"
        );
    }
}

fn tag(reply: &Reply) -> String {
    let value = reply.headers[header::ETAG].to_str().unwrap();
    assert!(
        value.starts_with('"') && value.ends_with('"'),
        "expected a strong entity-tag: {value}"
    );
    value.to_owned()
}

fn payloads() -> [Vec<u8>; 5] {
    [
        Vec::new(),
        vec![0],
        vec![0, 255, 128, 13, 10],
        "中文与é".as_bytes().to_vec(),
        (0..4097).map(|i| (i % 256) as u8).collect(),
    ]
}

fn changed(bytes: &[u8]) -> Vec<u8> {
    let mut other = bytes.to_vec();
    if let Some(last) = other.last_mut() {
        *last ^= 1;
    } else {
        other.push(0);
    }
    other
}

#[tokio::test]
async fn validator_binds_selected_bytes() {
    // Entity-tags are opaque. Equal bytes must have equal tags across locations
    // and metadata; a one-byte edit must not validate against the old bytes.
    for bytes in payloads() {
        let other = changed(&bytes);
        let dir = tempdir("mandara-frontend-identity-");
        for (name, body) in [
            ("index.html", b"entry document".as_slice()),
            ("first.js", &bytes),
            ("assets/copy.css", &bytes),
            ("other.bin", &other),
        ] {
            write(dir.path(), name, body);
        }
        // Same size/mtime, different bytes; same bytes, different mtime/MIME/path.
        for (name, offset) in [("first.js", 0), ("other.bin", 0), ("assets/copy.css", 10)] {
            File::options()
                .write(true)
                .open(dir.path().join(name))
                .unwrap()
                .set_times(
                    FileTimes::new()
                        .set_modified(UNIX_EPOCH + Duration::from_secs(1_600_000_000 + offset)),
                )
                .unwrap();
        }
        let app = router(dir.path()).unwrap();
        let first = request(&app, Method::GET, "/first.js", &[]).await;
        assert_eq!(first.status, StatusCode::OK);
        assert_eq!(first.body.as_ref(), bytes.as_slice());
        let own_tag = tag(&first);
        for (path, mime) in [
            ("/first.js", "javascript"),
            ("/%66irst.js?version=1", "javascript"),
            ("/./first.js", "javascript"),
            ("/assets/copy.css", "text/css"),
        ] {
            let get = request(&app, Method::GET, path, &[]).await;
            assert_eq!(get.status, StatusCode::OK, "{path}");
            assert_eq!(get.body.as_ref(), bytes.as_slice(), "{path}");
            assert_eq!(tag(&get), own_tag, "{path}");
            assert!(
                get.headers[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .contains(mime)
            );
        }
        let foreign = request(
            &app,
            Method::GET,
            "/other.bin",
            &[("if-none-match", &own_tag)],
        )
        .await;
        assert_eq!(foreign.status, StatusCode::OK);
        assert_eq!(foreign.body.as_ref(), other.as_slice());
        assert_ne!(tag(&foreign), own_tag);
    }
}

#[tokio::test]
async fn conditional_responses_follow_the_decision_table() {
    for bytes in payloads() {
        let changed = changed(&bytes);
        let (_dir, app) = site(&[
            ("page.html", &bytes),
            ("assets/app.js", &bytes),
            ("other.bin", &changed),
        ]);
        let current = tag(&request(&app, Method::GET, "/page.html", &[]).await);
        let other = tag(&request(&app, Method::GET, "/other.bin", &[]).await);
        assert_ne!(current, other);
        // Header fields, whether If-Match permits, whether If-None-Match matches.
        // These are protocol expectations, not calls to the implementation's parser.
        let mut cases = vec![
            (vec![], true, false),
            (vec!["*".to_owned()], true, true),
            (vec![current.clone()], true, true),
            (vec![format!("W/{current}")], false, true),
            (vec![other.clone()], false, false),
            (vec![format!("W/{other}")], false, false),
        ];
        // List order and splitting a list across header fields cannot affect matching.
        for (a, b, permits, matches) in [
            (current.clone(), other.clone(), true, true),
            (format!("W/{current}"), other.clone(), false, true),
            (other.clone(), format!("W/{other}"), false, false),
        ] {
            for values in [vec![a.clone(), b.clone()], vec![b, a]] {
                cases.push((vec![values.join(", ")], permits, matches));
                cases.push((values, permits, matches));
            }
        }
        for path in ["/page.html", "/assets/app.js"] {
            for (if_match, permits, _) in &cases {
                for (if_none, _, matches) in &cases {
                    let fields: Vec<_> = if_match
                        .iter()
                        .map(|v| ("if-match", v.as_str()))
                        .chain(if_none.iter().map(|v| ("if-none-match", v.as_str())))
                        .collect();
                    let expected = if !permits {
                        StatusCode::PRECONDITION_FAILED
                    } else if *matches {
                        StatusCode::NOT_MODIFIED
                    } else {
                        StatusCode::OK
                    };
                    let context = format!(
                        "{path}: If-Match={if_match:?}, If-None-Match={if_none:?}, bytes={}",
                        bytes.len()
                    );
                    let get = request(&app, Method::GET, path, &fields).await;
                    let head = request(&app, Method::HEAD, path, &fields).await;
                    assert_eq!(get.status, expected, "{context}");
                    assert_eq!(head.status, get.status, "{context}");
                    assert_same_representation_headers(&head, &get, &context);
                    assert!(head.body.is_empty(), "{context}");
                    assert_eq!(
                        get.body.as_ref(),
                        if expected == StatusCode::OK {
                            bytes.as_slice()
                        } else {
                            &[]
                        },
                        "{context}"
                    );
                    if expected != StatusCode::PRECONDITION_FAILED {
                        assert_eq!(get.headers[header::ETAG], current, "{context}");
                        assert_eq!(
                            get.headers[header::CONTENT_LENGTH],
                            bytes.len().to_string(),
                            "{context}"
                        );
                    }
                    let cacheable =
                        path.starts_with("/assets/") && expected != StatusCode::PRECONDITION_FAILED;
                    assert_eq!(
                        get.headers[header::CACHE_CONTROL],
                        if cacheable {
                            "public, max-age=31536000, immutable"
                        } else {
                            "no-cache"
                        },
                        "{context}"
                    );
                    assert_eq!(
                        get.headers.get_all(header::CACHE_CONTROL).iter().count(),
                        1,
                        "{context}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn snapshots_do_not_share_mutable_state() {
    // Two generations must retain their own representation, even before their
    // first request. A third immutable site supplies opaque reference tags.
    for bytes in payloads() {
        let other = changed(&bytes);
        let (_reference_dir, reference) = site(&[("old.bin", &bytes), ("new.bin", &other)]);
        let old_tag = tag(&request(&reference, Method::GET, "/old.bin", &[]).await);
        let new_tag = tag(&request(&reference, Method::GET, "/new.bin", &[]).await);
        assert_ne!(old_tag, new_tag);
        let (dir, first) = site(&[("index.html", &bytes)]);
        write(dir.path(), "index.html", &other);
        let second = router(dir.path()).unwrap();
        for (app, expected, own_tag, foreign_tag) in [
            (&first, &bytes, &old_tag, &new_tag),
            (&second, &other, &new_tag, &old_tag),
            (&first, &bytes, &old_tag, &new_tag),
        ] {
            let res = request(app, Method::GET, "/", &[("if-none-match", foreign_tag)]).await;
            assert_eq!(res.status, StatusCode::OK);
            assert_eq!(res.body.as_ref(), expected.as_slice());
            assert_eq!(tag(&res), *own_tag);
            let res = request(app, Method::GET, "/", &[("if-none-match", own_tag)]).await;
            assert_eq!(res.status, StatusCode::NOT_MODIFIED);
            assert_eq!(tag(&res), *own_tag);
            assert!(res.body.is_empty());
        }
    }
}

#[tokio::test]
async fn errors_are_not_validated_as_the_fallback_document() {
    let index = b"entry document, not the missing resource";
    let (_dir, app) = site(&[("index.html", index)]);
    let tag = tag(&request(&app, Method::GET, "/", &[]).await);
    for path in ["/missing", "/assets/missing.js", "/index.html/child"] {
        for fields in [
            vec![("if-none-match", "*")],
            vec![("if-none-match", tag.as_str())],
            vec![("if-match", "*"), ("if-none-match", "*")],
        ] {
            let get = request(&app, Method::GET, path, &fields).await;
            let head = request(&app, Method::HEAD, path, &fields).await;
            assert_eq!(get.status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(get.body.as_ref(), index);
            assert!(!get.headers.contains_key(header::ETAG));
            assert_eq!(get.headers[header::CACHE_CONTROL], "no-cache");
            assert_eq!(head.status, get.status);
            assert_same_representation_headers(&head, &get, path);
            assert!(head.body.is_empty());
        }
    }
    let (_empty, app) = site(&[]);
    let missing = request(&app, Method::GET, "/", &[("if-none-match", "*")]).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.body.is_empty());
    assert!(!missing.headers.contains_key(header::ETAG));
}

#[tokio::test]
async fn paths_cannot_escape_the_served_namespace() {
    let parent = tempdir("mandara-frontend-paths-");
    write(parent.path(), "dist/index.html", b"entry");
    write(parent.path(), "dist/inside.txt", b"inside");
    write(parent.path(), "dist-extra/secret.txt", b"outside secret");
    let app = router(&parent.path().join("dist")).unwrap();
    for path in ["/inside.txt", "/%69nside.txt", "/./inside.txt?ignored=1"] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status, StatusCode::OK);
        assert_eq!(res.body.as_ref(), b"inside");
    }
    for path in [
        "/../dist-extra/secret.txt",
        "/%2e%2e/dist-extra/secret.txt",
        "/.%2E%2Fdist-extra/secret.txt",
        "/..%5cdist-extra%5csecret.txt",
        "/%00",
        "/%ff",
    ] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert!(res.status.is_client_error(), "{path}");
        assert_ne!(res.body.as_ref(), b"outside secret", "{path}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_preserve_the_namespace_boundary() {
    use std::os::unix::fs::symlink;

    let parent = tempdir("mandara-frontend-links-");
    write(parent.path(), "dist/index.html", b"entry");
    write(parent.path(), "dist/docs/index.html", b"document index");
    write(parent.path(), "dist/docs/inside.txt", b"inside document");
    write(parent.path(), "dist-extra/secret.txt", b"outside secret");
    let root = parent.path().join("dist");
    let outside = parent.path().join("dist-extra");
    symlink(root.join("docs"), root.join("alias")).unwrap();
    symlink(&outside, root.join("outside")).unwrap();
    symlink(outside.join("secret.txt"), root.join("outside.txt")).unwrap();
    fs::create_dir(root.join("escape")).unwrap();
    symlink(outside.join("secret.txt"), root.join("escape/index.html")).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    let app = router(&root).unwrap();
    // A directory link inside the boundary must work, not merely be rejected.
    for (target, alias, bytes) in [
        (
            "/docs/inside.txt",
            "/alias/inside.txt",
            b"inside document".as_slice(),
        ),
        ("/docs/", "/alias/", b"document index".as_slice()),
    ] {
        let direct = request(&app, Method::GET, target, &[]).await;
        let linked = request(&app, Method::GET, alias, &[]).await;
        assert_eq!(linked.status, StatusCode::OK);
        assert_eq!(linked.body.as_ref(), bytes);
        assert_eq!(linked.body, direct.body);
        assert_eq!(linked.headers[header::ETAG], direct.headers[header::ETAG]);
    }
    for path in [
        "/outside/secret.txt",
        "/outside.txt",
        "/escape/",
        "/loop/index.html",
    ] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status, StatusCode::NOT_FOUND, "{path}");
        assert_ne!(res.body.as_ref(), b"outside secret", "{path}");
    }
}

#[tokio::test]
async fn range_requests_never_return_partial_representations() {
    let bytes: Vec<u8> = (0..=255).collect();
    let (_dir, app) = site(&[("data.bin", &bytes)]);
    let baseline = request(&app, Method::GET, "/data.bin", &[]).await;
    for range in ["bytes=0-0", "bytes=999-1000", "bytes=0-1,3-4"] {
        let res = request(&app, Method::GET, "/data.bin", &[("range", range)]).await;
        assert_eq!(res.status, StatusCode::OK);
        assert_eq!(res.body.as_ref(), bytes.as_slice());
        assert_same_representation_headers(&res, &baseline, range);
        assert!(!res.headers.contains_key(header::CONTENT_RANGE));
        assert_eq!(res.headers[header::ACCEPT_RANGES], "none");
    }
    let tag = tag(&baseline);
    let res = request(
        &app,
        Method::GET,
        "/data.bin",
        &[("range", "bytes=0-0"), ("if-none-match", &tag)],
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_MODIFIED);
    assert!(res.body.is_empty());
}
