//! Guest side of the `mandara:plugin/mandara-plugin` world (v3).
//!
//! A *metadata-only* source speaking HTTP: the catalog lives on a remote
//! site (config `base-url`), fetched through the host's `http.fetch`
//! import — the only network a plugin gets (see
//! `docs/plugin-http-api-design.md`). Every `book-entry` points its
//! `content-source` at the `reader` instance and its `content-id` at the
//! reader's book id, so the host materializes metadata from here and
//! chapters from the reader plugin (R5 demo).
//!
//! The demo source is `scripts/mock-source.py` (port 8765); point
//! `base-url` at any HTTP API with the same shape:
//!   GET /api/search?q=&page_size=&offset=&limit= -> {"total","items":[...]}
//!   GET /api/books/{id}                          -> book-entry | 404
//!
//! Fetch failures surface as empty results (the guest logs the
//! underlying error); retrigger via the source browser / refresh.
//!
//! Build:
//! ```sh
//! ./scripts/build-plugins.sh wiki   # -> plugins-built/wiki.wasm
//! # register:   POST /api/plugins/instances {"id": "wiki", "wasm_file": "wiki.wasm"}
//! ```

wit_bindgen::generate!({
    world: "mandara-plugin",
    path: "../../crates/mandara-plugin/wit",
});

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8765/wiki";

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 4] = ["base-url", "api-key", "site-name", "page-size"];

fn values() -> Vec<ConfigValue> {
    mandara::plugin::config::configure()
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

fn log_error(context: &str, url: &str, err: &str) {
    mandara::plugin::store::log(&format!("{context} `{url}` failed: {err}"));
}

/// RFC 3986 percent-encoding of a UTF-8 string (query values; the host
/// logs are query-stripped, so special characters are fine here).
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
    let mut headers = Vec::new();
    let api_key = config_string("api-key", "");
    if !api_key.is_empty() {
        headers.push(mandara::plugin::http::Header {
            name: "X-Api-Key".into(),
            value: api_key,
        });
    }
    let request = mandara::plugin::http::Request {
        method: "GET".into(),
        url: url.into(),
        headers,
        body: None,
        timeout_ms: Some(5_000),
    };
    match mandara::plugin::http::fetch(&request) {
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
    /// Multi-volume structure (WIT volume-info), when the metadata source
    /// declares it — the host splits acquisition into one book per 卷.
    /// Multi-volume is the METADATA source's responsibility, so a metadata
    /// source relays its own 卷 structure here independent of the content
    /// source's book.
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

fn empty_search() -> SearchResult {
    SearchResult {
        total: 0,
        items: Vec::new(),
    }
}

struct WikiPlugin;

impl Guest for WikiPlugin {
    fn name() -> String {
        "wiki".into()
    }

    fn source_info() -> SourceInfo {
        // Searchable metadata-only source (content comes from reader).
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
                kind: mandara::plugin::config::ConfigKind::Text,
                default: Some(DEFAULT_BASE_URL.into()),
                required: true,
                hint: Some("远程 API 的根地址（含命名空间），如 http://127.0.0.1:8765/wiki".into()),
            },
            ConfigField {
                key: "api-key".into(),
                label: "API 密钥".into(),
                kind: mandara::plugin::config::ConfigKind::Text,
                default: None,
                required: false,
                hint: Some("非空时作为 X-Api-Key 请求头发送；仅在管理员配置页可见".into()),
            },
            ConfigField {
                key: "site-name".into(),
                label: "站点名称".into(),
                kind: mandara::plugin::config::ConfigKind::Text,
                default: Some("维基书源".into()),
                required: true,
                hint: Some("显示在词条描述里".into()),
            },
            ConfigField {
                key: "page-size".into(),
                label: "词条数量".into(),
                kind: mandara::plugin::config::ConfigKind::Number,
                default: Some("8".into()),
                required: false,
                hint: Some("向上游请求的词条数上限（page_size 参数）".into()),
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
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let page_size = config_number("page-size", 8);
        let url = format!(
            "{base}/api/search?q={}&page_size={page_size}&offset={offset}&limit={limit}",
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
                // 404 = not found, everything else = logged + none.
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
