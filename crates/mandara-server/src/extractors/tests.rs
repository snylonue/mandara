use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::{Body, to_bytes};
use axum::extract::Multipart;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use diesel_async::SimpleAsyncConnection as _;
use mandara_core::model::{Role, User};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{AdminUser, AuthenticatedUser};
use crate::auth::AuthService;
use crate::config::Config;
use crate::service::{Library, plugins::PluginService};
use crate::state::{AppState, St};

mod numbers;
mod plugin_visibility;
mod sessions;
mod shares;

const SECRET: &str = "extractor-tests-only";

struct Fixture {
    state: St,
    dir: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_plugin(false).await
    }

    async fn with_plugin(load_plugin: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("mandara-auth-{}", uuid::Uuid::new_v4()));
        let db = dir.join("test.db");
        crate::db::run_migrations(&db).await.unwrap();
        let diesel_db = crate::db::connect_diesel(&db).unwrap();
        diesel_db
            .get()
            .await
            .unwrap()
            .batch_execute(
                "INSERT INTO users (id, username, password_hash, role) VALUES
             ('admin', 'admin', '', 'admin'), ('user', 'user', '', 'user'),
             ('other', 'other', '', 'user');",
            )
            .await
            .unwrap();
        let runtime = Arc::new(
            mandara_plugin::PluginRuntime::new(
                mandara_plugin::FetchPolicy::default(),
                crate::service::images::DbImageStore::new(diesel_db.clone(), dir.join("files")),
            )
            .unwrap(),
        );
        let wasm = if load_plugin {
            vec![
                runtime
                    .compile(
                        "fixture.wasm",
                        include_bytes!("../../../mandara-plugin/tests/fixtures/hello.wasm"),
                    )
                    .unwrap(),
            ]
        } else {
            vec![]
        };
        let plugins = Arc::new(
            PluginService::new(diesel_db.clone(), runtime, wasm)
                .await
                .unwrap(),
        );
        if load_plugin {
            plugins.register("fixture", "fixture.wasm").await.unwrap();
        }
        let library = Library::new(diesel_db.clone(), plugins, dir.join("files"));
        let cfg = Config {
            addr: "127.0.0.1:0".into(),
            db,
            data_dir: Some(dir.clone()),
            plugins_dir: dir.join("plugins"),
            jwt_secret: SECRET.into(),
            jwt_secret_file: None,
            allow_register: true,
            cookie_secure: false,
            max_upload_mb: 64,
            frontend_dir: dir.join("frontend"),
            reparse_originals: false,
        };
        let state = Arc::new(AppState {
            cfg,
            diesel_db,
            auth: AuthService::new(true, SECRET, false),
            library,
        });
        Self { state, dir }
    }

    fn app(&self) -> Router {
        crate::routes::router(self.state.clone())
    }

    fn token(&self, id: &str, role: Role) -> String {
        self.state
            .auth
            .issue_token(&User {
                id: id.into(),
                username: id.into(),
                role,
                created_at: Utc::now(),
            })
            .unwrap()
    }

    async fn sql(&self, sql: &str) {
        self.state
            .diesel_db
            .get()
            .await
            .unwrap()
            .batch_execute(sql)
            .await
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.dir.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            self.dir
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("mandara-auth-")
        );
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

async fn request(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: &str,
) -> Response {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn authenticated_chapter_reads_recover_cache_misses_but_shares_do_not() {
    let fixture = Fixture::with_plugin(true).await;
    fixture
        .state
        .library
        .plugins()
        .set_config("fixture", json!({"site-name": "示例"}))
        .await
        .unwrap();
    fixture.sql("INSERT INTO books(id,title) VALUES('book','示例');
        INSERT INTO book_files(id,book_id,source,external_id,format,visibility,owner_id,chapter_count)
        VALUES('file','book','fixture','hello-content-1','plugin','public','user',2);
        INSERT INTO chapters(file_id,idx,title,content) VALUES('file',0,'第一章',NULL);").await;
    let token = fixture.token("user", Role::User);
    let uri = "/api/files/file/chapters/0";
    assert_eq!(
        request(fixture.app(), "GET", uri, None, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let share = request(
        fixture.app(),
        "POST",
        "/api/files/file/shares",
        Some(&token),
        "{}",
    )
    .await;
    assert_eq!(share.status(), StatusCode::CREATED);
    let share = response_json(share).await;
    let shared = format!(
        "/api/shares/{}/chapters/0",
        share["token"].as_str().unwrap()
    );
    assert_eq!(
        request(fixture.app(), "GET", &shared, None, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let chapter = request(fixture.app(), "GET", uri, Some(&token), "").await;
    assert_eq!(chapter.status(), StatusCode::OK);
    let chapter = response_json(chapter).await;
    assert!(!chapter["content"].as_str().unwrap().is_empty());
    let missing_row = request(
        fixture.app(),
        "GET",
        "/api/files/file/chapters/1",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(missing_row.status(), StatusCode::OK);
    fixture
        .sql("UPDATE book_files SET source='unavailable' WHERE id='file';")
        .await;
    for (path, auth) in [(uri, Some(token.as_str())), (shared.as_str(), None)] {
        let cached = request(fixture.app(), "GET", path, auth, "").await;
        assert_eq!(cached.status(), StatusCode::OK);
        assert_eq!(response_json(cached).await, chapter);
    }
    assert_eq!(
        request(
            fixture.app(),
            "GET",
            "/api/files/file/chapters/2",
            Some(&token),
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn plugin_search_returns_the_catalog_envelope_for_an_empty_result() {
    let fixture = Fixture::with_plugin(true).await;
    fixture
        .state
        .library
        .plugins()
        .set_config("fixture", json!({"site-name": "示例"}))
        .await
        .unwrap();
    let token = fixture.token("user", Role::User);
    let response = request(
        fixture.app(),
        "GET",
        "/api/plugins/fixture/search?q=hello",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"total": 0, "items": []})
    );
}

#[tokio::test]
async fn book_list_attaches_series_without_dropping_standalone_books() {
    let fixture = Fixture::new().await;
    fixture
        .sql(
            "INSERT INTO series (id, title) VALUES ('a', '甲系列'), ('b', '乙系列');
         INSERT INTO books (id, title, created_by, series_id, volume_no) VALUES
         ('a-first', '甲系列第一卷', 'user', 'a', 1),
         ('a-second', '甲系列第二卷', 'user', 'a', 2),
         ('b-only', '乙系列第一卷', 'user', 'b', 1),
         ('standalone', '独立书籍', 'user', NULL, 0);",
        )
        .await;
    let token = fixture.token("user", Role::User);
    let response = request(fixture.app(), "GET", "/api/books", Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let entries = body.as_array().unwrap();
    let expected = [
        ("a-first", Some("a")),
        ("a-second", Some("a")),
        ("b-only", Some("b")),
        ("standalone", None),
    ];
    assert_eq!(entries.len(), expected.len());
    for (entry, (book_id, series_id)) in entries.iter().zip(expected) {
        assert_eq!(entry["book"]["id"], book_id);
        assert_eq!(entry["series"]["id"].as_str(), series_id);
        if series_id.is_none() {
            assert!(entry["series"].is_null());
        }
    }
    fixture.sql("DELETE FROM series;").await;
    let response = request(fixture.app(), "GET", "/api/books", Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), expected.len());
    assert!(entries.iter().all(|entry| entry["series"].is_null()));
}

#[tokio::test]
async fn book_and_series_responses_keep_streamed_groups_aligned_across_batches() {
    let fixture = Fixture::new().await;
    fixture
        .sql(
            "INSERT INTO series (id, title, created_by) VALUES ('a', '甲系列', 'user');
         WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO books (id, title, created_by, series_id, volume_no)
         SELECT printf('book-%04d', i), '系列分卷', 'user', 'a', i FROM n;
         INSERT INTO books (id, title, created_by) VALUES ('standalone', '独立书籍', 'user');
         INSERT INTO book_files (id, book_id, source, external_id, visibility, created_at)
         SELECT 'file-' || id, id, 'local', 'file-' || id, 'public', '2024-01-01T00:00:00Z'
         FROM books WHERE id LIKE 'book-%' AND id != 'book-0250';
         INSERT INTO book_files (id, book_id, source, external_id, visibility, created_at)
         VALUES ('newer', 'book-0500', 'local', 'newer', 'public', '2025-01-01T00:00:00Z'),
                ('hidden', 'book-0000', 'local', 'hidden', 'private', '2025-01-01T00:00:00Z');",
        )
        .await;
    let token = fixture.token("user", Role::User);
    let response = request(fixture.app(), "GET", "/api/books", Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 502);
    for (index, entry) in entries[..501].iter().enumerate() {
        let id = format!("book-{index:04}");
        assert_eq!(entry["book"]["id"], id);
        assert_eq!(entry["series"]["id"], "a");
        let files = entry["files"].as_array().unwrap();
        assert_eq!(
            files.len(),
            match index {
                250 => 0,
                500 => 2,
                _ => 1,
            }
        );
        assert!(files.iter().all(|file| file["book_id"] == id));
        assert!(files.iter().all(|file| file["id"] != "hidden"));
    }
    assert_eq!(entries[500]["files"][0]["id"], "newer");
    assert_eq!(entries[501]["book"]["id"], "standalone");
    assert!(entries[501]["series"].is_null());
    assert!(entries[501]["files"].as_array().unwrap().is_empty());

    let response = request(fixture.app(), "GET", "/api/series/a", Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let detail = response_json(response).await;
    let members = detail["books"].as_array().unwrap();
    assert_eq!(members.len(), 501);
    for (member, entry) in members.iter().zip(entries) {
        assert_eq!(member["book"], entry["book"]);
        assert_eq!(member["files"], entry["files"]);
        assert_eq!(member["series"]["id"], "a");
    }
}

#[tokio::test]
async fn bearer_and_cookie_resolve_the_database_user() {
    let fixture = Fixture::new().await;
    let token = fixture.token("user", Role::User);
    let response = request(fixture.app(), "GET", "/api/auth/me", Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["id"], "user");
    let response = fixture
        .app()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::COOKIE, format!("mandara_token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["role"], "user");
    // An invalid bearer token must not be rescued by a valid cookie.
    let response = fixture
        .app()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::COOKIE, format!("mandara_token={token}"))
                .header(header::AUTHORIZATION, "Bearer invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invalid_expired_and_deleted_credentials_are_unauthorized() {
    let fixture = Fixture::new().await;
    let expired = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({"sub": "user", "username": "user", "role": "user", "iat": 0, "exp": 0}),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    let wrong_signature = AuthService::new(true, "wrong-secret", false)
        .issue_token(&User {
            id: "user".into(),
            username: "user".into(),
            role: Role::User,
            created_at: Utc::now(),
        })
        .unwrap();
    let deleted = fixture.token("other", Role::User);
    fixture.sql("DELETE FROM users WHERE id='other'").await;
    for token in [
        None,
        Some("invalid"),
        Some(expired.as_str()),
        Some(wrong_signature.as_str()),
        Some(deleted.as_str()),
    ] {
        for (method, uri) in [("GET", "/api/auth/me"), ("POST", "/api/plugins/sync")] {
            let response = request(fixture.app(), method, uri, token, "").await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response_json(response).await,
                json!({"error": "unauthorized"})
            );
        }
    }
}

#[tokio::test]
async fn content_preview_confirms_independent_ids_without_importing() {
    use diesel::QueryDsl as _;
    use diesel_async::RunQueryDsl as _;

    let fixture = Fixture::with_plugin(true).await;
    let token = fixture.token("user", Role::User);
    let response = request(
        fixture.app(),
        "GET",
        "/api/plugins/fixture/content?q=hello-content-1",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "id": "hello-content-1", "mode": "chapters", "chapter_count": 2
        })
    );
    let response = request(
        fixture.app(),
        "GET",
        "/api/plugins/fixture/content?q=hello-1",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let mut conn = fixture.state.diesel_db.get().await.unwrap();
    assert_eq!(
        crate::schema::books::table
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        crate::schema::book_files::table
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        crate::schema::chapters::table
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .unwrap(),
        0
    );
    drop(conn);
    fixture
        .state
        .library
        .plugins()
        .set_enabled("fixture", false)
        .await
        .unwrap();
    let response = request(
        fixture.app(),
        "GET",
        "/api/plugins/fixture/content?q=hello-content-1",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn content_preview_rejects_blank_ids_and_missing_sources() {
    let fixture = Fixture::new().await;
    let token = fixture.token("user", Role::User);
    for (uri, expected) in [
        (
            "/api/plugins/missing/content?q=%20",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/plugins/missing/content?q=independent",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = request(fixture.app(), "GET", uri, Some(&token), "").await;
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn admin_guard_uses_current_database_role_not_jwt_role() {
    let fixture = Fixture::new().await;
    let admin = fixture.token("admin", Role::Admin);
    let user = fixture.token("user", Role::User);
    let response = request(fixture.app(), "POST", "/api/plugins/sync", Some(&user), "").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response_json(response).await, json!({"error": "forbidden"}));
    let response = request(fixture.app(), "POST", "/api/plugins/sync", Some(&admin), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, json!({"synced": 0}));
    fixture.sql("UPDATE users SET role='user' WHERE id='admin'; UPDATE users SET role='admin' WHERE id='user';").await;
    assert_eq!(
        request(fixture.app(), "POST", "/api/plugins/sync", Some(&admin), "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(fixture.app(), "POST", "/api/plugins/sync", Some(&user), "")
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn protected_routes_authenticate_before_parsing_request_data() {
    let fixture = Fixture::new().await;
    let admin_routes = [
        ("POST", "/api/plugins/sync"),
        ("POST", "/api/plugins/instances/id/sync"),
        ("GET", "/api/plugins/wasm-files"),
        ("POST", "/api/plugins/instances"),
        ("DELETE", "/api/plugins/instances/id"),
        ("PUT", "/api/plugins/instances/id/enabled"),
        ("GET", "/api/plugins/id/config-schema"),
        ("PUT", "/api/plugins/id/config"),
    ];
    let user_routes = [
        ("GET", "/api/auth/me"),
        ("GET", "/api/plugins"),
        ("GET", "/api/plugins/id/search?limit=invalid"),
        ("POST", "/api/plugins/id/books"),
        ("GET", "/api/plugins/id/series"),
        ("POST", "/api/plugins/id/series"),
        ("GET", "/api/plugins/id/lookup"),
        ("GET", "/api/plugins/id/content"),
        ("GET", "/api/books"),
        ("POST", "/api/books"),
        ("POST", "/api/books/id/files"),
        ("GET", "/api/books/id"),
        ("PATCH", "/api/books/id"),
        ("POST", "/api/books/id/refresh"),
        ("DELETE", "/api/books/id"),
        ("GET", "/api/files/id"),
        ("GET", "/api/files/id/chapters/invalid"),
        ("GET", "/api/files/id/download"),
        ("PATCH", "/api/files/id"),
        ("POST", "/api/files/id/rematerialize"),
        ("DELETE", "/api/files/id"),
        ("GET", "/api/sessions"),
        ("GET", "/api/files/id/sessions"),
        ("POST", "/api/sessions"),
        ("GET", "/api/sessions/id/progress"),
        ("PUT", "/api/sessions/id/files/file/progress"),
        ("POST", "/api/sessions/id/files/file/sync"),
        ("DELETE", "/api/sessions/id"),
        ("GET", "/api/series"),
        ("POST", "/api/series"),
        ("GET", "/api/series/id"),
        ("PATCH", "/api/series/id"),
        ("PUT", "/api/series/id/members"),
        ("DELETE", "/api/series/id"),
        ("POST", "/api/files/id/shares"),
        ("DELETE", "/api/shares/token"),
    ];
    for (method, uri) in admin_routes.into_iter().chain(user_routes) {
        assert_eq!(
            request(fixture.app(), method, uri, None, "{")
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
    let user = fixture.token("user", Role::User);
    for (method, uri) in admin_routes {
        assert_eq!(
            request(fixture.app(), method, uri, Some(&user), "{")
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{method} {uri}"
        );
    }
    let admin = fixture.token("admin", Role::Admin);
    assert_eq!(
        request(
            fixture.app(),
            "POST",
            "/api/plugins/instances",
            Some(&admin),
            "{"
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            fixture.app(),
            "GET",
            "/api/plugins/id/search?limit=invalid",
            Some(&user),
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            fixture.app(),
            "GET",
            "/api/files/id/chapters/invalid",
            Some(&user),
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chapter_detail_endpoints_return_only_stored_directories() {
    let fixture = Fixture::with_plugin(true).await;
    fixture.sql("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, content_mode, chapter_count)
        VALUES ('stored', 'book', 'fixture', 'hello-content-1', 'plugin', 'public', 'user', 'chapters', 2);
        INSERT INTO chapters (file_id, idx, title, content) VALUES ('stored', 0, '保存的目录标题', NULL);
        INSERT INTO shares (token, kind, file_id, created_by) VALUES ('share', 'book', 'stored', 'user');").await;
    let token = fixture.token("user", Role::User);
    for expected in [
        json!([{"idx":0,"title":"保存的目录标题","linear":true}]),
        json!([]),
    ] {
        for (uri, auth) in [
            ("/api/files/stored", Some(token.as_str())),
            ("/api/shares/share/book", None),
        ] {
            let response = request(fixture.app(), "GET", uri, auth, "").await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            assert_eq!(body["chapters"], expected);
            assert_eq!(body["file"]["chapter_count"], 2);
            assert_eq!(
                body["toc"].as_array().unwrap().len(),
                expected.as_array().unwrap().len()
            );
        }
        fixture
            .sql("DELETE FROM chapters WHERE file_id='stored';")
            .await;
    }
    // An unavailable source must not block either read endpoint.
    fixture
        .sql("UPDATE book_files SET source='unavailable' WHERE id='stored';")
        .await;
    for (uri, auth) in [
        ("/api/files/stored", Some(token.as_str())),
        ("/api/shares/share/book", None),
    ] {
        let response = request(fixture.app(), "GET", uri, auth, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["chapters"], json!([]));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chapter_reads_preserve_empty_bodies_and_shares_do_not_download() {
    let fixture = Fixture::with_plugin(true).await;
    fixture.sql("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, content_mode, chapter_count)
        VALUES ('stored', 'book', 'fixture', 'hello-content-1', 'plugin', 'public', 'user', 'chapters', 3);
        INSERT INTO chapters (file_id, idx, title, content) VALUES
            ('stored', 0, '保存的正文标题', '<p>保存的正文</p>'),
            ('stored', 1, '等待下载', NULL), ('stored', 2, '空章', '');
        INSERT INTO shares (token, kind, file_id, created_by) VALUES ('share', 'book', 'stored', 'user');").await;
    let token = fixture.token("user", Role::User);
    for (prefix, auth) in [
        ("/api/files/stored/chapters", Some(token.as_str())),
        ("/api/shares/share/chapters", None),
    ] {
        for (idx, content) in [(0, "<p>保存的正文</p>"), (2, "")] {
            let response =
                request(fixture.app(), "GET", &format!("{prefix}/{idx}"), auth, "").await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await["content"], content);
        }
    }
    // Anonymous reads cannot use the available plugin to fill chapter 1.
    for idx in [1, 99] {
        let response = request(
            fixture.app(),
            "GET",
            &format!("/api/shares/share/chapters/{idx}"),
            None,
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response_json(response).await,
            json!({"error":"stored chapter"})
        );
    }
    let file = fixture
        .state
        .library
        .get_file("stored")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fixture.state.library.get_stored_chapter(&file, 1).await,
        Err(crate::error::ApiError::NotFound(_))
    ));
}

#[tokio::test]
async fn guards_preserve_bodies_and_do_not_run_denied_handlers() {
    let fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let json_calls = calls.clone();
    let multipart_calls = calls.clone();
    let app = Router::new()
        .route(
            "/json",
            post(
                move |AdminUser(_admin): AdminUser, Json(body): Json<Value>| {
                    let calls = json_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Json(body)
                    }
                },
            ),
        )
        .route(
            "/multipart",
            post(
                move |AuthenticatedUser(_user): AuthenticatedUser, mut multipart: Multipart| {
                    let calls = multipart_calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        multipart
                            .next_field()
                            .await
                            .unwrap()
                            .unwrap()
                            .text()
                            .await
                            .unwrap()
                    }
                },
            ),
        )
        .with_state(fixture.state.clone());
    let user = fixture.token("user", Role::User);
    let admin = fixture.token("admin", Role::Admin);
    assert_eq!(
        request(app.clone(), "POST", "/json", None, "{")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(app.clone(), "POST", "/json", Some(&user), "{")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(app.clone(), "POST", "/multipart", None, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let response = request(
        app.clone(),
        "POST",
        "/json",
        Some(&admin),
        r#"{"value":42}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, json!({"value": 42}));
    let response = app.oneshot(Request::builder().method("POST").uri("/multipart")
        .header(header::AUTHORIZATION, format!("Bearer {user}"))
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=test-boundary")
        .body(Body::from("--test-boundary\r\nContent-Disposition: form-data; name=\"value\"\r\n\r\n正文\r\n--test-boundary--\r\n")).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        "正文".as_bytes()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn resource_permissions_and_public_routes_are_unchanged() {
    let fixture = Fixture::new().await;
    fixture.sql("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, content_mode)
        VALUES ('private', 'book', 'local', 'private', 'txt', 'private', 'user', 'file'),
               ('public', 'book', 'local', 'public', 'txt', 'public', 'user', 'file');
        INSERT INTO shares (token, kind, file_id, created_by) VALUES ('share', 'book', 'public', 'user');").await;
    let owner = fixture.token("user", Role::User);
    let other = fixture.token("other", Role::User);
    let admin = fixture.token("admin", Role::Admin);
    for token in [&owner, &admin] {
        assert_eq!(
            request(fixture.app(), "GET", "/api/files/private", Some(token), "")
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        request(fixture.app(), "GET", "/api/files/private", Some(&other), "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(fixture.app(), "GET", "/api/files/public", Some(&other), "")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            fixture.app(),
            "PATCH",
            "/api/files/public",
            Some(&other),
            "{}"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            fixture.app(),
            "DELETE",
            "/api/shares/share",
            Some(&other),
            ""
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for (method, uri, expected) in [
        ("GET", "/api/health", StatusCode::OK),
        ("POST", "/api/auth/logout", StatusCode::NO_CONTENT),
        ("GET", "/api/books/book/cover", StatusCode::NOT_FOUND),
        ("GET", "/api/images/invalid", StatusCode::NOT_FOUND),
        ("GET", "/api/shares/share", StatusCode::OK),
        ("GET", "/api/shares/share/book", StatusCode::OK),
        ("GET", "/api/shares/share/chapters/0", StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            request(fixture.app(), method, uri, None, "").await.status(),
            expected,
            "{method} {uri}"
        );
    }
}
