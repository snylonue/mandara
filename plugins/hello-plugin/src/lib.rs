//! Guest side of the `mandara:plugin/mandara-plugin` world (v2).
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
//! keeps no cross-call guest memory (R2). An undeclared state API probe
//! demonstrates explicit persistence through the host for integration tests.

wit_bindgen::generate!({
    world: "mandara-plugin",
    path: "../../crates/mandara-plugin/wit",
});

// `BookEntry`, `Chapter`, `DeclaredBook`, `SearchResult`, `ConfigField`,
// `ConfigValue` and the `Guest` trait are generated in this scope by
// generate!() (the world `use`s the types interface).

/// Configuration schema field order — the host injects values in this
/// order, so the guest maps indices back to keys.
const FIELDS: [&str; 5] = ["site-name", "mode", "verbose", "spin", "tags"];

fn field_value(config: &[ConfigValue], key: &str) -> Option<ConfigValue> {
    let idx = FIELDS.iter().position(|k| *k == key)?;
    config.get(idx).cloned()
}

fn config_string(config: &[ConfigValue], key: &str, default: &str) -> String {
    match field_value(config, key) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => s,
        _ => default.to_string(),
    }
}

fn config_bool(config: &[ConfigValue], key: &str, default: bool) -> bool {
    match field_value(config, key) {
        Some(ConfigValue::Boolean(b)) => b,
        _ => default,
    }
}

fn config_enum_index(config: &[ConfigValue], key: &str) -> u32 {
    match field_value(config, key) {
        Some(ConfigValue::EnumIndex(i)) => i,
        _ => 0,
    }
}

fn config_tags(config: &[ConfigValue]) -> Vec<String> {
    match field_value(config, "tags") {
        Some(ConfigValue::StringList(tags)) => tags,
        _ => Vec::new(),
    }
}

/// The plugin implementation. `Guest` is the generated trait for the
/// world-level exports.
struct HelloPlugin;

impl Guest for HelloPlugin {
    fn name() -> String {
        // Metadata probes demonstrate that these calls also have instance state.
        mandara::plugin::state::set("demo:metadata-called", &[])
            .expect("metadata state is available");
        mandara::plugin::state::get("demo:name")
            .expect("metadata state is available")
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| "hello".into())
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
        let mut fields = vec![
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: mandara::plugin::types::ConfigKind::Text,
                default: Some("Mandara 示例库".into()),
                required: true,
                hint: Some("出现在推荐书目和识别提示里".into()),
            },
            ConfigField {
                key: "mode".into(),
                label: "内容版本".into(),
                kind: mandara::plugin::types::ConfigKind::EnumOptions(vec![
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
                kind: mandara::plugin::types::ConfigKind::Boolean,
                default: Some("false".into()),
                required: false,
                hint: Some("每次取章时写一条宿主日志".into()),
            },
            ConfigField {
                key: "spin".into(),
                label: "死循环测试(不要开启)".into(),
                kind: mandara::plugin::types::ConfigKind::Boolean,
                default: Some("false".into()),
                required: false,
                hint: Some("开启后取章会陷入死循环，用于演示宿主超时保护".into()),
            },
            ConfigField {
                key: "tags".into(),
                label: "额外标签".into(),
                kind: mandara::plugin::types::ConfigKind::ListOfString,
                default: Some("[]".into()),
                required: false,
                hint: Some("逗号分隔，会附在书目描述中".into()),
            },
        ];
        if mandara::plugin::state::get("demo:no-default")
            .expect("metadata state is available")
            .is_some()
        {
            fields[0].default = None;
        }
        fields
    }

    fn capabilities() -> Vec<String> {
        // declare: small static catalog synced eagerly; identify: upload
        // recognition; content: chapters + titles.
        vec!["declare".into(), "identify".into(), "content".into()]
    }

    /// Static catalog with supplied bodies, a lazy body and a valid empty body.
    fn declare(config: Vec<ConfigValue>) -> Option<Vec<DeclaredBook>> {
        let site = config_string(&config, "site-name", "Mandara 示例库");
        let tags = config_tags(&config);
        let tail = if tags.is_empty() {
            String::new()
        } else {
            format!("（{}）", tags.join("、"))
        };
        let book1 = BookEntry {
            id: "hello-1".into(),
            title: format!("Hello Mandara（{site}）"),
            authors: vec!["Mandara Team".into()],
            description: Some(format!(
                "这是一本由 wasm 插件依据配置生成的示例书。当前配置：站点={site}，版本=default。{tail}"
            )),
            cover_url: None,
            extra: None,
            series: None,
            volume_no: 0,
        };
        let book2 = BookEntry {
            id: "hello-2".into(),
            title: "Hello Mandara·幕后".into(),
            authors: vec!["Mandara Team".into()],
            description: Some("示例插件内部机制的说明。".into()),
            cover_url: None,
            // Extended-metadata demo: known keys validate against BookExt,
            // unknown keys round-trip verbatim.
            extra: Some(
                r#"{"publisher":"Mandara Press","pub_date":"2026-08","pages":42,"future_key":{"nested":true}}"#.into(),
            ),
            series: None,
            volume_no: 0,
        };
        Some(vec![
            DeclaredBook {
                book: book1,
                content: Some(DeclaredContent { id: "hello-content-1".into(), mode: ContentMode::Chapters, chapters: vec![
                    DeclaredChapter {
                        title: "第一章 你好，书架".into(),
                        content: Some(format!(
                            "你成功加载了一个 wasm 插件！\n\n当前站点名称由配置注入：{site}。{tail}\n\n这本书由 Rust 编写、以 WebAssembly 组件的形式运行在服务器上。\n\n插件在沙箱中运行，配置通过方法参数传入，宿主提供 log 等接口。"
                        )),
                        format: "text".into(),
                    },
                    DeclaredChapter {
                        title: "第二章 世界很大".into(),
                        content: Some("你可以用同样的方式写出自己的书籍来源插件：实现 capabilities、declare（或 search-books/get-book）、get-content 和 get-chapter 即可。\n\n参考 docs/plugins.md。".into()),
                        format: "text".into(),
                    },
                ], }),
            },
            DeclaredBook {
                book: book2,
                content: Some(DeclaredContent { id: "hello-content-2".into(), mode: ContentMode::Chapters, chapters: vec![DeclaredChapter {
                    title: "幕后".into(),
                    content: Some("这个插件的目录是声明式的数据，行为由配置驱动：\n\n- site-name 改变书名与正文；\n- mode 切换正文变体；\n- verbose 让插件记录宿主日志；\n- tags 附加到描述中。\n\n每次调用宿主都注入配置，插件内部没有任何跨调用状态。".into()),
                    format: "text".into(),
                }, DeclaredChapter {
                    title: "延迟加载".into(),
                    content: None,
                    format: "text".into(),
                }, DeclaredChapter {
                    title: "空章节".into(),
                    content: Some(String::new()),
                    format: "text".into(),
                }], }),
            },
        ])
    }

    fn search_books(
        _config: Vec<ConfigValue>,
        query: String,
        _offset: u32,
        _limit: u32,
    ) -> SearchResult {
        // No `search` capability: the host never calls this. Stub.
        let _ = query;
        SearchResult {
            total: 0,
            items: Vec::new(),
        }
    }

    fn get_book(_config: Vec<ConfigValue>, _book_id: String) -> Option<BookEntry> {
        // No `lookup` capability: the host never calls this. Stub.
        None
    }

    fn list_content(_config: Vec<ConfigValue>, _reference: String) -> Option<Vec<ContentOption>> {
        None
    }

    fn get_content(_config: Vec<ConfigValue>, content_id: String) -> Option<DeclaredContent> {
        let titles = match content_id.as_str() {
            "hello-content-1" => vec!["第一章 你好，书架", "第二章 世界很大"],
            "hello-content-2" => vec!["幕后", "延迟加载", "空章节"],
            _ => return None,
        };
        let chapters = titles
            .into_iter()
            .enumerate()
            .map(|(index, title)| DeclaredChapter {
                title: title.into(),
                format: "text".into(),
                content: (content_id == "hello-content-2" && index == 2).then(String::new),
            })
            .collect();
        Some(DeclaredContent {
            id: content_id,
            mode: ContentMode::Chapters,
            chapters,
        })
    }

    fn get_chapter(config: Vec<ConfigValue>, book_id: String, index: u32) -> Option<Chapter> {
        // An undeclared probe used by host tests and state API examples.
        if let Some(key) = book_id.strip_prefix("state:") {
            let content = match index {
                0 => format!("{:?}", mandara::plugin::state::get(key)),
                1 => format!(
                    "{:?}",
                    mandara::plugin::state::set(
                        key,
                        config_string(&config, "site-name", "Mandara 示例库").as_bytes(),
                    )
                ),
                2 => format!("{:?}", mandara::plugin::state::delete(key)),
                3 => format!(
                    "{:?}",
                    mandara::plugin::state::set(key, &vec![0; 256 * 1024 + 1])
                ),
                4 => format!("{:?}", mandara::plugin::state::set(key, &[])),
                _ => return None,
            };
            return Some(Chapter {
                title: "状态接口示例".into(),
                content,
                format: "text".into(),
            });
        }
        if config_bool(&config, "spin", false) {
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
        if config_bool(&config, "verbose", false) {
            mandara::plugin::store::log(&format!("get_chapter({book_id}, {index})"));
        }
        let mode = config_enum_index(&config, "mode");
        let alt = mode == 1;
        match (book_id.as_str(), index, alt) {
            ("hello-content-1", 0, false) => Some(Chapter {
                title: "第一章 你好，书架".into(),
                content: "你成功加载了一个 wasm 插件！\n\n这本书由 Rust 编写、以 WebAssembly 组件的形式运行在服务器上。\n\n插件在沙箱中运行，只能调用宿主提供的 log 接口。".into(),
                format: "text".into(),
            }),
            ("hello-content-1", 0, true) => Some(Chapter {
                title: "第一章 你好，书架（变体）".into(),
                content: "配置里选择了 alt 版本：这一章的正文与 default 版本不同。\n\n你成功加载了一个 wasm 插件，并且配置注入生效了！".into(),
                format: "text".into(),
            }),
            ("hello-content-1", 1, false) => Some(Chapter {
                title: "第二章 世界很大".into(),
                content: "你可以用同样的方式写出自己的书籍来源插件：实现 capabilities、declare（或 search-books/get-book）、chapter-titles 和 get-chapter 即可。\n\n参考 docs/plugins.md。".into(),
                format: "text".into(),
            }),
            ("hello-content-1", 1, true) => Some(Chapter {
                title: "第二章 世界很大（变体）".into(),
                content: "（alt 变体）不同配置实例可以指向同一个 wasm 文件：实例 id 是源 id，配置归属实例。".into(),
                format: "text".into(),
            }),
            ("hello-content-2", 0, _) => Some(Chapter {
                title: "幕后".into(),
                content: "这个插件的目录是声明式的数据，行为由配置驱动。\n\n每次调用宿主都注入配置，插件内部没有任何跨调用状态。".into(),
                format: "text".into(),
            }),
            ("hello-content-2", 1, _) => Some(Chapter {
                title: "延迟加载".into(),
                content: "声明目录只提供了标题，这段正文在读取时下载。".into(),
                format: "text".into(),
            }),
            ("hello-content-2", 2, _) => Some(Chapter {
                title: "空章节".into(),
                content: String::new(),
                format: "text".into(),
            }),
            _ => None,
        }
    }

    /// Example book identification: any uploaded file whose name contains
    /// "hello" is recognized as this plugin's sample book, so the server
    /// can take the metadata from here instead of parsing the file.
    fn identify_upload(
        config: Vec<ConfigValue>,
        filename: String,
        _file_hash: String,
    ) -> Option<BookEntry> {
        let lower = filename.to_lowercase();
        if lower.contains("hello") || lower.contains("示例") {
            let site = config_string(&config, "site-name", "Mandara 示例库");
            Some(BookEntry {
                id: "hello-1".into(),
                title: format!("Hello Mandara（{site}）"),
                authors: vec!["Mandara Team".into()],
                description: Some("这是一本由 wasm 插件提供的示例书。".into()),
                cover_url: None,
                extra: None,
                series: None,
                volume_no: 0,
            })
        } else {
            None
        }
    }

    fn get_series(_config: Vec<ConfigValue>, _series_id: String) -> Option<SeriesResult> {
        None
    }

    fn get_book_file(_config: Vec<ConfigValue>, _book_id: String) -> Option<BookFile> {
        // No `book-file` capability: the host never calls this. Stub.
        None
    }
}

export!(HelloPlugin);
