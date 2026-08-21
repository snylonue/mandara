//! Guest side of the `bookshelf:plugin/bookshelf-plugin` world.
//!
//! Build:
//! ```sh
//! cargo build -p hello-plugin --release --target wasm32-unknown-unknown
//! wasm-tools component new \
//!     target/wasm32-unknown-unknown/release/hello_plugin.wasm \
//!     -o bookshelf-hello.wasm
//! # copy bookshelf-hello.wasm to the server plugins dir (default: data/plugins)
//! ```
//!
//! The generated core module embeds a `component-type` custom section, so
//! `wasm-tools component new` can lift it into a component without adapters
//! (this world does not import any wasi interfaces).

wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

// `BookEntry`, `Chapter` and the `Guest` trait are generated in this scope by
// generate!() (the world `use`s the types interface).

/// The plugin implementation. `Guest` is the generated trait for the
/// world-level exports (`name`, `list-books`, `chapter-titles`,
/// `get-chapter`).
struct HelloPlugin;

impl Guest for HelloPlugin {
    fn name() -> String {
        "hello".to_string()
    }

    fn list_books() -> Vec<BookEntry> {
        vec![BookEntry {
            id: "hello-1".into(),
            title: "Hello Bookshelf(示例书)".into(),
            authors: vec!["Bookshelf Team".into()],
            description: Some("这是一本由 wasm 插件提供的示例书。".into()),
            cover_url: None,
        }]
    }

    fn chapter_titles(_book_id: String) -> Vec<String> {
        ["第一章 你好，书架", "第二章 世界很大"].iter().map(|s| s.to_string()).collect()
    }

    fn get_chapter(book_id: String, index: u32) -> Option<Chapter> {
        match (book_id.as_str(), index) {
            ("hello-1", 0) => Some(Chapter {
                title: "第一章 你好，书架".into(),
                content: "你成功加载了一个 wasm 插件！\n\n这本书由 Rust 编写、以 WebAssembly 组件的形式运行在服务器上。\n\n插件在沙箱中运行，只能调用宿主提供的 log 接口。".into(),
            }),
            ("hello-1", 1) => Some(Chapter {
                title: "第二章 世界很大".into(),
                content: "你可以用同样的方式写出自己的书籍来源插件：实现 list-books、chapter-titles 和 get-chapter 即可。\n\n参考 docs/plugins.md。".into(),
            }),
            _ => None,
        }
    }
}

export!(HelloPlugin);