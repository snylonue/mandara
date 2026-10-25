use super::router;
use crate::test_support::tempdir;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use std::fs::{self, File, FileTimes};
use std::time::Duration;
use tower::ServiceExt;

const HELLO_ETAG: &str =
    "\"sha256-2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\"";
const EMPTY_ETAG: &str =
    "\"sha256-e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

fn fixture() -> tempfile::TempDir {
    let dir = tempdir("mandara-frontend-");
    fs::write(dir.path().join("index.html"), b"hello").unwrap();
    fs::create_dir(dir.path().join("assets")).unwrap();
    fs::write(dir.path().join("assets/app-hash.js"), b"hello").unwrap();
    dir
}

async fn request(app: &Router, method: Method, path: &str, fields: &[(&str, &str)]) -> Response {
    let mut req = Request::builder().method(method).uri(path);
    for &(name, value) in fields {
        req = req.header(name, value);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body(res: Response) -> Vec<u8> {
    to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn get_and_head_return_the_same_content_hash() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for path in ["/", "/index.html", "/index.html?version=1"] {
        for method in [Method::GET, Method::HEAD] {
            let res = request(&app, method.clone(), path, &[]).await;
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
            assert_eq!(res.headers()[header::CONTENT_TYPE], "text/html");
            assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
            assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
            assert!(!res.headers().contains_key(header::LAST_MODIFIED));
            assert_eq!(
                body(res).await,
                if method == Method::HEAD {
                    b"".as_slice()
                } else {
                    b"hello"
                }
            );
        }
    }
}

#[tokio::test]
async fn etags_follow_content_not_file_metadata() {
    let dir = fixture();
    let path = dir.path().join("index.html");
    let modified = fs::metadata(&path).unwrap().modified().unwrap() + Duration::from_secs(60);
    File::open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    let app = router(dir.path()).unwrap();
    let res = request(&app, Method::GET, "/", &[("if-none-match", HELLO_ETAG)]).await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);

    let before = fs::metadata(&path).unwrap();
    fs::write(&path, b"world").unwrap();
    File::open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    let rebuilt = router(dir.path()).unwrap();
    for path in ["/", "/index.html"] {
        let res = request(
            &rebuilt,
            Method::GET,
            path,
            &[("if-none-match", HELLO_ETAG)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_ne!(res.headers()[header::ETAG], HELLO_ETAG);
        assert_eq!(body(res).await, b"world");
    }
}

#[tokio::test]
async fn constructed_handlers_keep_bytes_and_etags_without_filesystem_access() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    fs::write(dir.path().join("index.html"), b"world").unwrap();
    fs::write(dir.path().join("assets/late.js"), b"world").unwrap();
    let rebuilt = router(dir.path()).unwrap();
    drop(dir);

    for path in ["/", "/index.html", "/assets/app-hash.js"] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
        assert_eq!(body(res).await, b"hello");
        let res = request(&app, Method::HEAD, path, &[("if-none-match", HELLO_ETAG)]).await;
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
        assert!(body(res).await.is_empty());
    }
    let res = request(&app, Method::GET, "/assets/late.js", &[]).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(res).await, b"hello");
    for path in ["/", "/assets/late.js"] {
        let res = request(
            &rebuilt,
            Method::GET,
            path,
            &[("if-none-match", HELLO_ETAG)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_ne!(res.headers()[header::ETAG], HELLO_ETAG);
        assert_eq!(body(res).await, b"world");
    }
}

#[tokio::test]
async fn if_none_match_accepts_weak_lists_wildcards_and_repeated_fields() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for value in [
        HELLO_ETAG.to_owned(),
        format!("W/{HELLO_ETAG}"),
        format!("\"stale\", W/{HELLO_ETAG}"),
        "*".to_owned(),
    ] {
        for method in [Method::GET, Method::HEAD] {
            let res = request(&app, method, "/", &[("if-none-match", &value)]).await;
            assert_eq!(res.status(), StatusCode::NOT_MODIFIED, "{value}");
            assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
            assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
            assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
            assert!(body(res).await.is_empty());
        }
    }
    let res = request(
        &app,
        Method::GET,
        "/",
        &[
            ("if-none-match", "\"stale\""),
            ("if-none-match", HELLO_ETAG),
        ],
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    let res = request(&app, Method::GET, "/", &[("if-none-match", "\"stale\"")]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body(res).await, b"hello");
}

#[tokio::test]
async fn if_match_is_strong_and_precedes_if_none_match() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for value in [format!("W/{HELLO_ETAG}"), "\"stale\"".to_owned()] {
        for path in ["/", "/assets/app-hash.js"] {
            let res = request(
                &app,
                Method::GET,
                path,
                &[("if-match", &value), ("if-none-match", HELLO_ETAG)],
            )
            .await;
            assert_eq!(res.status(), StatusCode::PRECONDITION_FAILED);
            assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
            assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
            assert_eq!(
                res.headers().get_all(header::CACHE_CONTROL).iter().count(),
                1
            );
            assert!(body(res).await.is_empty());
        }
    }
    for value in [
        HELLO_ETAG.to_owned(),
        "*".to_owned(),
        format!("\"stale\", {HELLO_ETAG}"),
    ] {
        let res = request(&app, Method::GET, "/", &[("if-match", &value)]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body(res).await, b"hello");
    }
    let res = request(
        &app,
        Method::GET,
        "/",
        &[("if-match", HELLO_ETAG), ("if-none-match", HELLO_ETAG)],
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn assets_keep_immutable_caching_and_mime_types() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for (path, mime) in [
        ("/assets/style-hash.css", "text/css"),
        ("/assets/font-hash.woff2", "font/woff2"),
        ("/assets/data.unknown", "application/octet-stream"),
    ] {
        fs::write(dir.path().join(path.trim_start_matches('/')), b"hello").unwrap();
        let app = router(dir.path()).unwrap();
        for method in [Method::GET, Method::HEAD] {
            let res = request(&app, method.clone(), path, &[]).await;
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(res.headers()[header::CONTENT_TYPE], mime);
            assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
            assert_eq!(res.headers()[header::CACHE_CONTROL], IMMUTABLE);
            assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
            assert_eq!(
                body(res).await,
                if method == Method::HEAD {
                    b"".as_slice()
                } else {
                    b"hello"
                }
            );
        }
    }
    let res = request(&app, Method::GET, "/assets/app-hash.js?v=1", &[]).await;
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    assert_eq!(res.headers()[header::CACHE_CONTROL], IMMUTABLE);
    let res = request(
        &app,
        Method::GET,
        "/assets/app-hash.js",
        &[("if-none-match", HELLO_ETAG)],
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers()[header::CACHE_CONTROL], IMMUTABLE);
    assert!(body(res).await.is_empty());
}

#[tokio::test]
async fn range_and_date_headers_do_not_override_content_validation() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for value in ["bytes=1-3", "bytes=999-1000", "bytes=0-1,3-4"] {
        let res = request(
            &app,
            Method::GET,
            "/",
            &[("range", value), ("if-range", HELLO_ETAG)],
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::ACCEPT_RANGES], "none");
        assert!(!res.headers().contains_key(header::CONTENT_RANGE));
        assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
        assert_eq!(body(res).await, b"hello");
    }
    let fields = [
        ("if-modified-since", "Fri, 01 Jan 2100 00:00:00 GMT"),
        ("if-unmodified-since", "Thu, 01 Jan 1970 00:00:00 GMT"),
    ];
    let res = request(&app, Method::GET, "/", &fields).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(!res.headers().contains_key(header::LAST_MODIFIED));
    assert_eq!(body(res).await, b"hello");
    let res = request(&app, Method::HEAD, "/", &[("range", "bytes=1-3")]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
    assert!(body(res).await.is_empty());
    let res = request(
        &app,
        Method::GET,
        "/",
        &[("range", "bytes=1-3"), ("if-none-match", HELLO_ETAG)],
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert!(body(res).await.is_empty());
}

#[tokio::test]
async fn missing_files_keep_the_html_404_fallback() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for path in ["/login", "/assets/missing.js", "/index.html/child"] {
        for method in [Method::GET, Method::HEAD] {
            let res = request(
                &app,
                method.clone(),
                path,
                &[("if-none-match", "*"), ("if-match", "*")],
            )
            .await;
            assert_eq!(res.status(), StatusCode::NOT_FOUND);
            assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
            assert_eq!(res.headers()[header::CONTENT_TYPE], "text/html");
            assert_eq!(res.headers()[header::CONTENT_LENGTH], "5");
            assert!(!res.headers().contains_key(header::ETAG));
            assert!(!res.headers().contains_key(header::LAST_MODIFIED));
            assert_eq!(
                body(res).await,
                if method == Method::HEAD {
                    b"".as_slice()
                } else {
                    b"hello"
                }
            );
        }
    }
}

#[tokio::test]
async fn empty_files_have_valid_etags_and_lengths() {
    let dir = fixture();
    fs::write(dir.path().join("index.html"), b"").unwrap();
    let app = router(dir.path()).unwrap();
    for method in [Method::GET, Method::HEAD] {
        let res = request(&app, method.clone(), "/", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::ETAG], EMPTY_ETAG);
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "0");
        assert!(body(res).await.is_empty());
        let res = request(&app, method, "/", &[("if-none-match", EMPTY_ETAG)]).await;
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(res.headers()[header::CONTENT_LENGTH], "0");
        assert!(body(res).await.is_empty());
    }
}

#[tokio::test]
async fn missing_index_returns_404_and_missing_root_fails_construction() {
    let dir = tempdir("mandara-frontend-empty-");
    let app = router(dir.path()).unwrap();
    for path in ["/", "/missing"] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert!(!res.headers().contains_key(header::ETAG));
        assert!(body(res).await.is_empty());
    }
    let error = router(&dir.path().join("absent")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[tokio::test]
async fn invalid_paths_cannot_traverse_outside_the_frontend_root() {
    let parent = tempdir("mandara-frontend-paths-");
    let root = parent.path().join("dist");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("index.html"), b"hello").unwrap();
    fs::write(parent.path().join("secret.txt"), b"private content").unwrap();
    let app = router(&root).unwrap();
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/..%5csecret.txt",
        "/%00",
    ] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(body(res).await.is_empty(), "{path}");
    }
    let res = request(&app, Method::GET, "/%ff", &[]).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn encoded_filenames_and_directory_indexes_are_served() {
    let dir = fixture();
    fs::write(dir.path().join("示例.txt"), b"hello").unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested/index.html"), b"hello").unwrap();
    let app = router(dir.path()).unwrap();
    for (path, mime) in [
        ("/%E7%A4%BA%E4%BE%8B.txt?q=1", "text/plain"),
        ("/nested/", "text/html"),
    ] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], mime);
        assert_eq!(res.headers()[header::ETAG], HELLO_ETAG);
        assert_eq!(body(res).await, b"hello");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_escape_the_root_including_directory_indexes() {
    use std::os::unix::fs::symlink;

    let parent = tempdir("mandara-frontend-links-");
    let root = parent.path().join("dist");
    let outside = parent.path().join("dist-extra");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(root.join("index.html"), b"hello").unwrap();
    let secret = outside.join("secret.txt");
    fs::write(&secret, b"private content").unwrap();
    symlink(&secret, root.join("link.txt")).unwrap();
    symlink(&outside, root.join("outside")).unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    symlink(&secret, root.join("nested/index.html")).unwrap();
    symlink(root.join("index.html"), root.join("local.html")).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    let app = router(&root).unwrap();
    for path in [
        "/link.txt",
        "/outside/secret.txt",
        "/nested/",
        "/loop/index.html",
    ] {
        let res = request(&app, Method::GET, path, &[]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(body(res).await.is_empty(), "{path}");
    }
    let res = request(&app, Method::GET, "/local.html", &[]).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body(res).await, b"hello");
}

#[tokio::test]
async fn unsupported_methods_return_405() {
    let dir = fixture();
    let app = router(dir.path()).unwrap();
    for method in [
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ] {
        let res = request(&app, method, "/", &[]).await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        let allowed = res.headers()[header::ALLOW].to_str().unwrap();
        assert!(allowed.contains("GET"));
        assert!(allowed.contains("HEAD"));
        assert!(!res.headers().contains_key(header::ETAG));
    }
}

#[tokio::test]
async fn frontend_fallback_does_not_intercept_api_responses() {
    let dir = fixture();
    let app = Router::new()
        .route(
            "/api/test",
            axum::routing::get(|| async { ([(header::ETAG, "\"api\"")], "api") }),
        )
        .fallback_service(router(dir.path()).unwrap());
    let res = request(
        &app,
        Method::GET,
        "/api/test",
        &[("if-none-match", "\"api\"")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::ETAG], "\"api\"");
    assert!(!res.headers().contains_key(header::CACHE_CONTROL));
    assert_eq!(body(res).await, b"api");
}
