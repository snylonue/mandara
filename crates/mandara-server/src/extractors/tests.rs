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

const SECRET: &str = "extractor-tests-only";

struct Fixture {
    state: St,
    dir: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
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
        let plugins = Arc::new(PluginService::new(diesel_db.clone(), vec![]));
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
        ("POST", "/api/files/id/sessions"),
        ("PUT", "/api/sessions/id"),
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
