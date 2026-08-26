//! Guest side of the `bookshelf:plugin/bookshelf-plugin` world (v3).
//!
//! A *content* source speaking HTTP (chapter mode): the reading-site
//! catalog lives on a remote site (config `base-url`), fetched through
//! the host's `http.fetch` import — the only network a plugin gets (see
//! `docs/plugin-http-api-design.md`).
//!
//! The demo flow: the wiki metadata entry points `content-source:
//! "reader"`, `content-id: "r-N"`, and the host asks this instance for
//! titles/chapters of `r-N`. A user can also materialize `r-N` directly
//! through the source browser.
//!
//! The demo source is `scripts/mock-source.py` (port 8765); point
//! `base-url` at any HTTP API with the same shape:
//!   GET /api/search?q=&page_size=&offset=&limit= -> {"total","items":[...]}
//!   GET /api/books/{id}                          -> book-entry | 404
//!   GET /api/books/{id}/chapters?max=            -> {"titles":[...]}
//!   GET /api/books/{id}/chapters/{i}             -> {"title","content"} | 404
//!
//! Fetch failures surface as empty results / `none` (the guest logs the
//! underlying error); retrigger via the source browser / refresh.
//!
//! Build:
//! ```sh
//! ./scripts/build-plugins.sh reader -o plugins-built/reader.wasm
//! # register:   POST /api/plugins/instances {"id": "reader", "wasm_file": "reader.wasm"}
//! ```

wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8765/reader";

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 3] = ["base-url", "site-name", "max-chapters"];

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
        Some(ConfigValue::Number(n)) => n.clamp(1.0, 20.0) as u32,
        _ => default,
    }
}

fn log_error(context: &str, url: &str, err: &str) {
    bookshelf::plugin::store::log(&format!("{context} `{url}` failed: {err}"));
}

/// RFC 3986 percent-encoding of a UTF-8 string (query values).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// One remote `http.fetch` (timeout 5 s); application statuses come back
/// as `(status, body)` — only transport-level problems are errors.
fn http_get(url: &str) -> Result<(u16, Vec<u8>), String> {
    let request = bookshelf::plugin::http::Request {
        method: "GET".into(),
        url: url.into(),
        headers: Vec::new(),
        body: None,
        timeout_ms: Some(5_000),
    };
    match bookshelf::plugin::http::fetch(&request) {
        Ok(resp) => Ok((resp.status, resp.body)),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// The wire format of a `book-entry` on the remote source.
#[derive(serde::Deserialize)]
struct JsonEntry {
    id: String,
    title: String,
    authors: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cover_url: Option<String>,
    #[serde(default)]
    content_source: Option<String>,
    #[serde(default)]
    content_id: Option<String>,
    /// Multi-volume structure (WIT volume-info), when the source
    /// declares it; the host splits acquisition into one book per 卷.
    #[serde(default)]
    volumes: Option<Vec<JsonVolume>>,
}

#[derive(serde::Deserialize)]
struct JsonVolume {
    title: String,
    #[serde(rename = "chapter-count")]
    chapter_count: u32,
}

impl From<JsonEntry> for BookEntry {
    fn from(e: JsonEntry) -> Self {
        BookEntry {
            id: e.id,
            title: e.title,
            authors: e.authors,
            description: e.description,
            cover_url: e.cover_url,
            extra: None,
            content_source: e.content_source,
            content_id: e.content_id,
            volumes: e.volumes.map(|vs| {
                vs.into_iter()
                    .map(|v| VolumeInfo {
                        title: v.title,
                        chapter_count: v.chapter_count,
                    })
                    .collect()
            }),
        }
    }
}

#[derive(serde::Deserialize)]
struct JsonSearch {
    total: u64,
    items: Vec<JsonEntry>,
}

#[derive(serde::Deserialize)]
struct JsonChapter {
    title: String,
    content: String,
}

fn empty_search() -> SearchResult {
    SearchResult {
        total: 0,
        items: Vec::new(),
    }
}

struct ReaderPlugin;

impl Guest for ReaderPlugin {
    fn name() -> String {
        "reader".into()
    }

    fn source_info() -> SourceInfo {
        // Searchable content source (chapter + file mode).
        SourceInfo {
            kind: "search".into(),
            id_kind: "free".into(),
            id_hint: None,
            search_hint: Some("搜索书名 / 作者…".into()),
        }
    }

    fn config_schema() -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "base-url".into(),
                label: "站点地址".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: Some(DEFAULT_BASE_URL.into()),
                required: true,
                hint: Some(
                    "远程 API 的根地址（含命名空间），如 http://127.0.0.1:8765/reader".into(),
                ),
            },
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: Some("远程阅读源".into()),
                required: true,
                hint: Some("显示在词条描述里".into()),
            },
            ConfigField {
                key: "max-chapters".into(),
                label: "每章上限".into(),
                kind: bookshelf::plugin::config::ConfigKind::Number,
                default: Some("3".into()),
                required: false,
                hint: Some("每本书最多提供的章节数（1~20，向上游 max 参数）".into()),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        // Chapter mode (`content`) + file mode (`book-file`): the host
        // tries the whole-file download first and falls back to
        // per-chapter pulls when `get-book-file` returns none.
        vec![
            "search".into(),
            "lookup".into(),
            "content".into(),
            "book-file".into(),
        ]
    }

    fn declare() -> Option<Vec<DeclaredBook>> {
        None // large source: never enumerated eagerly
    }

    fn search_books(query: String, offset: u32, limit: u32) -> SearchResult {
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let url = format!(
            "{base}/api/search?q={}&offset={offset}&limit={limit}",
            urlencode(&query)
        );
        match http_get(&url) {
            Ok((200, body)) => match serde_json::from_slice::<JsonSearch>(&body) {
                Ok(result) => SearchResult {
                    total: result.total,
                    items: result.items.into_iter().map(BookEntry::from).collect(),
                },
                Err(e) => {
                    log_error("search", &url, &format!("json: {e}"));
                    empty_search()
                }
            },
            Ok((status, _)) => {
                log_error("search", &url, &format!("status {status}"));
                empty_search()
            }
            Err(e) => {
                log_error("search", &url, &e);
                empty_search()
            }
        }
    }

    fn get_book(book_id: String) -> Option<BookEntry> {
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let url = format!("{base}/api/books/{book_id}");
        match http_get(&url) {
            Ok((200, body)) => match serde_json::from_slice::<JsonEntry>(&body) {
                Ok(entry) => Some(entry.into()),
                Err(e) => {
                    log_error("get-book", &url, &format!("json: {e}"));
                    None
                }
            },
            Ok((status, _)) => {
                if status != 404 {
                    log_error("get-book", &url, &format!("status {status}"));
                }
                None
            }
            Err(e) => {
                log_error("get-book", &url, &e);
                None
            }
        }
    }

    fn chapter_titles(book_id: String) -> Vec<String> {
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let max = config_number("max-chapters", 3);
        let url = format!("{base}/api/books/{book_id}/chapters?max={max}");
        match http_get(&url) {
            Ok((200, body)) => match serde_json::from_slice::<serde_json::Value>(&body) {
                Ok(value) => value
                    .get("titles")
                    .and_then(|t| t.as_array())
                    .map(|titles| {
                        titles
                            .iter()
                            .filter_map(|t| t.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(e) => {
                    log_error("chapter-titles", &url, &format!("json: {e}"));
                    Vec::new()
                }
            },
            Ok((status, _)) => {
                if status != 404 {
                    log_error("chapter-titles", &url, &format!("status {status}"));
                }
                Vec::new()
            }
            Err(e) => {
                log_error("chapter-titles", &url, &e);
                Vec::new()
            }
        }
    }

    fn get_chapter(book_id: String, index: u32) -> Option<Chapter> {
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let url = format!("{base}/api/books/{book_id}/chapters/{index}");
        match http_get(&url) {
            Ok((200, body)) => match serde_json::from_slice::<JsonChapter>(&body) {
                Ok(chapter) => Some(Chapter {
                    title: chapter.title,
                    content: chapter.content,
                }),
                Err(e) => {
                    log_error("get-chapter", &url, &format!("json: {e}"));
                    None
                }
            },
            Ok((status, _)) => {
                if status != 404 {
                    log_error("get-chapter", &url, &format!("status {status}"));
                }
                None
            }
            Err(e) => {
                log_error("get-chapter", &url, &e);
                None
            }
        }
    }

    fn identify_upload(_filename: String, _file_hash: String) -> Option<BookEntry> {
        None // no `identify` capability; host never calls this
    }

    fn get_book_file(book_id: String) -> Option<BookFile> {
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let url = format!("{base}/api/books/{book_id}/download");
        match http_get(&url) {
            Ok((200, bytes)) => Some(BookFile {
                filename: format!("{book_id}.epub"),
                mime: "application/epub+zip".into(),
                bytes,
            }),
            Ok((status, _)) => {
                if status != 404 {
                    log_error("get-book-file", &url, &format!("status {status}"));
                }
                None
            }
            Err(e) => {
                log_error("get-book-file", &url, &e);
                None
            }
        }
    }
}

export!(ReaderPlugin);
