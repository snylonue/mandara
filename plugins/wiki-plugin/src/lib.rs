//! Guest side of the `bookshelf:plugin/bookshelf-plugin` world (v2).
//!
//! A *metadata-only* source (design R5 demo): a wiki that knows a book
//! universe but holds no chapter bodies. Every `book-entry` points its
//! `content-source` at the `reader` instance and its `content-id` at the
//! reader's book id, so the host materializes metadata from here and
//! chapters from the reader plugin.
//!
//! Catalog access is lazy: `search` + `lookup` only (no `declare`) — the
//! library stays empty until a user picks a book from the source browser.
//!
//! Build:
//! ```sh
//! ./scripts/build-plugins.sh wiki   # -> plugins-built/wiki.wasm
//! # register:   POST /api/plugins/instances {"id": "wiki", "wasm_file": "wiki.wasm"}
//! ```

wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 2] = ["site-name", "page-size"];

fn values() -> Vec<ConfigValue> {
    bookshelf::plugin::config::configure()
}

fn config_string(key: &str, default: &str) -> String {
    let idx = FIELDS.iter().position(|k| *k == key).unwrap();
    match values().get(idx) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => s.clone(),
        _ => default.to_string(),
    }
}

fn config_number(key: &str, default: u32) -> u32 {
    let idx = FIELDS.iter().position(|k| *k == key).unwrap();
    match values().get(idx) {
        Some(ConfigValue::Number(n)) => n.max(1.0) as u32,
        _ => default,
    }
}

/// One fake "wiki page". `site`/`page_size` shape the metadata, so the
/// demo shows config injection end-to-end.
struct WikiBook {
    id: &'static str,
    title: &'static str,
    author: &'static str,
    description: &'static str,
    reader_id: &'static str,
    chapter_count: usize,
}

const BOOKS: &[WikiBook] = &[
    WikiBook {
        id: "w-1",
        title: "星海拾遗",
        author: "洛离",
        description: "宇宙边缘电台的周播栏目档案。",
        reader_id: "r-1",
        chapter_count: 3,
    },
    WikiBook {
        id: "w-2",
        title: "雾都侦探手记",
        author: "白川",
        description: "终年有雾的城市里，一间小事务所的接案记录。",
        reader_id: "r-2",
        chapter_count: 3,
    },
    WikiBook {
        id: "w-3",
        title: "剑与茶室",
        author: "山岚",
        description: "隐于山道的茶室，白天泡茶，晚上磨剑。",
        reader_id: "r-3",
        chapter_count: 2,
    },
    WikiBook {
        id: "w-4",
        title: "云端咖啡馆",
        author: "苏晚晴",
        description: "开在平流层边缘的咖啡馆。",
        reader_id: "r-4",
        chapter_count: 2,
    },
    WikiBook {
        id: "w-5",
        title: "旧书店的猫",
        author: "林默",
        description: "每本旧书里都有一枚猫爪印。",
        reader_id: "r-5",
        chapter_count: 3,
    },
    WikiBook {
        id: "w-6",
        title: "时间旅人的信",
        author: "迟舟",
        description: "寄信人来自明天。",
        reader_id: "r-6",
        chapter_count: 3,
    },
    WikiBook {
        id: "w-7",
        title: "深海广播",
        author: "韩潮",
        description: "马里亚纳海沟下的电台信号。",
        reader_id: "r-7",
        chapter_count: 2,
    },
    WikiBook {
        id: "w-8",
        title: "第七封印物语",
        author: "陆离",
        description: "第七道封印之后，世界安静得不像话。",
        reader_id: "r-8",
        chapter_count: 3,
    },
];

fn entry(book: &WikiBook, site: &str) -> BookEntry {
    BookEntry {
        id: book.id.into(),
        title: book.title.into(),
        authors: vec![book.author.into()],
        description: Some(format!(
            "{}（词条由 {site} 提供，共 {} 章）",
            book.description, book.chapter_count
        )),
        cover_url: None,
        // Content comes from the `reader` plugin instance (R5).
        content_source: Some("reader".into()),
        content_id: Some(book.reader_id.into()),
    }
}

struct WikiPlugin;

impl Guest for WikiPlugin {
    fn name() -> String {
        "wiki".into()
    }

    fn config_schema() -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: Some("维基书源".into()),
                required: true,
                hint: Some("显示在词条描述里".into()),
            },
            ConfigField {
                key: "page-size".into(),
                label: "词条数量".into(),
                kind: bookshelf::plugin::config::ConfigKind::Number,
                default: Some("8".into()),
                required: false,
                hint: Some("模拟上游分页取回的词条数（1~8）".into()),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        // Metadata only: browse + fetch by id; no catalog dump, no
        // content, no upload identification.
        vec!["search".into(), "lookup".into()]
    }

    fn declare() -> Option<Vec<DeclaredBook>> {
        None // large source: never enumerated eagerly
    }

    fn search_books(query: String, offset: u32, limit: u32) -> SearchResult {
        let site = config_string("site-name", "维基书源");
        let page_size = config_number("page-size", 8).min(BOOKS.len() as u32);
        let q = query.trim().to_lowercase();
        let all: Vec<BookEntry> = BOOKS
            .iter()
            .take(page_size as usize)
            .filter(|b| {
                q.is_empty()
                    || b.title.to_lowercase().contains(&q)
                    || b.author.to_lowercase().contains(&q)
            })
            .map(|b| entry(b, &site))
            .collect();
        let total = all.len() as u64;
        let items = all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        SearchResult { total, items }
    }

    fn get_book(book_id: String) -> Option<BookEntry> {
        let site = config_string("site-name", "维基书源");
        BOOKS
            .iter()
            .find(|b| b.id == book_id)
            .map(|b| entry(b, &site))
    }

    fn chapter_titles(_book_id: String) -> Vec<String> {
        Vec::new() // no `content` capability; host never calls this
    }

    fn get_chapter(_book_id: String, _index: u32) -> Option<Chapter> {
        None // no `content` capability; host never calls this
    }

    fn identify_upload(_filename: String, _file_hash: String) -> Option<BookEntry> {
        None // no `identify` capability; host never calls this
    }

    fn get_book_file(_book_id: String) -> Option<BookFile> {
        None // no `book-file` capability; host never calls this
    }
}

export!(WikiPlugin);
