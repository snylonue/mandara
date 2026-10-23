use super::*;

async fn shared_fixture() -> Fixture {
    let f = Fixture::with_plugin(true).await;
    f.state
        .library
        .plugins()
        .set_config("fixture", json!({"site-name": "示例"}))
        .await
        .unwrap();
    f.sql("INSERT INTO books (id,title) VALUES ('book','示例'),('other-book','其他书');
        INSERT INTO book_files (id,book_id,source,external_id,format,visibility,owner_id,content_mode,chapter_count) VALUES
        ('default','book','local','default','txt','private','user','file',1),
        ('selected','book','fixture','hello-content-1','plugin','public','user','chapters',3),
        ('unselected','book','fixture','hello-content-1','plugin','public','user','chapters',3),
        ('different','other-book','fixture','hello-content-1','plugin','public','user','chapters',3);
        INSERT INTO chapters (file_id,idx,title,content) VALUES
        ('default',0,'默认章节','<p>默认版本</p>'),
        ('selected',0,'保存的章节','<p>已保存正文</p>'),('selected',1,'等待下载',NULL),('selected',2,'空章',''),
        ('unselected',1,'不分享的章节',NULL),('different',1,'其他书籍章节',NULL);
        INSERT INTO shares (token,kind,file_id,created_by) VALUES ('share','book','default','user');
        INSERT INTO share_files (token,file_id) VALUES ('share','selected');").await;
    f
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_retries_recover_only_the_selected_missing_body_without_login() {
    let f = shared_fixture().await;
    let read = "/api/shares/share/chapters/1?file_id=selected";
    let retry = "/api/shares/share/chapters/1/retry?file_id=selected";
    for _ in 0..2 {
        let response = request(f.app(), "GET", read, None, "").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response_json(response).await,
            json!({"error":"stored chapter"})
        );
    }
    let (first, second) = tokio::join!(
        request(f.app(), "POST", retry, None, ""),
        request(f.app(), "POST", retry, None, "")
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let repaired = response_json(first).await;
    assert_eq!(repaired, response_json(second).await);
    assert_eq!(repaired["idx"], 1);
    assert!(!repaired["content"].as_str().unwrap().is_empty());
    f.state
        .library
        .plugins()
        .set_enabled("fixture", false)
        .await
        .unwrap();
    for (method, uri) in [("GET", read), ("POST", retry)] {
        let response = request(f.app(), method, uri, None, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, repaired);
    }
    // Cached siblings, valid empty bodies and the default version stay unchanged.
    for (query, idx, content) in [
        ("", 0, "<p>默认版本</p>"),
        ("?file_id=selected", 0, "<p>已保存正文</p>"),
        ("?file_id=selected", 2, ""),
    ] {
        for (method, suffix) in [("GET", ""), ("POST", "/retry")] {
            let response = request(
                f.app(),
                method,
                &format!("/api/shares/share/chapters/{idx}{suffix}{query}"),
                None,
                "",
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await["content"], content);
        }
    }
}

#[tokio::test]
async fn share_retries_enforce_membership_directory_and_token_lifetime() {
    let f = shared_fixture().await;
    // Even a corrupted membership cannot authorize another book's source.
    f.sql("INSERT INTO share_files (token,file_id) VALUES ('share','different');")
        .await;
    for uri in [
        "/api/shares/missing/chapters/0/retry",
        "/api/shares/share/chapters/1/retry?file_id=unselected",
        "/api/shares/share/chapters/1/retry?file_id=different",
        "/api/shares/share/chapters/1/retry?file_id=missing",
        "/api/shares/share/chapters/99/retry?file_id=selected",
    ] {
        assert_eq!(
            request(f.app(), "POST", uri, None, "").await.status(),
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    // A missing row inside chapter_count must not discover a source chapter.
    f.sql("DELETE FROM chapters WHERE file_id='selected' AND idx=1;")
        .await;
    assert_eq!(
        request(
            f.app(),
            "POST",
            "/api/shares/share/chapters/1/retry?file_id=selected",
            None,
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // Token checks precede cache hits too.
    let cached = "/api/shares/share/chapters/0/retry";
    f.sql("UPDATE shares SET expires_at='2000-01-01T00:00:00Z';")
        .await;
    assert_eq!(
        request(f.app(), "POST", cached, None, "").await.status(),
        StatusCode::NOT_FOUND
    );
    f.sql("UPDATE shares SET expires_at=NULL;").await;
    assert_eq!(
        request(f.app(), "POST", cached, None, "").await.status(),
        StatusCode::OK
    );
    f.sql("DELETE FROM shares;").await;
    assert_eq!(
        request(f.app(), "POST", cached, None, "").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn share_retries_reparse_retained_uploads_and_allow_retry_after_failure() {
    let f = Fixture::new().await;
    let text = "第一章 示例\n分享补下载的正文。\n";
    let hash: String = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    f.sql(&format!("INSERT INTO books (id,title) VALUES ('book','示例');
        INSERT INTO book_files (id,book_id,source,external_id,format,visibility,owner_id,content_mode,chapter_count,orig_ext,orig_size,orig_sha256)
        VALUES ('upload','book','local','upload','txt','private','user','file',1,'txt',{},'{hash}');
        INSERT INTO chapters (file_id,idx,title,content) VALUES ('upload',0,'第一章',NULL);
        INSERT INTO shares (token,kind,file_id,created_by) VALUES ('share','book','upload','user');", text.len())).await;
    let retry = "/api/shares/share/chapters/0/retry";
    assert_eq!(
        request(f.app(), "POST", retry, None, "").await.status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let files = f.dir.path().join("files");
    tokio::fs::create_dir_all(&files).await.unwrap();
    tokio::fs::write(files.join("upload.txt"), text)
        .await
        .unwrap();
    // Making an original available must not turn a GET into implicit recovery.
    let read = "/api/shares/share/chapters/0";
    assert_eq!(
        request(f.app(), "GET", read, None, "").await.status(),
        StatusCode::NOT_FOUND
    );
    let response = request(f.app(), "POST", retry, None, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let repaired = response_json(response).await;
    assert!(
        repaired["content"]
            .as_str()
            .unwrap()
            .contains("分享补下载的正文")
    );
    let response = request(f.app(), "GET", read, None, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, repaired);
}
