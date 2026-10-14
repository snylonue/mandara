use super::*;

fn settings(translation: u32) -> Settings {
    Settings {
        base: BASE.into(),
        wenku: false,
        providers: vec![],
        translation,
        token: String::new(),
    }
}

#[test]
fn opaque_ids_roundtrip_and_reject_malformed_components() {
    let filename = "第一卷 [a+b] 100%:测试.epub";
    assert_eq!(
        Id::parse(&volume_id("series", filename)),
        Some(Id::Volume("series".into(), filename.into()))
    );
    assert_eq!(
        Id::parse("web:syosetu:n1234ab"),
        Some(Id::Web("syosetu".into(), "n1234ab".into()))
    );
    for bad in [
        "web:a:",
        "web:a:%",
        "web:a:%GG",
        "web:a:%FF",
        "web:a:%00",
        "web:a:..",
        "wenku:",
        "wenku-volume:a:b:c",
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
    for base in ["file:///tmp", "https://user:secret@example.com"] {
        let mut s = settings(0);
        s.base = base.into();
        assert!(s.url(&["api", "novel"]).is_none());
    }
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
fn wenku_file_identity_survives_reordering_without_inventing_asin_matches() {
    let mut novel: WenkuNovel = serde_json::from_value(json!({
        "title": "原系列", "titleZh": "译系列", "cover": null, "authors": ["作者"], "artists": ["画师"],
        "introduction": "简介", "volumes": [{"asin": "unrelated", "title": "不应猜测"}], "volumeJp": [
            {"volumeId": "第二卷.epub", "gpt": 3}, {"volumeId": "第一卷.txt", "sakura": 2}
        ]
    })).unwrap();
    let first = novel.books("id").remove(0);
    assert_eq!(first.title, "第二卷");
    assert_eq!(first.series.unwrap().id, "wenku:id");
    novel.volume_jp.reverse();
    assert_eq!(novel.books("id")[1].id, first.id);
}
