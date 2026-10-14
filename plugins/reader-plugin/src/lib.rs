//! Guest side of the `mandara:plugin/mandara-plugin` world (v3).
//!
//! A *content* source speaking HTTP (chapter mode): the reading-site
//! catalog lives on a remote site (config `base-url`), fetched through
//! the host's `http.fetch` import — the only network a plugin gets (see
//! `docs/plugin-http-api-design.md`).
//!
//! Users select this content source independently of a metadata source
//! in the host acquisition flow, or acquire its books directly.
//!
//! The demo source is `scripts/mock-source.py` (port 8765); point
//! `base-url` at any HTTP API with the same shape:
//!   GET /api/search?q=&page_size=&offset=&limit= -> {"total","items":[...]}
//!   GET /api/books/{id}                          -> book-entry | 404
//!   GET /api/books/{id}/content?max=             -> {"mode","chapters":[...]} | 404
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
    world: "mandara-plugin",
    path: "../../crates/mandara-plugin/wit",
});

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8765/reader";

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 3] = ["base-url", "site-name", "max-chapters"];

fn config_string(config: &[ConfigValue], key: &str, default: &str) -> String {
    let idx = FIELDS.iter().position(|k| *k == key).unwrap();
    match config.get(idx) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => s.clone(),
        _ => default.to_string(),
    }
}

fn config_number(config: &[ConfigValue], key: &str, default: u32) -> u32 {
    let idx = FIELDS.iter().position(|k| *k == key).unwrap();
    match config.get(idx) {
        Some(ConfigValue::Number(n)) => n.clamp(1.0, 20.0) as u32,
        _ => default,
    }
}

fn log_error(context: &str, url: &str, err: &(impl std::fmt::Display + ?Sized)) {
    mandara::plugin::store::log(&format!("{context} `{url}` failed: {err}"));
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
fn http_get(url: &str) -> Result<(u16, Vec<u8>), mandara::plugin::http::FetchError> {
    let request = mandara::plugin::http::Request {
        method: "GET".into(),
        url: url.into(),
        headers: Vec::new(),
        body: None,
        timeout_ms: Some(5_000),
    };
    let resp = mandara::plugin::http::fetch(&request)?;
    Ok((resp.status, resp.body))
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
    series: Option<JsonSeries>,
    #[serde(default)]
    volume_no: u32,
}

#[derive(serde::Deserialize)]
struct JsonSeries {
    id: String,
    title: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cover_url: Option<String>,
}

impl From<JsonSeries> for SeriesEntry {
    fn from(s: JsonSeries) -> Self {
        Self {
            id: s.id,
            title: s.title,
            authors: s.authors,
            description: s.description,
            cover_url: s.cover_url,
            extra: None,
        }
    }
}

#[derive(serde::Deserialize)]
struct JsonSeriesResult {
    series: JsonSeries,
    books: Vec<JsonEntry>,
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
            series: e.series.map(SeriesEntry::from),
            volume_no: e.volume_no,
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
                kind: mandara::plugin::types::ConfigKind::Text,
                default: Some(DEFAULT_BASE_URL.into()),
                required: true,
                hint: Some(
                    "远程 API 的根地址（含命名空间），如 http://127.0.0.1:8765/reader".into(),
                ),
            },
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: mandara::plugin::types::ConfigKind::Text,
                default: Some("远程阅读源".into()),
                required: true,
                hint: Some("显示在词条描述里".into()),
            },
            ConfigField {
                key: "max-chapters".into(),
                label: "每章上限".into(),
                kind: mandara::plugin::types::ConfigKind::Number,
                default: Some("3".into()),
                required: false,
                hint: Some("每本书最多提供的章节数（1~20，向上游 max 参数）".into()),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        // Availability in upstream metadata selects each book's content form.
        vec![
            "search".into(),
            "lookup".into(),
            "content".into(),
            "book-file".into(),
        ]
    }

    fn declare(_config: Vec<ConfigValue>) -> Option<Vec<DeclaredBook>> {
        None // large source: never enumerated eagerly
    }

    fn search_books(
        config: Vec<ConfigValue>,
        query: String,
        offset: u32,
        limit: u32,
    ) -> SearchResult {
        let base = config_string(&config, "base-url", DEFAULT_BASE_URL);
        let url = format!(
            "{base}/api/search?q={}&offset={offset}&limit={limit}",
            urlencode(&query)
        );
        match http_get(&url) {
            Ok((200, body)) => match serde_json::from_slice::<JsonSearch>(&body) {
                Ok(result) => SearchResult {
                    total: result.total,
                    items: result
                        .items
                        .into_iter()
                        .map(|e| CatalogEntry::Single(e.into()))
                        .collect(),
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

    fn get_book(config: Vec<ConfigValue>, book_id: String) -> Option<BookEntry> {
        let base = config_string(&config, "base-url", DEFAULT_BASE_URL);
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

    fn get_content(config: Vec<ConfigValue>, content_id: String) -> Option<DeclaredContent> {
        let base = config_string(&config, "base-url", DEFAULT_BASE_URL);
        let max = config_number(&config, "max-chapters", 3);
        let url = format!("{base}/api/books/{content_id}/content?max={max}");
        match http_get(&url) {
            Ok((200, body)) => {
                let value: serde_json::Value = serde_json::from_slice(&body)
                    .map_err(|e| log_error("get-content", &url, &e))
                    .ok()?;
                let mode = match value.get("mode")?.as_str()? {
                    "file" => ContentMode::File,
                    "chapters" => ContentMode::Chapters,
                    _ => return None,
                };
                let chapters = value
                    .get("chapters")?
                    .as_array()?
                    .iter()
                    .map(|chapter| {
                        Some(DeclaredChapter {
                            title: chapter.get("title")?.as_str()?.into(),
                            format: chapter.get("format")?.as_str()?.into(),
                            content: match chapter.get("content")? {
                                serde_json::Value::Null => None,
                                serde_json::Value::String(body) => Some(body.clone()),
                                _ => return None,
                            },
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(DeclaredContent {
                    id: content_id,
                    mode,
                    chapters,
                })
            }
            Ok((status, _)) => {
                if status != 404 {
                    log_error("get-content", &url, &format!("status {status}"));
                }
                None
            }
            Err(error) => {
                log_error("get-content", &url, &error);
                None
            }
        }
    }

    fn get_chapter(config: Vec<ConfigValue>, book_id: String, index: u32) -> Option<Chapter> {
        let base = config_string(&config, "base-url", DEFAULT_BASE_URL);
        let url = format!("{base}/api/books/{book_id}/chapters/{index}");
        match http_get(&url) {
            Ok((200, body)) => match serde_json::from_slice::<JsonChapter>(&body) {
                Ok(chapter) => Some(Chapter {
                    title: chapter.title,
                    content: chapter.content,
                    format: "text".into(),
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

    fn identify_upload(
        _config: Vec<ConfigValue>,
        _filename: String,
        _file_hash: String,
    ) -> Option<BookEntry> {
        None // no `identify` capability; host never calls this
    }

    fn get_series(config: Vec<ConfigValue>, series_id: String) -> Option<SeriesResult> {
        let base = config_string(&config, "base-url", DEFAULT_BASE_URL);
        let url = format!("{base}/api/series/{}", urlencode(&series_id));
        match http_get(&url) {
            Ok((200, body)) => serde_json::from_slice::<JsonSeriesResult>(&body)
                .ok()
                .map(|s| SeriesResult {
                    series: s.series.into(),
                    books: s.books.into_iter().map(BookEntry::from).collect(),
                }),
            _ => None,
        }
    }

    fn get_book_file(config: Vec<ConfigValue>, book_id: String) -> Option<BookFile> {
        let base = config_string(&config, "base-url", DEFAULT_BASE_URL);
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
