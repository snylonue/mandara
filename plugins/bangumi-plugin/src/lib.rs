//! Bangumi (bgm.tv) book-subject metadata source.
//!
//! A *metadata-only* plugin over the official Bangumi API
//! (`https://api.bgm.tv`, v0): `search` + `lookup`, no content — acquired
//! books are metadata entries; attach content by uploading files or
//! rebinding to a content plugin instance.
//!
//! - `search-books` → `POST /v0/search/subjects?limit=&offset=` (the
//!   pagination parameters live in the query string, not the body; body =
//!   `{keyword, filter:{type:[1]}}` for 书籍 subjects).
//! - `get-book` → `GET /v0/subjects/{id}`; the subject `infobox` is mapped
//!   onto `book-entry` fields and the extended-metadata JSON (`extra`):
//!   作者/译者/插画 → authors/translators/illustrators, 出版社/发售日/
//!   ISBN/页数/价格/副标题 → publisher/pub_date/isbn/pages/price/subtitle,
//!   plus tags. The original name is kept as `original_title`.
//!
//! **NSFW**: Bangumi hides NSFW subjects from unauthenticated requests.
//! Configure an access token (generate one at
//! <https://next.bgm.tv/demo/access-token>) and it is sent as
//! `Authorization: Bearer …` on every request.
//!
//! Every request carries a descriptive `User-Agent` (the API rejects
//! default/empty UAs). Transport-level failures surface as empty results /
//! `none` with the error logged; retrigger via the source browser or
//! refresh.
//!
//! Build:
//! ```sh
//! ./scripts/build-plugins.sh bangumi  # -> plugins-built/bangumi.wasm
//! ```

wit_bindgen::generate!({
    world: "mandara-plugin",
    path: "../../crates/mandara-plugin/wit",
});

const DEFAULT_BASE_URL: &str = "https://api.bgm.tv";

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 2] = ["base-url", "access-token"];

fn values() -> Vec<ConfigValue> {
    mandara::plugin::config::configure()
}

fn config_string(key: &str) -> Option<String> {
    let idx = FIELDS.iter().position(|k| *k == key).unwrap();
    match values().get(idx) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Descriptive User-Agent — required by the Bangumi developer guidelines.
const USER_AGENT: &str = "snylonue/mandara (https://github.com/snylonue/mandara)";

fn log_error(context: &str, url: &str, err: &str) {
    mandara::plugin::store::log(&format!("{context} `{url}` failed: {err}"));
}

/// One `http.fetch`; application statuses come back as `(status, body)` —
/// only transport-level problems are errors.
fn http_request(
    method: &str,
    url: &str,
    body: Option<Vec<u8>>,
    json_body: bool,
) -> Result<(u16, Vec<u8>), String> {
    let mut headers = vec![mandara::plugin::http::Header {
        name: "User-Agent".into(),
        value: USER_AGENT.into(),
    }];
    if let Some(token) = config_string("access-token") {
        headers.push(mandara::plugin::http::Header {
            name: "Authorization".into(),
            value: format!("Bearer {token}"),
        });
    }
    if json_body {
        headers.push(mandara::plugin::http::Header {
            name: "Content-Type".into(),
            value: "application/json".into(),
        });
    }
    let request = mandara::plugin::http::Request {
        method: method.into(),
        url: url.into(),
        headers,
        body,
        timeout_ms: Some(10_000),
    };
    match mandara::plugin::http::fetch(&request) {
        Ok(resp) => Ok((resp.status, resp.body)),
        Err(e) => Err(format!("{e:?}")),
    }
}

// ---------------------------------------------------------------------------
// Bangumi API types (only the fields we use)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ApiImages {
    #[serde(default)]
    large: Option<String>,
    #[serde(default)]
    common: Option<String>,
    #[serde(default)]
    medium: Option<String>,
}

impl ApiImages {
    fn best(&self) -> Option<String> {
        self.large
            .clone()
            .or_else(|| self.common.clone())
            .or_else(|| self.medium.clone())
            .filter(|u| !u.is_empty())
    }
}

#[derive(serde::Deserialize)]
struct ApiSubject {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    name_cn: String,
    #[serde(default)]
    summary: String,
    /// 发售日 / 出版日期 (`"2020-01-01"`); empty string when unknown.
    #[serde(default)]
    date: String,
    #[serde(default)]
    images: Option<ApiImages>,
    /// Whether this subject is a multi-volume series entry: `true` (bool)
    /// on the current API, an object `{id,type,name,name_cn}` on
    /// v0.34+. Single volumes are `false`/absent.
    #[serde(default)]
    series: Option<serde_json::Value>,
    /// Number of volumes when this subject is a series entry (0 for
    /// single volumes / non-series subjects).
    #[serde(default)]
    volumes: u32,
    /// `[{key: "出版社", value: "电击文库"}, {key: "作者", value: [...]}, …]`
    #[serde(default)]
    infobox: Vec<ApiInfobox>,
    /// `[{name: "科幻", count: 12}, …]`
    #[serde(default)]
    tags: Vec<ApiTag>,
}

impl ApiSubject {
    /// Is this subject a multi-volume series entry (bangumi marks these
    /// with `series` truthy and a volume count in `volumes`)?
    fn is_series(&self) -> bool {
        matches!(
            &self.series,
            Some(serde_json::Value::Bool(true)) | Some(serde_json::Value::Object(_))
        )
    }
}

#[derive(serde::Deserialize)]
struct ApiInfobox {
    key: String,
    /// A single string or a list of `{v: …}` rows (multi-value keys such
    /// as 作者 / 出版社).
    #[serde(default)]
    value: serde_json::Value,
}

#[derive(serde::Deserialize)]
struct ApiTag {
    #[serde(default)]
    name: String,
}

impl ApiInfobox {
    /// The infobox value as a flat list of strings: strings stay whole,
    /// arrays flatten their `{v}` rows, objects fall back to raw JSON.
    fn values(&self) -> Vec<String> {
        match &self.value {
            serde_json::Value::String(s) => {
                if s.trim().is_empty() {
                    Vec::new()
                } else {
                    vec![s.clone()]
                }
            }
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|it| {
                    it.get("v")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| it.as_str().map(|s| s.to_string()))
                })
                .filter(|s| !s.trim().is_empty())
                .collect(),
            other => other
                .as_str()
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
        }
    }

    fn first(&self) -> Option<String> {
        self.values().into_iter().next()
    }
}

#[derive(serde::Deserialize)]
struct ApiSearchResult {
    #[serde(default)]
    total: u64,
    #[serde(default)]
    data: Vec<ApiSubject>,
}

// ---------------------------------------------------------------------------
// Mapping: ApiSubject -> BookEntry (+ extra JSON)
// ---------------------------------------------------------------------------

/// Split an infobox author-ish string on the separators bangumi uses
/// inside single-string values ("A / B", "A、B").
fn split_names(s: &str) -> Vec<String> {
    s.split(['/', '、', '，', ','])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build the extended-metadata JSON object (`BookExt` shape) from the
/// infobox + top-level fields. Unknown infobox keys are preserved under
/// `infobox.{key}` so nothing is lost.
fn build_extra(subject: &ApiSubject) -> serde_json::Value {
    use serde_json::{Map, Value};

    let mut ext = Map::new();
    let mut misc = Map::new();

    // Original (usually Japanese) title.
    if !subject.name.trim().is_empty() && subject.name != subject.name_cn {
        ext.insert("original_title".into(), Value::String(subject.name.clone()));
    }

    for item in &subject.infobox {
        match item.key.as_str() {
            "中文名" | "别名" => {} // covered by name_cn / noise
            "副标题" => {
                if let Some(v) = item.first() {
                    ext.insert("subtitle".into(), Value::String(v));
                }
            }
            "作者" | "原作" => {
                let authors: Vec<String> =
                    item.values().iter().flat_map(|s| split_names(s)).collect();
                if !authors.is_empty() {
                    ext.entry("authors".to_string()).or_insert_with(|| {
                        Value::Array(authors.into_iter().map(Value::String).collect())
                    });
                }
            }
            "译者" | "翻译" => {
                let translators: Vec<String> =
                    item.values().iter().flat_map(|s| split_names(s)).collect();
                if !translators.is_empty() {
                    ext.insert(
                        "translators".into(),
                        Value::Array(translators.into_iter().map(Value::String).collect()),
                    );
                }
            }
            "插画" | "插图" | "作画" | "人物设定" | "插畫" => {
                let illustrators: Vec<String> =
                    item.values().iter().flat_map(|s| split_names(s)).collect();
                if !illustrators.is_empty() {
                    ext.insert(
                        "illustrators".into(),
                        Value::Array(illustrators.into_iter().map(Value::String).collect()),
                    );
                }
            }
            "出版社" | "發行" | "发行" => {
                if let Some(v) = item.first() {
                    ext.insert("publisher".into(), Value::String(v));
                }
            }
            "ISBN" | "ISBN-10" | "ISBN-13" => {
                if let Some(v) = item.first() {
                    // The host rejects invalid ISBNs by failing the whole
                    // call, so verify the checksum here and skip bad ones.
                    let cleaned = v.replace(['-', ' ', '–'], "");
                    if is_valid_isbn(&cleaned) {
                        ext.insert("isbn".into(), Value::String(cleaned.to_ascii_uppercase()));
                    }
                }
            }
            "页数" | "頁數" => {
                if let Some(v) = item.first() {
                    let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(pages) = digits.parse::<u32>() {
                        ext.insert("pages".into(), Value::from(pages));
                    }
                }
            }
            "价格" | "價格" => {
                if let Some(v) = item.first() {
                    ext.insert("price".into(), Value::String(v));
                }
            }
            "发售日" | "發售日" | "出版年月日" => {}
            _ => {
                // Preserve everything else verbatim.
                if let Some(v) = item.first() {
                    misc.insert(item.key.clone(), Value::String(v));
                }
            }
        }
    }

    // Publication date: explicit infobox date first, then the subject's
    // own `date` field.
    if let Some(date) = subject
        .infobox
        .iter()
        .find(|i| matches!(i.key.as_str(), "发售日" | "發售日" | "出版年月日"))
        .and_then(|i| i.first())
        .or_else(|| {
            if subject.date.trim().is_empty() {
                None
            } else {
                Some(subject.date.clone())
            }
        })
    {
        ext.insert("pub_date".into(), Value::String(date));
    }

    // Community tags (top few by count as returned).
    let tags: Vec<String> = subject
        .tags
        .iter()
        .filter(|t| !t.name.trim().is_empty())
        .take(10)
        .map(|t| t.name.clone())
        .collect();
    if !tags.is_empty() {
        ext.insert(
            "tags".into(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
    }

    if !misc.is_empty() {
        ext.insert("infobox_misc".into(), Value::Object(misc));
    }

    Value::Object(ext)
}

impl From<&ApiSubject> for BookEntry {
    fn from(s: &ApiSubject) -> Self {
        let extra = build_extra(s);
        // Prefer the localized title; fall back to the original name.
        let title = if s.name_cn.trim().is_empty() {
            s.name.clone()
        } else {
            s.name_cn.clone()
        };
        // Authors may also live in the extra JSON (infobox 作者); pull them
        // back out for the dedicated field so the library can filter/sort.
        let authors: Vec<String> = extra
            .get("authors")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        BookEntry {
            id: s.id.to_string(),
            title,
            authors,
            description: if s.summary.trim().is_empty() {
                None
            } else {
                Some(s.summary.clone())
            },
            cover_url: s.images.as_ref().and_then(ApiImages::best),
            extra: serde_json::to_string(&extra).ok(),
            content_source: None,
            content_id: None,
            // A bangumi **series** subject (e.g. 狼与香辛料 = 24 卷) is a
            // multi-volume publication family. Declare one volume-info per
            // 卷 (each with a single placeholder chapter — bangumi is
            // metadata-only, the host splits into one metadata book per
            // 卷 under a series; content can be attached later). Single
            // volumes stay `None`.
            volumes: if s.is_series() && s.volumes > 1 {
                Some(
                    (1..=s.volumes)
                        .map(|v| VolumeInfo {
                            title: format!("第{v}卷"),
                            chapter_count: 1,
                        })
                        .collect(),
                )
            } else {
                None
            },
        }
    }
}

fn empty_search() -> SearchResult {
    SearchResult {
        total: 0,
        items: Vec::new(),
    }
}

/// ISBN-10/13 checksum validation (hyphenless input). Guards against
/// infobox typos — the host fails the whole call on invalid ISBNs.
fn is_valid_isbn(raw: &str) -> bool {
    let s = raw.to_ascii_uppercase();
    match s.len() {
        10 => {
            let digits = s.as_bytes();
            digits.iter().enumerate().all(|(i, &b)| {
                if i < 9 {
                    b.is_ascii_digit()
                } else {
                    b.is_ascii_digit() || b == b'X'
                }
            }) && digits
                .iter()
                .enumerate()
                .map(|(i, &b)| {
                    let d = if b == b'X' { 10 } else { (b - b'0') as u32 };
                    d * (10 - i as u32)
                })
                .sum::<u32>()
                % 11
                == 0
        }
        13 => {
            let digits = s.as_bytes();
            digits.iter().all(u8::is_ascii_digit)
                && digits
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| {
                        let d = (b - b'0') as u32;
                        if i % 2 == 0 { d } else { d * 3 }
                    })
                    .sum::<u32>()
                    % 10
                    == 0
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// World exports
// ---------------------------------------------------------------------------

struct BangumiPlugin;

impl Guest for BangumiPlugin {
    fn name() -> String {
        "bangumi".into()
    }

    fn source_info() -> SourceInfo {
        SourceInfo {
            kind: "search".into(),
            id_kind: "numeric".into(),
            id_hint: Some("bangumi 条目编号（如 43142）".into()),
            search_hint: Some("搜索书名 / 原名…".into()),
        }
    }

    fn config_schema() -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "base-url".into(),
                label: "API 地址".into(),
                kind: mandara::plugin::config::ConfigKind::Text,
                default: Some(DEFAULT_BASE_URL.into()),
                required: true,
                hint: Some("Bangumi 官方 API 根地址，一般无需修改".into()),
            },
            ConfigField {
                key: "access-token".into(),
                label: "Access Token".into(),
                kind: mandara::plugin::config::ConfigKind::Text,
                default: None,
                required: false,
                hint: Some(
                    "个人访问令牌（https://next.bgm.tv/demo/access-token 生成）；NSFW 条目需要鉴权才能看到，留空则只能搜索全年龄条目"
                        .into(),
                ),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        // Metadata only: browse + fetch by id; no catalog dump, no
        // content, no upload identification.
        vec!["search".into(), "lookup".into()]
    }

    fn declare() -> Option<Vec<DeclaredBook>> {
        None // huge source: never enumerated eagerly
    }

    fn search_books(query: String, offset: u32, limit: u32) -> SearchResult {
        if query.trim().is_empty() {
            return empty_search(); // the search endpoint requires a keyword
        }
        let base = config_string("base-url").unwrap_or_else(|| DEFAULT_BASE_URL.into());
        let url = format!("{base}/v0/search/subjects?limit={limit}&offset={offset}");
        let body = serde_json::json!({
            "keyword": query,
            "sort": "match",
            "filter": { "type": [1] }, // 书籍 subjects only
        });
        let payload = serde_json::to_vec(&body).ok();
        match http_request("POST", &url, payload, true) {
            Ok((200, resp_body)) => match serde_json::from_slice::<ApiSearchResult>(&resp_body) {
                Ok(result) => SearchResult {
                    total: result.total,
                    items: result.data.iter().map(BookEntry::from).collect(),
                },
                Err(e) => {
                    log_error("search", &url, &format!("json: {e}"));
                    empty_search()
                }
            },
            Ok((401, _)) => {
                log_error("search", &url, "status 401：access token 无效或已过期");
                empty_search()
            }
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
        let base = config_string("base-url").unwrap_or_else(|| DEFAULT_BASE_URL.into());
        let url = format!("{base}/v0/subjects/{book_id}");
        match http_request("GET", &url, None, false) {
            Ok((200, resp_body)) => match serde_json::from_slice::<ApiSubject>(&resp_body) {
                Ok(subject) => Some((&subject).into()),
                Err(e) => {
                    log_error("get-book", &url, &format!("json: {e}"));
                    None
                }
            },
            // 401/404: NSFW without token surfaces as 404/401 upstream —
            // tell the two cases apart in the log.
            Ok((401 | 403, _)) => {
                // NSFW subjects without a token also come back as 401/404.
                log_error(
                    "get-book",
                    &url,
                    "status 401/403：条目可能是 NSFW 且未配置 access token，或 token 无效/已过期",
                );
                None
            }
            Ok((404, _)) => None,
            Ok((status, _)) => {
                log_error("get-book", &url, &format!("status {status}"));
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

export!(BangumiPlugin);
