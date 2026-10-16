use diesel::QueryDsl as _;
use diesel_async::RunQueryDsl as _;

use super::*;

async fn acquire(
    fixture: &Fixture,
    token: &str,
    fields: &[(&str, &str)],
    upload: bool,
) -> Response {
    let boundary = "mandara-visibility-test";
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    if upload {
        body.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"示例.txt\"\r\nContent-Type: text/plain\r\n\r\n第一章 正文\n本地上传正文。\r\n"));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    fixture
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/books")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn plugin_content_defaults_public_and_rejects_private_before_writing() {
    let f = Fixture::with_plugin(true).await;
    f.state
        .library
        .plugins()
        .set_config("fixture", json!({"site-name":"示例"}))
        .await
        .unwrap();
    f.sql("INSERT INTO books(id,title,created_by) VALUES('book','书籍','user');")
        .await;
    let owner = f.token("user", Role::User);
    let other = f.token("other", Role::User);
    let fields = [
        ("book_id", "book"),
        ("plugin_source", "fixture"),
        ("plugin_book_id", "hello-content-1"),
    ];
    let mut private = fields.to_vec();
    private.push(("visibility", "private"));
    let response = acquire(&f, &owner, &private, false).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"],
        "plugin content must be public"
    );
    assert!(
        f.state
            .library
            .files_of_book(
                "book",
                &User {
                    id: "user".into(),
                    username: "user".into(),
                    role: Role::Admin,
                    created_at: Utc::now(),
                }
            )
            .await
            .unwrap()
            .is_empty()
    );

    let response = acquire(&f, &owner, &fields, false).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response_json(response).await;
    let file = &body["books"][0]["files"][0];
    assert_eq!(file["visibility"], "public");
    let id = file["id"].as_str().unwrap();
    let uri = format!("/api/files/{id}");
    assert_eq!(
        request(f.app(), "GET", &uri, Some(&other), "")
            .await
            .status(),
        StatusCode::OK
    );
    // Owners and administrators cannot hide plugin content or partly apply a patch.
    for token in [&owner, &f.token("admin", Role::Admin)] {
        let response = request(
            f.app(),
            "PATCH",
            &uri,
            Some(token),
            r#"{"visibility":"private","label":"不应保存"}"#,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let unchanged = f.state.library.get_file(id).await.unwrap().unwrap();
        assert_eq!(unchanged.visibility, mandara_core::Visibility::Public);
        assert_ne!(unchanged.label, "不应保存");
    }
    assert_eq!(
        request(f.app(), "PATCH", &uri, Some(&other), r#"{"label":"他人"}"#)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = request(
        f.app(),
        "PATCH",
        &uri,
        Some(&owner),
        r#"{"visibility":"public","label":"公开版本"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["label"], "公开版本");

    // A parsed plugin EPUB is still plugin content, not a local upload.
    f.sql("UPDATE book_files SET format='epub',content_mode='file';")
        .await;
    assert_eq!(
        request(
            f.app(),
            "PATCH",
            &uri,
            Some(&owner),
            r#"{"visibility":"private"}"#
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn uploads_keep_private_default_even_with_plugin_metadata() {
    let f = Fixture::with_plugin(true).await;
    f.state
        .library
        .plugins()
        .set_config("fixture", json!({"site-name":"示例"}))
        .await
        .unwrap();
    let owner = f.token("user", Role::User);
    let other = f.token("other", Role::User);
    // This fixture identifies 示例.txt on upload but has no metadata lookup export.
    let response = acquire(&f, &owner, &[], true).await;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let metadata_source = crate::schema::books::table
        .find(body["books"][0]["book"]["id"].as_str().unwrap())
        .select(crate::schema::books::meta_source)
        .first::<Option<String>>(&mut f.state.diesel_db.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(metadata_source.as_deref(), Some("fixture"));
    let file = &body["books"][0]["files"][0];
    assert_eq!(file["source"], "local");
    assert_eq!(file["visibility"], "private");
    let uri = format!("/api/files/{}", file["id"].as_str().unwrap());
    assert_eq!(
        request(f.app(), "GET", &uri, Some(&other), "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for visibility in ["public", "private"] {
        let response = request(
            f.app(),
            "PATCH",
            &uri,
            Some(&owner),
            &json!({"visibility":visibility}).to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["visibility"], visibility);
    }
    let response = acquire(&f, &owner, &[("visibility", "public")], true).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response_json(response).await["books"][0]["files"][0]["visibility"],
        "public"
    );
}
