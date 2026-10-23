use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chapter_retry_repairs_only_the_missing_body_and_persists_it() {
    let fixture = Fixture::with_plugin(true).await;
    fixture
        .state
        .library
        .plugins()
        .set_config("fixture", json!({"site-name":"示例"}))
        .await
        .unwrap();
    fixture.sql("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, content_mode, chapter_count)
        VALUES ('stored', 'book', 'fixture', 'hello-content-1', 'plugin', 'public', 'user', 'chapters', 3);
        INSERT INTO chapters (file_id, idx, title, content) VALUES
            ('stored', 0, '保存的正文标题', '<p>保存的正文</p>'),
            ('stored', 1, '等待下载', NULL), ('stored', 2, '空章', '');
        INSERT INTO shares (token, kind, file_id, created_by) VALUES ('share', 'book', 'stored', 'user');").await;
    // Any authenticated reader can repair public content without clearing other chapters.
    let token = fixture.token("other", Role::User);
    let uri = "/api/files/stored/chapters/1/retry";
    let (first, second) = tokio::join!(
        request(fixture.app(), "POST", uri, Some(&token), ""),
        request(fixture.app(), "POST", uri, Some(&token), "")
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let repaired = response_json(first).await;
    assert_eq!(repaired, response_json(second).await);
    assert_eq!(repaired["idx"], 1);
    assert!(!repaired["content"].as_str().unwrap().is_empty());
    // Once persisted, both authenticated and public reads work with the source disabled.
    fixture
        .state
        .library
        .plugins()
        .set_enabled("fixture", false)
        .await
        .unwrap();
    for (prefix, auth) in [
        ("/api/files/stored/chapters", Some(token.as_str())),
        ("/api/shares/share/chapters", None),
    ] {
        let response = request(fixture.app(), "GET", &format!("{prefix}/1"), auth, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, repaired);
        for (idx, content) in [(0, "<p>保存的正文</p>"), (2, "")] {
            let response =
                request(fixture.app(), "GET", &format!("{prefix}/{idx}"), auth, "").await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await["content"], content);
        }
    }
    for (idx, content) in [(0, "<p>保存的正文</p>"), (2, "")] {
        let response = request(
            fixture.app(),
            "POST",
            &format!("/api/files/stored/chapters/{idx}/retry"),
            Some(&token),
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["content"], content);
    }
}

#[tokio::test]
async fn chapter_retry_enforces_auth_visibility_and_stored_directory_bounds() {
    let fixture = Fixture::new().await;
    fixture.sql("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, content_mode, chapter_count)
        VALUES ('private', 'book', 'local', 'private', 'txt', 'private', 'user', 'file', 1);
        INSERT INTO chapters (file_id, idx, title, content) VALUES ('private', 0, '第一章', '');").await;
    let uri = "/api/files/private/chapters/0/retry";
    assert_eq!(
        request(fixture.app(), "POST", uri, None, "").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let other = fixture.token("other", Role::User);
    assert_eq!(
        request(fixture.app(), "POST", uri, Some(&other), "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for (id, role) in [("user", Role::User), ("admin", Role::Admin)] {
        let token = fixture.token(id, role);
        assert_eq!(
            request(fixture.app(), "POST", uri, Some(&token), "")
                .await
                .status(),
            StatusCode::OK
        );
        let response = request(
            fixture.app(),
            "POST",
            "/api/files/private/chapters/99/retry",
            Some(&token),
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response_json(response).await, json!({"error":"chapter"}));
    }
    // An absent directory is not discovered, even for a valid source chapter index.
    fixture.sql("DELETE FROM chapters;").await;
    let token = fixture.token("user", Role::User);
    let response = request(fixture.app(), "POST", uri, Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_json(response).await, json!({"error":"chapter"}));
}

#[tokio::test]
async fn chapter_retry_uses_retained_uploads_and_can_retry_a_failed_download() {
    let fixture = Fixture::new().await;
    let text = "第一章 示例\n修复后的正文。\n";
    let hash: String = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fixture.sql(&format!("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, content_mode, chapter_count, orig_ext, orig_size, orig_sha256)
        VALUES ('upload', 'book', 'local', 'upload', 'txt', 'private', 'user', 'file', 1, 'txt', {}, '{hash}');
        INSERT INTO chapters (file_id, idx, title, content) VALUES ('upload', 0, '第一章', NULL);", text.len())).await;
    let token = fixture.token("user", Role::User);
    let uri = "/api/files/upload/chapters/0/retry";
    // The retained file is initially missing. Failed writers must not poison future retries.
    assert_eq!(
        request(fixture.app(), "POST", uri, Some(&token), "")
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let files = fixture.dir.path().join("files");
    tokio::fs::create_dir_all(&files).await.unwrap();
    tokio::fs::write(files.join("upload.txt"), text)
        .await
        .unwrap();
    let response = request(fixture.app(), "POST", uri, Some(&token), "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["content"].as_str().unwrap().contains("修复后的正文"));
    let response = request(
        fixture.app(),
        "GET",
        "/api/files/upload/chapters/0",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, body);
}
