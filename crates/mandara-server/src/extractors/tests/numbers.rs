use super::*;

#[tokio::test]
async fn volume_writes_require_unsigned_integers_and_reject_auto_number_overflow() {
    let fixture = Fixture::new().await;
    fixture
        .sql(
            "INSERT INTO series (id, title, created_by) VALUES ('series', '系列', 'user');
        INSERT INTO books (id, title, created_by, series_id) VALUES
        ('first', '第一卷', 'user', 'series'), ('second', '第二卷', 'user', NULL);",
        )
        .await;
    let token = fixture.token("user", Role::User);
    for volume in [1_u32, u32::MAX] {
        let response = request(
            fixture.app(),
            "PATCH",
            "/api/books/first",
            Some(&token),
            &json!({"volume_no":volume}).to_string(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["volume_no"], volume);
    }
    for value in ["-1", "4294967296", "1.5"] {
        let response = request(
            fixture.app(),
            "PATCH",
            "/api/books/first",
            Some(&token),
            &format!("{{\"volume_no\":{value}}}"),
        )
        .await;
        assert!(response.status().is_client_error(), "{value}");
        let book = fixture
            .state
            .library
            .get_book("first")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(book.volume_no, u32::MAX);
    }
    let response = request(
        fixture.app(),
        "PATCH",
        "/api/books/second",
        Some(&token),
        r#"{"series_id":"series"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let book = fixture
        .state
        .library
        .get_book("second")
        .await
        .unwrap()
        .unwrap();
    assert!(book.series_id.is_none());
    assert_eq!(book.volume_no, 0);
    assert_eq!(
        fixture
            .state
            .library
            .series_volume_count("series")
            .await
            .unwrap(),
        1
    );
    let user = User {
        id: "user".into(),
        username: "user".into(),
        role: Role::User,
        created_at: Utc::now(),
    };
    let series = fixture.state.library.list_series(&user).await.unwrap();
    assert_eq!(series[0].1, 1);
}
