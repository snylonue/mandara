use super::*;

async fn json_request(f: &Fixture, method: &str, uri: &str, token: &str, body: Value) -> Value {
    let response = request(f.app(), method, uri, Some(token), &body.to_string()).await;
    assert!(
        response.status().is_success(),
        "{method} {uri}: {}",
        response.status()
    );
    response_json(response).await
}

#[tokio::test]
async fn global_sessions_keep_file_progress_independent_and_sync_exactly_one_file() {
    let f = Fixture::new().await;
    f.sql("INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, visibility, owner_id, chapter_count)
        VALUES ('first', 'book', 'local', 'first', 'txt', 'public', 'user', 10),
               ('second', 'book', 'local', 'second', 'txt', 'public', 'user', 10);").await;
    let token = f.token("user", Role::User);
    let contexts = json_request(&f, "GET", "/api/sessions", &token, json!(null)).await;
    assert_eq!(contexts.as_array().unwrap().len(), 1);
    assert!(contexts[0].get("is_default").is_none());
    let default = contexts[0]["id"].as_str().unwrap();
    let created = json_request(
        &f,
        "POST",
        "/api/sessions",
        &token,
        json!({"label":" 手机 "}),
    )
    .await;
    let phone = created["id"].as_str().unwrap();
    assert_eq!(created["label"], "手机");
    let reused = json_request(&f, "POST", "/api/sessions", &token, json!({"label":"手机"})).await;
    assert_eq!(reused["id"], phone);
    assert_eq!(
        json_request(
            &f,
            "GET",
            &format!("/api/sessions/{phone}/progress"),
            &token,
            json!(null)
        )
        .await,
        json!([])
    );
    let first = format!("/api/sessions/{default}/files/first/progress");
    let second = format!("/api/sessions/{default}/files/second/progress");
    json_request(
        &f,
        "PUT",
        &first,
        &token,
        json!({"chapter_idx":8,"in_chapter":{"fraction":0.9}}),
    )
    .await;
    let other_book = json_request(
        &f,
        "PUT",
        &second,
        &token,
        json!({"chapter_idx":4,"in_chapter":{"fraction":0.3}}),
    )
    .await;
    let source = json_request(
        &f,
        "PUT",
        &format!("/api/sessions/{phone}/files/first/progress"),
        &token,
        json!({"chapter_idx":2,"in_chapter":{"offset":120}}),
    )
    .await;
    // Partial writes and synchronization preserve the character-offset channel.
    let partial = json_request(
        &f,
        "PUT",
        &format!("/api/sessions/{phone}/files/first/progress"),
        &token,
        json!({"chapter_idx":3}),
    )
    .await;
    assert_eq!(partial["position"]["in_chapter"], json!({"offset":120}));
    let synced = json_request(
        &f,
        "POST",
        &format!("/api/sessions/{default}/files/first/sync"),
        &token,
        json!({"source_session_id":phone}),
    )
    .await;
    assert_eq!(synced["position"], partial["position"]);
    assert_eq!(synced["session_id"], default);
    assert_ne!(synced["id"], source["id"]);
    let all = json_request(
        &f,
        "GET",
        &format!("/api/sessions/{default}/progress"),
        &token,
        json!(null),
    )
    .await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(
        all.as_array()
            .unwrap()
            .iter()
            .find(|p| p["file_id"] == "second")
            .unwrap(),
        &other_book
    );
    let source_after = json_request(
        &f,
        "GET",
        &format!("/api/sessions/{phone}/progress"),
        &token,
        json!(null),
    )
    .await;
    assert_eq!(source_after, json!([partial]));
    // Rewriting a position keeps public progress links attached to a stable id.
    let share = json_request(
        &f,
        "POST",
        "/api/files/first/shares",
        &token,
        json!({"kind":"session", "session_id":synced["id"]}),
    )
    .await;
    let share_uri = format!("/api/shares/{}", share["token"].as_str().unwrap());
    let offset_view = response_json(request(f.app(), "GET", &share_uri, None, "").await).await;
    assert_eq!(
        offset_view["session"]["position"]["in_chapter"],
        json!({"offset":120})
    );
    let zero_offset = json_request(
        &f,
        "PUT",
        &first,
        &token,
        json!({"in_chapter":{"offset":0}}),
    )
    .await;
    assert_eq!(zero_offset["position"]["in_chapter"], json!({"offset":0}));
    json_request(
        &f,
        "PUT",
        &first,
        &token,
        json!({"chapter_idx":99,"in_chapter":{"fraction":2}}),
    )
    .await;
    let view = response_json(request(f.app(), "GET", &share_uri, None, "").await).await;
    assert_eq!(
        view["session"]["position"],
        json!({"chapter_idx":9,"in_chapter":{"fraction":1.0}})
    );
    let response = request(
        f.app(),
        "DELETE",
        &format!("/api/sessions/{phone}"),
        Some(&token),
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        json_request(&f, "GET", "/api/files/first/sessions", &token, json!(null)).await["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn sessions_enforce_ownership_and_visibility() {
    let f = Fixture::new().await;
    f.sql(
        "INSERT INTO books (id, title) VALUES ('book', '示例');
        INSERT INTO book_files (id, book_id, source, external_id, format, owner_id, chapter_count)
        VALUES ('private', 'book', 'local', 'private', 'txt', 'other', 2);",
    )
    .await;
    let token = f.token("user", Role::User);
    let other = f.token("other", Role::User);
    let contexts = json_request(&f, "GET", "/api/sessions", &token, json!(null)).await;
    let id = contexts[0]["id"].as_str().unwrap();
    for (method, uri, auth, body, status) in [
        (
            "DELETE",
            format!("/api/sessions/{id}"),
            &other,
            json!({}),
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            format!("/api/sessions/{id}/progress"),
            &other,
            json!({}),
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            format!("/api/sessions/{id}/files/private/progress"),
            &other,
            json!({}),
            StatusCode::FORBIDDEN,
        ),
        (
            "PUT",
            format!("/api/sessions/{id}/files/private/progress"),
            &token,
            json!({}),
            StatusCode::FORBIDDEN,
        ),
        (
            "POST",
            "/api/sessions".into(),
            &token,
            json!({"label":" "}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/api/sessions".into(),
            &token,
            json!({"label":"长".repeat(65)}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            request(f.app(), method, &uri, Some(auth), &body.to_string())
                .await
                .status(),
            status
        );
    }
    f.sql("UPDATE book_files SET visibility='public' WHERE id='private';")
        .await;
    let source_context = json_request(&f, "GET", "/api/sessions", &other, json!(null)).await;
    let source_id = source_context[0]["id"].as_str().unwrap();
    let empty = json_request(
        &f,
        "POST",
        "/api/sessions",
        &token,
        json!({"label":"空档案"}),
    )
    .await;
    let sync_uri = format!("/api/sessions/{id}/files/private/sync");
    for (source, status) in [
        (id, StatusCode::BAD_REQUEST),
        (source_id, StatusCode::FORBIDDEN),
        (empty["id"].as_str().unwrap(), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            request(
                f.app(),
                "POST",
                &sync_uri,
                Some(&token),
                &json!({"source_session_id":source}).to_string()
            )
            .await
            .status(),
            status
        );
    }
    assert_eq!(
        json_request(
            &f,
            "GET",
            &format!("/api/sessions/{id}/progress"),
            &token,
            json!(null)
        )
        .await,
        json!([])
    );
}

#[tokio::test]
async fn account_creation_creates_one_ordinary_session_without_recreating_deleted_sessions() {
    let f = Fixture::new().await;
    let response = request(
        f.app(),
        "POST",
        "/api/auth/register",
        None,
        &json!({"username":"new-reader", "password":"reading-password"}).to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let auth = response_json(response).await;
    let token = auth["token"].as_str().unwrap();
    let sessions = json_request(&f, "GET", "/api/sessions", token, json!(null)).await;
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["user_id"], auth["user"]["id"]);
    assert!(sessions[0].get("is_default").is_none());
    let uri = format!("/api/sessions/{}", sessions[0]["id"].as_str().unwrap());
    assert_eq!(
        request(f.app(), "DELETE", &uri, Some(token), "")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    for _ in 0..2 {
        assert_eq!(
            json_request(&f, "GET", "/api/sessions", token, json!(null)).await,
            json!([])
        );
    }
    let response = request(
        f.app(),
        "POST",
        "/api/auth/login",
        None,
        &json!({"username":"new-reader", "password":"reading-password"}).to_string(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_request(&f, "GET", "/api/sessions", token, json!(null)).await,
        json!([])
    );
    let created = json_request(&f, "POST", "/api/sessions", token, json!({"label":"重读"})).await;
    let remaining = json_request(&f, "GET", "/api/sessions", token, json!(null)).await;
    assert_eq!(remaining, json!([created]));
}

#[tokio::test]
async fn migration_groups_legacy_labels_without_losing_progress_or_shares() {
    use diesel::connection::SimpleConnection as _;
    use diesel::prelude::*;
    let mut conn = diesel::sqlite::SqliteConnection::establish(":memory:").unwrap();
    conn.batch_execute(include_str!("../../../migrations/0001_init.sql"))
        .unwrap();
    conn.batch_execute("PRAGMA foreign_keys=ON;
        INSERT INTO users (id,username,password_hash) VALUES ('u','u',''), ('v','v','');
        INSERT INTO books (id,title) VALUES ('b','示例');
        INSERT INTO book_files (id,book_id,source,external_id) VALUES ('a','b','local','a'),('b','b','local','b');
        INSERT INTO sessions (id,user_id,file_id,label,chapter_idx,fraction) VALUES ('p1','u','a','phone',3,0.5),('p2','u','b','phone',7,0.2),('p3','u','b','default',1,0.1);
        INSERT INTO shares (token,kind,file_id,session_id) VALUES ('link','session','a','p1');").unwrap();
    conn.batch_execute(include_str!(
        "../../../migrations/0021_reading_sessions.sql"
    ))
    .unwrap();
    conn.batch_execute(include_str!(
        "../../../migrations/0022_ordinary_reading_sessions.sql"
    ))
    .unwrap();
    assert!(
        conn.batch_execute("SELECT is_default FROM reading_sessions;")
            .is_err()
    );
    let contexts: Vec<(String, String)> = crate::schema::reading_sessions::table
        .select((
            crate::schema::reading_sessions::id,
            crate::schema::reading_sessions::user_id,
        ))
        .load(&mut conn)
        .unwrap();
    assert_eq!(contexts.len(), 3);
    let progress: Vec<(String, Option<String>, i64, f64)> = crate::schema::sessions::table
        .select((
            crate::schema::sessions::id,
            crate::schema::sessions::reading_session_id,
            crate::schema::sessions::chapter_idx,
            crate::schema::sessions::fraction,
        ))
        .order(crate::schema::sessions::id)
        .load(&mut conn)
        .unwrap();
    assert_eq!(progress[0].1, progress[1].1);
    assert_ne!(progress[1].1, progress[2].1);
    assert_eq!((progress[0].2, progress[0].3), (3, 0.5));
    assert_eq!((progress[1].2, progress[1].3), (7, 0.2));
    let share: Option<String> = crate::schema::shares::table
        .select(crate::schema::shares::session_id)
        .first(&mut conn)
        .unwrap();
    assert_eq!(share.as_deref(), Some("p1"));
    conn.batch_execute("INSERT INTO users (id,username,password_hash) VALUES ('new','new','');")
        .unwrap();
    assert_eq!(
        crate::schema::reading_sessions::table
            .filter(crate::schema::reading_sessions::user_id.eq("new"))
            .count()
            .get_result::<i64>(&mut conn)
            .unwrap(),
        1
    );
}
