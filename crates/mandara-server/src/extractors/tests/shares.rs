use super::*;
use diesel::QueryDsl as _;

async fn fixture() -> Fixture {
    let f = Fixture::new().await;
    f.sql("INSERT INTO books (id,title,cover_url,cover,cover_mime) VALUES ('book','示例','https://example.test/cover.png',X'89504E47','image/png'),('other-book','其他书',NULL,NULL,NULL);
        INSERT INTO book_files (id,book_id,source,external_id,format,visibility,owner_id,chapter_count,label) VALUES
        ('a','book','local','a','epub','private','user',1,'精校版'),
        ('b','book','书源乙','b','plugin','public',NULL,1,'在线版'),
        ('hidden','book','local','hidden','txt','private','other',1,''),
        ('foreign-public','book','local','foreign-public','txt','public','other',1,''),
        ('unselected','book','local','unselected','txt','public','user',1,''),
        ('different','other-book','local','different','txt','public','user',1,'');
        INSERT INTO chapters (file_id,idx,title,content) VALUES ('a',0,'插图','<p>版本甲</p>'),('b',0,'序章','<p>版本乙</p>');").await;
    f
}

async fn create(f: &Fixture, body: Value) -> String {
    let response = request(
        f.app(),
        "POST",
        "/api/files/a/shares",
        Some(&f.token("user", Role::User)),
        &body.to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    response_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn selected_versions_are_public_but_unselected_content_never_leaks() {
    let f = fixture().await;
    let image = crate::service::library::store_image(
        &f.state.diesel_db,
        &f.dir.path().join("files"),
        b"illustration-fixture",
        "image/png",
    )
    .await
    .unwrap();
    f.sql(&format!(r#"UPDATE chapters SET content='<p>版本甲</p><img src="/api/images/{image}">' WHERE file_id='a';"#)).await;
    assert_eq!(
        request(f.app(), "GET", &format!("/api/images/{image}"), None, "")
            .await
            .status(),
        StatusCode::OK
    );
    let token = create(&f, json!({"file_ids":["a","b"]})).await;
    let base = format!("/api/shares/{token}");
    let view = response_json(request(f.app(), "GET", &base, None, "").await).await;
    assert_eq!(view["files"].as_array().unwrap().len(), 2);
    assert_eq!(view["files"][0]["id"], "a");
    assert_eq!(view["files"][1]["source"], "书源乙");
    assert_eq!(view["book"]["cover_url"], "https://example.test/cover.png");
    assert_eq!(
        request(f.app(), "GET", "/api/books/book/cover", None, "")
            .await
            .status(),
        StatusCode::OK
    );
    for (query, content) in [("", "版本甲"), ("?file_id=b", "版本乙")] {
        let chapter = response_json(
            request(
                f.app(),
                "GET",
                &format!("{base}/chapters/0{query}"),
                None,
                "",
            )
            .await,
        )
        .await;
        assert!(chapter["content"].as_str().unwrap().contains(content));
    }
    let directory =
        response_json(request(f.app(), "GET", &format!("{base}/book?file_id=b"), None, "").await)
            .await;
    assert_eq!(directory["file"]["id"], "b");
    for id in ["unselected", "hidden", "different", "missing"] {
        for path in ["book", "chapters/0"] {
            assert_eq!(
                request(
                    f.app(),
                    "GET",
                    &format!("{base}/{path}?file_id={id}"),
                    None,
                    ""
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    f.sql("INSERT INTO book_files (id,book_id,source,external_id,format) VALUES ('new-version','book','local','new','txt');").await;
    let view = response_json(request(f.app(), "GET", &base, None, "").await).await;
    assert_eq!(view["files"].as_array().unwrap().len(), 2);
    let revoked = request(
        f.app(),
        "DELETE",
        &base,
        Some(&f.token("user", Role::User)),
        "",
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        request(f.app(), "GET", &format!("{base}/book?file_id=b"), None, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn selection_checks_every_file_and_rejects_invalid_sets_without_creating_links() {
    let f = fixture().await;
    let token = f.token("user", Role::User);
    for (body, status) in [
        (json!({"file_ids":[]}), StatusCode::BAD_REQUEST),
        (json!({"file_ids":["b"]}), StatusCode::BAD_REQUEST),
        (json!({"file_ids":["a","a"]}), StatusCode::BAD_REQUEST),
        (
            json!({"file_ids":["a","different"]}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({"file_ids":["a","hidden"]}), StatusCode::FORBIDDEN),
        (
            json!({"file_ids":["a","foreign-public"]}),
            StatusCode::FORBIDDEN,
        ),
        (json!({"file_ids":["a","missing"]}), StatusCode::NOT_FOUND),
        (
            json!({"kind":"session","file_ids":["a","b"]}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = request(
            f.app(),
            "POST",
            "/api/files/a/shares",
            Some(&token),
            &body.to_string(),
        )
        .await;
        assert_eq!(response.status(), status, "{body}");
    }
    use diesel_async::RunQueryDsl as _;
    assert_eq!(
        crate::schema::shares::table
            .count()
            .get_result::<i64>(&mut f.state.diesel_db.get().await.unwrap())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn legacy_single_file_creation_remains_scoped_and_expiry_guards_every_version() {
    let f = fixture().await;
    let token = create(&f, json!({})).await;
    let base = format!("/api/shares/{token}");
    let view = response_json(request(f.app(), "GET", &base, None, "").await).await;
    assert_eq!(view["files"].as_array().unwrap().len(), 1);
    assert_eq!(
        request(
            f.app(),
            "GET",
            &format!("{base}/chapters/0?file_id=b"),
            None,
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    f.sql("UPDATE shares SET expires_at='2000-01-01T00:00:00Z';")
        .await;
    for suffix in ["", "/book", "/chapters/0"] {
        assert_eq!(
            request(f.app(), "GET", &format!("{base}{suffix}"), None, "")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn content_membership_failure_rolls_back_the_entire_share() {
    let f = fixture().await;
    f.sql("CREATE TRIGGER reject_membership BEFORE INSERT ON share_files WHEN NEW.file_id='b' BEGIN SELECT RAISE(ABORT,'fixture'); END;").await;
    let response = request(
        f.app(),
        "POST",
        "/api/files/a/shares",
        Some(&f.token("user", Role::User)),
        &json!({"file_ids":["a","b"]}).to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    use diesel_async::RunQueryDsl as _;
    let mut conn = f.state.diesel_db.get().await.unwrap();
    assert_eq!(
        crate::schema::shares::table
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        crate::schema::share_files::table
            .count()
            .get_result::<i64>(&mut conn)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn membership_migration_preserves_legacy_links_and_cascades_on_revocation() {
    use diesel::connection::SimpleConnection as _;
    use diesel::{Connection as _, RunQueryDsl as _};
    let mut conn = diesel::sqlite::SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute(
        "PRAGMA foreign_keys=ON;
        CREATE TABLE book_files(id TEXT PRIMARY KEY);
        CREATE TABLE shares(token TEXT PRIMARY KEY,file_id TEXT REFERENCES book_files(id));
        INSERT INTO book_files VALUES ('a');
        INSERT INTO shares VALUES ('legacy','a');",
    )
    .unwrap();
    conn.batch_execute(include_str!("../../../migrations/0026_share_files.sql"))
        .unwrap();
    let selected: Vec<(String, String)> =
        crate::schema::share_files::table.load(&mut conn).unwrap();
    assert_eq!(selected, vec![("legacy".into(), "a".into())]);
    conn.batch_execute("DELETE FROM shares;").unwrap();
    assert!(
        crate::schema::share_files::table
            .load::<(String, String)>(&mut conn)
            .unwrap()
            .is_empty()
    );
}
