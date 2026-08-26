//! Guest side of the `bookshelf:plugin/bookshelf-plugin` world (v2).
//!
//! Build:
//! ```sh
//! cargo build -p hello-plugin --release --target wasm32-unknown-unknown
//! wasm-tools component new \
//!     target/wasm32-unknown-unknown/release/hello_plugin.wasm \
//!     -o plugins-built/hello.wasm
//! # copy plugins-built/hello.wasm to the server plugins dir (default: data/plugins)
//! ```
//!
//! The generated core module embeds a `component-type` custom section, so
//! `wasm-tools component new` can lift it into a component without adapters
//! (this world does not import any wasi interfaces).
//!
//! This demo plugin is *data-driven* (design R1): its catalog is static
//! data returned by `declare`, its behavior is configured through
//! `config-schema` values injected by the host on every call (R3), and it
//! stays stateless (R2) — there is no mutable state anywhere.

wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

// `BookEntry`, `Chapter`, `DeclaredBook`, `SearchResult`, `ConfigField`,
// `ConfigValue` and the `Guest` trait are generated in this scope by
// generate!() (the world `use`s the types and config interfaces).

/// Configuration schema field order — the host injects values in this
/// order, so the guest maps indices back to keys.
const FIELDS: [&str; 5] = ["site-name", "mode", "verbose", "spin", "tags"];

/// Pull the host-injected configuration for this call.
fn values() -> Vec<ConfigValue> {
    bookshelf::plugin::config::configure()
}

fn field_value(key: &str) -> Option<ConfigValue> {
    let idx = FIELDS.iter().position(|k| *k == key)?;
    values().get(idx).cloned()
}

fn config_string(key: &str, default: &str) -> String {
    match field_value(key) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => s,
        _ => default.to_string(),
    }
}

fn config_bool(key: &str, default: bool) -> bool {
    match field_value(key) {
        Some(ConfigValue::Boolean(b)) => b,
        _ => default,
    }
}

fn config_enum_index(key: &str) -> u32 {
    match field_value(key) {
        Some(ConfigValue::EnumIndex(i)) => i,
        _ => 0,
    }
}

fn config_tags() -> Vec<String> {
    match field_value("tags") {
        Some(ConfigValue::StringList(tags)) => tags,
        _ => Vec::new(),
    }
}

/// The plugin implementation. `Guest` is the generated trait for the
/// world-level exports.
struct HelloPlugin;

impl Guest for HelloPlugin {
    fn name() -> String {
        "hello".to_string()
    }

    fn source_info() -> SourceInfo {
        // A small declared catalog, fully synced at startup; nothing to
        // pick in a source browser.
        SourceInfo {
            kind: "browse".into(),
            id_kind: "free".into(),
            id_hint: None,
            search_hint: None,
        }
    }

    /// Schema of per-instance configuration. The host renders this schema
    /// as an admin form and injects the values at the start of every call.
    fn config_schema() -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: Some("Bookshelf 示例库".into()),
                required: true,
                hint: Some("出现在推荐书目和识别提示里".into()),
            },
            ConfigField {
                key: "mode".into(),
                label: "内容版本".into(),
                kind: bookshelf::plugin::config::ConfigKind::EnumOptions(vec![
                    "default".into(),
                    "alt".into(),
                ]),
                default: Some("0".into()),
                required: false,
                hint: Some("章节正文的变体".into()),
            },
            ConfigField {
                key: "verbose".into(),
                label: "详细日志".into(),
                kind: bookshelf::plugin::config::ConfigKind::Boolean,
                default: Some("false".into()),
                required: false,
                hint: Some("每次取章时写一条宿主日志".into()),
            },
            ConfigField {
                key: "spin".into(),
                label: "死循环测试(不要开启)".into(),
                kind: bookshelf::plugin::config::ConfigKind::Boolean,
                default: Some("false".into()),
                required: false,
                hint: Some("开启后取章会陷入死循环，用于演示宿主超时保护".into()),
            },
            ConfigField {
                key: "tags".into(),
                label: "额外标签".into(),
                kind: bookshelf::plugin::config::ConfigKind::ListOfString,
                default: Some("[]".into()),
                required: false,
                hint: Some("逗号分隔，会附在书目描述中".into()),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        // declare: small static catalog synced eagerly; identify: upload
        // recognition; content: chapters + titles.
        vec!["declare".into(), "identify".into(), "content".into()]
    }

    /// Static catalog. Bodies are inline (non-empty `content`), so the
    /// host materializes them eagerly.
    fn declare() -> Option<Vec<DeclaredBook>> {
        let site = config_string("site-name", "Bookshelf 示例库");
        let tags = config_tags();
        let tail = if tags.is_empty() {
            String::new()
        } else {
            format!("（{}）", tags.join("、"))
        };
        let book1 = BookEntry {
            id: "hello-1".into(),
            title: format!("Hello Bookshelf（{site}）"),
            authors: vec!["Bookshelf Team".into()],
            description: Some(format!(
                "这是一本由 wasm 插件依据配置生成的示例书。当前配置：站点={site}，版本=default。{tail}"
            )),
            cover_url: None,
            extra: None,
            content_source: None,
            content_id: None,
            volumes: None,
        };
        let book2 = BookEntry {
            id: "hello-2".into(),
            title: "Hello Bookshelf·幕后".into(),
            authors: vec!["Bookshelf Team".into()],
            description: Some("示例插件内部机制的说明。".into()),
            cover_url: None,
            // Extended-metadata demo: known keys validate against BookExt,
            // unknown keys round-trip verbatim.
            extra: Some(
                r#"{"publisher":"Bookshelf Press","pub-date":"2026-08","pages":42,"future_key":{"nested":true}}"#.into(),
            ),
            content_source: None,
            content_id: None,
            volumes: None,
        };
        Some(vec![
            DeclaredBook {
                book: book1,
                chapters: vec![
                    Chapter {
                        title: "第一章 你好，书架".into(),
                        content: format!(
                            "你成功加载了一个 wasm 插件！\n\n当前站点名称由配置注入：{site}。{tail}\n\n这本书由 Rust 编写、以 WebAssembly 组件的形式运行在服务器上。\n\n插件在沙箱中运行，只能调用宿主提供的 log 与 configure 接口。"
                        ),
                    },
                    Chapter {
                        title: "第二章 世界很大".into(),
                        content: "你可以用同样的方式写出自己的书籍来源插件：实现 capabilities、declare（或 search-books/get-book）、chapter-titles 和 get-chapter 即可。\n\n参考 docs/plugins.md。".into(),
                    },
                ],
            },
            DeclaredBook {
                book: book2,
                chapters: vec![Chapter {
                    title: "幕后".into(),
                    content: "这个插件的目录是声明式的数据，行为由配置驱动：\n\n- site-name 改变书名与正文；\n- mode 切换正文变体；\n- verbose 让插件记录宿主日志；\n- tags 附加到描述中。\n\n每次调用宿主都注入配置，插件内部没有任何跨调用状态。".into(),
                }],
            },
        ])
    }

    fn search_books(query: String, _offset: u32, _limit: u32) -> SearchResult {
        // No `search` capability: the host never calls this. Stub.
        let _ = query;
        SearchResult {
            total: 0,
            items: Vec::new(),
        }
    }

    fn get_book(_book_id: String) -> Option<BookEntry> {
        // No `lookup` capability: the host never calls this. Stub.
        None
    }

    fn chapter_titles(book_id: String) -> Vec<String> {
        match book_id.as_str() {
            "hello-1" => vec!["第一章 你好，书架".into(), "第二章 世界很大".into()],
            "hello-2" => vec!["幕后".into()],
            _ => Vec::new(),
        }
    }

    fn get_chapter(book_id: String, index: u32) -> Option<Chapter> {
        if config_bool("spin", false) {
            // Deliberate infinite loop: the host's epoch deadline must
            // trap this call ("Plugin timeout").
            let mut x: u64 = 0;
            loop {
                x = x.wrapping_add(1);
                if x == 0 {
                    x = 1;
                }
            }
        }
        if config_bool("verbose", false) {
            bookshelf::plugin::store::log(&format!("get_chapter({book_id}, {index})"));
        }
        let mode = config_enum_index("mode");
        let alt = mode == 1;
        match (book_id.as_str(), index, alt) {
            ("hello-1", 0, false) => Some(Chapter {
                title: "第一章 你好，书架".into(),
                content: "你成功加载了一个 wasm 插件！\n\n这本书由 Rust 编写、以 WebAssembly 组件的形式运行在服务器上。\n\n插件在沙箱中运行，只能调用宿主提供的 log 接口。".into(),
            }),
            ("hello-1", 0, true) => Some(Chapter {
                title: "第一章 你好，书架（变体）".into(),
                content: "配置里选择了 alt 版本：这一章的正文与 default 版本不同。\n\n你成功加载了一个 wasm 插件，并且配置注入生效了！".into(),
            }),
            ("hello-1", 1, false) => Some(Chapter {
                title: "第二章 世界很大".into(),
                content: "你可以用同样的方式写出自己的书籍来源插件：实现 capabilities、declare（或 search-books/get-book）、chapter-titles 和 get-chapter 即可。\n\n参考 docs/plugins.md。".into(),
            }),
            ("hello-1", 1, true) => Some(Chapter {
                title: "第二章 世界很大（变体）".into(),
                content: "（alt 变体）不同配置实例可以指向同一个 wasm 文件：实例 id 是源 id，配置归属实例。".into(),
            }),
            ("hello-2", 0, _) => Some(Chapter {
                title: "幕后".into(),
                content: "这个插件的目录是声明式的数据，行为由配置驱动。\n\n每次调用宿主都注入配置，插件内部没有任何跨调用状态。".into(),
            }),
            _ => None,
        }
    }

    /// Example book identification: any uploaded file whose name contains
    /// "hello" is recognized as this plugin's sample book, so the server
    /// can take the metadata from here instead of parsing the file.
    fn identify_upload(filename: String, _file_hash: String) -> Option<BookEntry> {
        let lower = filename.to_lowercase();
        if lower.contains("hello") || lower.contains("示例") {
            let site = config_string("site-name", "Bookshelf 示例库");
            Some(BookEntry {
                id: "hello-1".into(),
                title: format!("Hello Bookshelf（{site}）"),
                authors: vec!["Bookshelf Team".into()],
                description: Some("这是一本由 wasm 插件提供的示例书。".into()),
                cover_url: None,
                extra: None,
                content_source: None,
                content_id: None,
                volumes: None,
            })
        } else {
            None
        }
    }

    fn get_book_file(_book_id: String) -> Option<BookFile> {
        // No `book-file` capability: the host never calls this. Stub.
        None
    }
}

export!(HelloPlugin);
