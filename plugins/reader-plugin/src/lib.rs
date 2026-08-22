//! Guest side of the `bookshelf:plugin/bookshelf-plugin` world (v2).
//!
//! A *content* source (design R5 demo): a reading site that knows the same
//! book universe as the wiki plugin, but under its own ids (`r-N`) and
//! with the actual chapter bodies. The demo flow:
//!
//! 1. the wiki metadata entry points `content-source: "reader"`,
//!    `content-id: "r-N"`,
//! 2. the host asks this instance for titles/chapters of `r-N`,
//! 3. a user can also rebind any plugin file to a book found here (the
//!    content-source picker searches this instance).
//!
//! Build:
//! ```sh
//! ./scripts/build-plugins.sh reader   # -> plugins-built/reader.wasm
//! # register:   POST /api/plugins/instances {"id": "reader", "wasm_file": "reader.wasm"}
//! ```

wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 2] = ["site-name", "max-chapters"];

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
        Some(ConfigValue::Number(n)) => n.clamp(1.0, 10.0) as u32,
        _ => default,
    }
}

struct ReaderBook {
    id: &'static str,
    title: &'static str,
    author: &'static str,
    description: &'static str,
    chapters: &'static [&'static str],
}

const BOOKS: &[ReaderBook] = &[
    ReaderBook {
        id: "r-1",
        title: "星海拾遗",
        author: "洛离",
        description: "宇宙边缘电台的周播栏目档案。",
        chapters: &["第一章 回声信标", "第二章 潮汐图书馆", "第三章 末班星舟"],
    },
    ReaderBook {
        id: "r-2",
        title: "雾都侦探手记",
        author: "白川",
        description: "终年有雾的城市里，一间小事务所的接案记录。",
        chapters: &["第一章 雾中来信", "第二章 九号站台", "第三章 钟楼谜题"],
    },
    ReaderBook {
        id: "r-3",
        title: "剑与茶室",
        author: "山岚",
        description: "隐于山道的茶室，白天泡茶，晚上磨剑。",
        chapters: &["第一章 刀鞘与茶匙", "第二章 双刀店主"],
    },
    ReaderBook {
        id: "r-4",
        title: "云端咖啡馆",
        author: "苏晚晴",
        description: "开在平流层边缘的咖啡馆。",
        chapters: &["第一章 海拔三千米的拿铁", "第二章 云上菜单"],
    },
    ReaderBook {
        id: "r-5",
        title: "旧书店的猫",
        author: "林默",
        description: "每本旧书里都有一枚猫爪印。",
        chapters: &[
            "第一章 扉页爪印",
            "第二章 借阅卡背面的名字",
            "第三章 打烊后的书梯",
        ],
    },
    ReaderBook {
        id: "r-6",
        title: "时间旅人的信",
        author: "迟舟",
        description: "寄信人来自明天。",
        chapters: &[
            "第一章 明天寄来的明信片",
            "第二章 错序的邮票",
            "第三章 末班邮箱",
        ],
    },
    ReaderBook {
        id: "r-7",
        title: "深海广播",
        author: "韩潮",
        description: "马里亚纳海沟下的电台信号。",
        chapters: &["第一章 波长 31.4", "第二章 鲸歌应答"],
    },
    ReaderBook {
        id: "r-8",
        title: "第七封印物语",
        author: "陆离",
        description: "第七道封印之后，世界安静得不像话。",
        chapters: &[
            "第一章 石门的刻痕",
            "第二章 无名的守印人",
            "第三章 封印之下",
        ],
    },
];

fn entry(book: &ReaderBook, site: &str) -> BookEntry {
    BookEntry {
        id: book.id.into(),
        title: book.title.into(),
        authors: vec![book.author.into()],
        description: Some(format!("{}（阅读源：{site}）", book.description)),
        cover_url: None,
        content_source: None, // this instance provides the content itself
        content_id: None,
    }
}

fn chapter_text(title: &str, site: &str) -> String {
    format!(
        "（正文由阅读源「{site}」提供）\n\n{title}。\n\n这一章的内容是模板生成的示例正文：\n\n窗外雨声不断，桌上的灯把书页照得发暖。故事从这里继续……\n\n—— 阅读源 {site} 模拟正文"
    )
}

struct ReaderPlugin;

impl Guest for ReaderPlugin {
    fn name() -> String {
        "reader".into()
    }

    fn config_schema() -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: Some("阅读源".into()),
                required: true,
                hint: Some("出现在正文与描述里".into()),
            },
            ConfigField {
                key: "max-chapters".into(),
                label: "每章上限".into(),
                kind: bookshelf::plugin::config::ConfigKind::Number,
                default: Some("3".into()),
                required: false,
                hint: Some("每本书最多提供的章节数（1~10）".into()),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        vec!["search".into(), "lookup".into(), "content".into()]
    }

    fn declare() -> Option<Vec<DeclaredBook>> {
        None // large source: never enumerated eagerly
    }

    fn search_books(query: String, offset: u32, limit: u32) -> SearchResult {
        let site = config_string("site-name", "阅读源");
        let q = query.trim().to_lowercase();
        let all: Vec<BookEntry> = BOOKS
            .iter()
            .filter(|b| {
                q.is_empty()
                    || b.title.to_lowercase().contains(&q)
                    || b.author.to_lowercase().contains(&q)
                    || b.id.to_lowercase().contains(&q)
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
        let site = config_string("site-name", "阅读源");
        BOOKS
            .iter()
            .find(|b| b.id == book_id)
            .map(|b| entry(b, &site))
    }

    fn chapter_titles(book_id: String) -> Vec<String> {
        let max = config_number("max-chapters", 3);
        BOOKS
            .iter()
            .find(|b| b.id == book_id)
            .map(|b| {
                b.chapters
                    .iter()
                    .take(max as usize)
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn get_chapter(book_id: String, index: u32) -> Option<Chapter> {
        let site = config_string("site-name", "阅读源");
        let max = config_number("max-chapters", 3);
        let book = BOOKS.iter().find(|b| b.id == book_id)?;
        if (index as usize) >= book.chapters.len().min(max as usize) {
            return None;
        }
        let title = book.chapters[index as usize];
        Some(Chapter {
            title: title.to_string(),
            content: chapter_text(title, &site),
        })
    }

    fn identify_upload(_filename: String, _file_hash: String) -> Option<BookEntry> {
        None // no `identify` capability; host never calls this
    }

    fn get_book_file(_book_id: String) -> Option<BookFile> {
        None // no `book-file` capability; host never calls this
    }
}

export!(ReaderPlugin);
