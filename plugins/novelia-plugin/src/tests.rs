use super::*;

fn settings(translation: u32) -> Settings {
    Settings {
        base: BASE.into(),
        providers: vec![],
        translation,
        username: String::new(),
        password: String::new(),
        auth_base: AUTH_BASE.into(),
    }
}

#[test]
fn book_references_do_not_require_upload_filenames() {
    for raw in [
        "6806f70f2d549f092c9ae862",
        "wenku:6806f70f2d549f092c9ae862",
        " https://n.novelia.cc/wenku/6806f70f2d549f092c9ae862?tab=files#uploads ",
    ] {
        assert_eq!(
            BookReference::parse(raw),
            Some(BookReference::Wenku("6806f70f2d549f092c9ae862".into()))
        );
    }
    for raw in ["web:syosetu:n1", "https://n.novelia.cc/novel/syosetu/n1/"] {
        assert_eq!(
            BookReference::parse(raw),
            Some(BookReference::Web("syosetu".into(), "n1".into()))
        );
    }
    for raw in [
        "",
        "wenku:",
        "https://n.novelia.cc/wenku",
        "https://n.novelia.cc/wenku/a/b",
        "file:///wenku/a",
        "wenku-volume:a:file.epub",
    ] {
        assert_eq!(BookReference::parse(raw), None, "{raw}");
    }
}

#[test]
fn content_choices_preserve_every_upload_and_mark_unavailable_translations() {
    let novel: WenkuNovel = serde_json::from_value(json!({"volumeJp": [
        {"volumeId": "第二卷 100%:原文.epub", "sakura": 1},
        {"volumeId": "第一卷.txt", "gpt": 1},
        {"volumeId": "未译.txt"}
    ]}))
    .unwrap();
    let options = content_options(&settings(0), "series", &novel);
    assert_eq!(options.len(), 3);
    assert_eq!(options[0].title, "第二卷 100%:原文.epub");
    assert_eq!(
        Id::parse(&options[0].id),
        Some(Id::Volume("series".into(), options[0].title.clone()))
    );
    assert!(options[0].available && options[1].available);
    assert!(!options[2].available);
    let gpt = content_options(&settings(1), "series", &novel);
    assert!(!gpt[0].available && gpt[1].available);
    assert!(
        content_options(&settings(4), "series", &novel)
            .iter()
            .all(|item| !item.available)
    );
}

#[test]
fn token_scope_tracks_credentials_and_endpoints_but_not_translation() {
    let original = settings(0);
    let scope = original.auth_scope();
    assert_eq!(scope, settings(4).auth_scope());
    for field in 0..4 {
        let mut changed = settings(0);
        match field {
            0 => changed.base.push_str("/api2"),
            1 => changed.auth_base.push_str("/auth2"),
            2 => changed.username.push(' '),
            _ => changed.password.push(' '),
        }
        assert_ne!(scope, changed.auth_scope());
    }
    let mut first = settings(0);
    first.username = "ab".into();
    first.password = "c".into();
    let mut second = settings(0);
    second.username = "a".into();
    second.password = "bc".into();
    assert_ne!(first.auth_scope(), second.auth_scope());
}

#[test]
fn opaque_ids_roundtrip_and_reject_malformed_components() {
    assert_eq!(
        Id::parse(&volume_id("series", "第一卷 [a+b] 100%:测试.epub")),
        Some(Id::Volume(
            "series".into(),
            "第一卷 [a+b] 100%:测试.epub".into()
        ))
    );
    assert_eq!(
        Id::parse("web:syosetu:n1234ab"),
        Some(Id::Web("syosetu".into(), "n1234ab".into()))
    );
    for bad in [
        "web:a:%",
        "web:a:%GG",
        "web:a:%FF",
        "wenku:",
        "wenku:a",
        "wenku-asin:a:old.epub",
        "wenku-asin:a:b000000001",
        "wenku-asin:a:",
    ] {
        assert_eq!(Id::parse(bad), None, "{bad}");
    }
}

#[test]
fn url_segments_and_repeated_translation_queries_are_encoded() {
    let s = settings(0);
    let volume = WenkuVolume {
        volume_id: "第一卷 [a+b].epub".into(),
        gpt: 1,
        sakura: 2,
        youdao: 0,
    };
    let url = file_url(&s, "123", &volume).unwrap();
    assert!(url.path().contains("%20[a+b].epub"), "{url}");
    let pairs: Vec<_> = url.query_pairs().collect();
    let translations: Vec<_> = pairs
        .iter()
        .filter(|(k, _)| k == "translations")
        .map(|(_, v)| v.as_ref())
        .collect();
    assert_eq!(translations, ["gpt", "sakura"]);
    assert!(
        pairs
            .iter()
            .any(|(k, v)| k == "filename" && v == &volume.volume_id)
    );
    assert!(file_url(&settings(3), "123", &volume).is_none());
    assert!(file_url(&settings(4), "123", &volume).is_none());
}

#[test]
fn pagination_handles_offsets_crossing_pages_and_last_page_totals() {
    let mut calls = vec![];
    let (total, items) = search_pages(95, 12, |page| {
        calls.push(page);
        Some(Page {
            page_number: 2,
            items: if page == 0 {
                (0..100).collect()
            } else {
                (100..107).collect()
            },
        })
    })
    .unwrap();
    assert_eq!(calls, [0, 1]);
    assert_eq!(items, (95..107).collect::<Vec<_>>());
    assert_eq!(total, 107);
    let (total, items) = search_pages(0, 2, |_| {
        Some(Page {
            page_number: 4,
            items: vec![1, 2, 3],
        })
    })
    .unwrap();
    assert_eq!(total, 400);
    assert_eq!(items, [1, 2]);
    let result = search_pages::<u32>(1000, 2, |_| {
        Some(Page {
            page_number: 0,
            items: vec![],
        })
    })
    .unwrap();
    assert_eq!(result, (0, vec![]));
    let mut calls = 0;
    let (_, empty) = search_pages::<u32>(0, 100, |_| {
        calls += 1;
        Some(Page {
            page_number: u64::MAX,
            items: vec![],
        })
    })
    .unwrap();
    assert_eq!(calls, 2);
    assert!(empty.is_empty());
    assert!(
        search_pages::<u32>(95, 12, |p| if p == 0 {
            Some(Page {
                page_number: 2,
                items: (0..100).collect(),
            })
        } else {
            None
        })
        .is_none()
    );
}

#[test]
fn web_metadata_and_toc_ignore_heading_rows() {
    let novel: WebNovel = serde_json::from_value(json!({
        "titleJp": "原书名", "titleZh": "译书名", "authors": [{"name": "作者", "link": "https://example.com"}],
        "introductionJp": "原简介", "introductionZh": "", "keywords": ["奇幻"],
        "toc": [{"titleJp": "第一部", "chapterId": null}, {"titleJp": "序章", "titleZh": "序幕", "chapterId": "1"}, {"titleJp": "空标题", "chapterId": ""}]
    })).unwrap();
    let book = novel.book(web_id("syosetu", "n1"));
    assert_eq!(book.title, "译书名");
    assert_eq!(book.authors, ["作者"]);
    assert_eq!(book.description.as_deref(), Some("原简介"));
    assert_eq!(novel.chapters().count(), 1);
    assert_eq!(
        novel.chapters().next().unwrap().chapter_id.as_deref(),
        Some("1")
    );
}

#[test]
fn translation_selection_ignores_incomplete_arrays_and_keeps_empty_chapters() {
    let mut chapter: WebChapter = serde_json::from_value(json!({
        "titleJp": "章节", "paragraphs": ["原文", ""], "gptParagraphs": ["不完整"],
        "sakuraParagraphs": ["译文", ""], "youdaoParagraphs": ["有道", ""]
    }))
    .unwrap();
    assert_eq!(
        selected_paragraphs(&chapter, &settings(0)).unwrap()[0],
        "译文"
    );
    assert!(selected_paragraphs(&chapter, &settings(1)).is_none());
    assert_eq!(
        selected_paragraphs(&chapter, &settings(4)).unwrap()[0],
        "原文"
    );
    chapter.sakura_paragraphs = None;
    chapter.youdao_paragraphs = None;
    assert_eq!(
        selected_paragraphs(&chapter, &settings(0)).unwrap()[0],
        "原文"
    );
    chapter.paragraphs.clear();
    chapter.gpt_paragraphs = Some(vec![]);
    assert_eq!(
        selected_paragraphs(&chapter, &settings(1)),
        Some([].as_slice())
    );
}

#[test]
fn html_escapes_prose_and_preserves_images_from_original_paragraphs() {
    let original = vec![
        "原文".into(),
        "<图片>https://example.com/a.png".into(),
        "".into(),
    ];
    let selected = vec![
        "<script>&\"\n下一行".into(),
        "译文不得覆盖图片".into(),
        "".into(),
    ];
    let html = paragraphs_html(&original, &selected, |_| Some("/api/images/abc".into()));
    assert!(html.contains("&lt;script&gt;&amp;&quot;<br/>下一行"));
    assert!(html.contains("<figure><img src=\"/api/images/abc\"/></figure>"));
    assert!(html.ends_with("<p></p>"));
}

#[test]
fn wenku_uses_exact_upstream_volume_ids_without_metadata_or_asins() {
    let mut novel: WenkuNovel = serde_json::from_value(json!({"volumeJp": [
        {"volumeId": "第二卷.epub", "gpt": 3}, {"volumeId": "第一卷.txt", "sakura": 2}
    ]}))
    .unwrap();
    let id = volume_id("series", "第一卷.txt");
    assert_eq!(
        Id::parse(&id),
        Some(Id::Volume("series".into(), "第一卷.txt".into()))
    );
    assert!(novel.upload("第一卷.txt").is_some());
    novel.volume_jp.reverse();
    assert!(novel.upload("第一卷.txt").is_some());
    novel.volume_jp[0].volume_id = "重新上传.txt".into();
    assert!(novel.upload("第一卷.txt").is_none());
    assert!(novel.upload("重新上传.txt").is_some());
    // Width/whitespace normalization must not alias distinct upstream filenames.
    assert!(novel.upload("第 一 卷.txt").is_none());
}

#[test]
fn live_wenku_snapshot_addresses_each_upload_by_volume_id() {
    let novel: WenkuNovel = serde_json::from_str(include_str!(
        "../fixtures/wenku-6806f70f2d549f092c9ae862.json"
    ))
    .unwrap();
    assert_eq!(novel.volume_jp.len(), 9);
    for upload in &novel.volume_jp {
        assert_eq!(
            Id::parse(&volume_id("series", &upload.volume_id)),
            Some(Id::Volume("series".into(), upload.volume_id.clone()))
        );
        assert!(novel.upload(&upload.volume_id).is_some());
    }
}

#[test]
fn configuration_preserves_supplied_text_and_defaults_only_when_missing() {
    let defaults = Settings::from_values(&[]);
    assert!(defaults.username.is_empty());
    assert!(defaults.password.is_empty());
    assert_eq!(defaults.auth_base, AUTH_BASE);
    let configured = Settings::from_values(&[
        ConfigValue::Text(BASE.into()),
        ConfigValue::StringList(vec![]),
        ConfigValue::EnumIndex(2),
        ConfigValue::Text("  测试账号  ".into()),
        ConfigValue::Text(" password with spaces ".into()),
    ]);
    assert_eq!(configured.username, "  测试账号  ");
    assert_eq!(configured.password, " password with spaces ");
    assert_eq!(configured.translation, 2);
}

#[test]
fn empty_configured_urls_are_not_replaced_with_defaults() {
    let s = Settings::from_values(&[
        ConfigValue::Text(String::new()),
        ConfigValue::StringList(vec![]),
        ConfigValue::EnumIndex(0),
        ConfigValue::Text(String::new()),
        ConfigValue::Text(String::new()),
        ConfigValue::Text(String::new()),
    ]);
    assert!(s.base.is_empty());
    assert!(s.auth_base.is_empty());
}

#[test]
fn endpoints_use_configured_addresses_without_plugin_policy_checks() {
    let url = endpoint(
        "http://user:secret@example.com",
        &["api", "v1", "auth", "login"],
    )
    .unwrap();
    assert_eq!(
        url.as_str(),
        "http://user:secret@example.com/api/v1/auth/login"
    );
}

#[test]
fn upstream_identifier_values_are_decoded_without_content_validation() {
    for value in ["", ".", "..", "/", "a\\b", "a:b", "\u{0}"] {
        assert_eq!(
            Id::parse(&volume_id("series", value)),
            Some(Id::Volume("series".into(), value.into()))
        );
    }
}
