//! Novelia's public JSON API, adapted to Mandara's stateless WIT world.
//! Web novels provide metadata and chapters; Wenku provides content only.
//! Wenku content uses the upstream volumeId, scoped by its series ID.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;

wit_bindgen::generate!({
    world: "mandara-plugin",
    path: "../../crates/mandara-plugin/wit",
});

const BASE: &str = "https://n.novelia.cc";
const AUTH_BASE: &str = "https://auth.novelia.cc";
const PROVIDERS: &[&str] = &[
    "syosetu",
    "kakuyomu",
    "hameln",
    "pixiv",
    "alphapolis",
    "novelup",
];
const PAGE_SIZE: u32 = 100;

struct Settings {
    base: String,
    providers: Vec<String>,
    translation: u32,
    username: String,
    password: String,
    auth_base: String,
}

const TOKEN_KEY: &str = "auth:v1";

#[derive(Deserialize, Serialize)]
struct CachedToken {
    scope: String,
    token: String,
}

fn endpoint(base: &str, segments: &[&str]) -> Option<Url> {
    let mut url = Url::parse(base).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .ok()?
        .pop_if_empty()
        .extend(segments);
    Some(url)
}

impl Settings {
    fn from_values(values: &[ConfigValue]) -> Self {
        let text = |i, default: &str| match values.get(i) {
            Some(ConfigValue::Text(s)) => s.clone(),
            _ => default.into(),
        };
        Self {
            base: text(0, BASE),
            providers: match values.get(1) {
                Some(ConfigValue::StringList(p)) => p.clone(),
                _ => PROVIDERS.iter().map(|p| (*p).into()).collect(),
            },
            translation: match values.get(2) {
                Some(ConfigValue::EnumIndex(n)) => *n,
                _ => 0,
            },
            username: text(3, ""),
            password: text(4, ""),
            auth_base: text(5, AUTH_BASE),
        }
    }

    fn url(&self, segments: &[&str]) -> Option<Url> {
        endpoint(&self.base, segments)
    }

    fn login(&self) -> Option<String> {
        use mandara::plugin::http::{Header, Request, fetch};
        let url = endpoint(&self.auth_base, &["api", "v1", "auth", "login"])?;
        // Do not retry password submissions, downgrade to guest, or log bodies.
        let response = match fetch(&Request {
            method: "POST".into(),
            url: url.to_string(),
            headers: vec![
                Header {
                    name: "Content-Type".into(),
                    value: "application/json".into(),
                },
                Header {
                    name: "User-Agent".into(),
                    value: "snylonue/mandara Novelia plugin".into(),
                },
            ],
            body: Some(
                json!({"app": "n", "username": self.username, "password": self.password})
                    .to_string()
                    .into_bytes(),
            ),
            timeout_ms: Some(30_000),
        }) {
            Ok(response) => response,
            Err(_) => {
                log("Novelia login transport/policy failure");
                return None;
            }
        };
        if response.status != 200 {
            log(&format!("Novelia login HTTP {}", response.status));
            return None;
        }
        String::from_utf8(response.body)
            .ok()
            .filter(|token| !token.is_empty())
    }

    fn auth_scope(&self) -> String {
        // Hash the full credential/endpoint tuple without persisting another password copy.
        let scope = json!([self.base, self.auth_base, self.username, self.password]);
        format!("{:x}", Sha256::digest(scope.to_string().as_bytes()))
    }

    fn fresh_token(&self) -> Option<String> {
        let token = self.login()?;
        let entry = CachedToken {
            scope: self.auth_scope(),
            token: token.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec(&entry)
            && mandara::plugin::state::set(TOKEN_KEY, &bytes).is_err()
        {
            log("Novelia token cache write failed");
        }
        Some(token)
    }

    fn cached_token(&self) -> Option<String> {
        mandara::plugin::state::get(TOKEN_KEY)
            .ok()
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<CachedToken>(&bytes).ok())
            .filter(|entry| entry.scope == self.auth_scope() && !entry.token.is_empty())
            .map(|entry| entry.token)
    }

    fn invalidate_token(&self) {
        if mandara::plugin::state::delete(TOKEN_KEY).is_err() {
            log("Novelia token cache delete failed");
        }
    }

    fn translators(&self) -> &'static [&'static str] {
        match self.translation {
            1 => &["gpt"],
            2 => &["sakura"],
            3 => &["youdao"],
            4 => &[],
            _ => &["gpt", "sakura", "youdao"],
        }
    }

    fn fetch(&self, url: &Url, authenticated: bool) -> Option<Vec<u8>> {
        use mandara::plugin::http::{Header, Request, fetch};
        let authenticated =
            authenticated && (!self.username.is_empty() || !self.password.is_empty());
        let mut token = if authenticated {
            self.cached_token()
        } else {
            None
        };
        let mut refresh_auth = token.is_some();
        if authenticated && token.is_none() {
            token = Some(self.fresh_token()?);
        }
        let mut retry_transient = true;
        loop {
            let mut headers = vec![Header {
                name: "User-Agent".into(),
                value: "snylonue/mandara Novelia plugin".into(),
            }];
            if let Some(token) = &token {
                headers.push(Header {
                    name: "Authorization".into(),
                    value: format!("Bearer {token}"),
                });
            }
            match fetch(&Request {
                method: "GET".into(),
                url: url.to_string(),
                headers,
                body: None,
                timeout_ms: Some(30_000),
            }) {
                Ok(r) if r.status == 200 => return Some(r.body),
                Ok(r) if r.status == 401 && authenticated => {
                    self.invalidate_token();
                    // Only a reused token can trigger one new login. Never retry
                    // a rejected freshly issued token or a failed password submission.
                    if refresh_auth {
                        refresh_auth = false;
                        token = Some(self.fresh_token()?);
                        continue;
                    }
                    log("Novelia API rejected the access token");
                    return None;
                }
                Ok(r) if r.status >= 500 && retry_transient => {
                    retry_transient = false;
                }
                Ok(r) => {
                    log(&format!("Novelia HTTP {} at {}", r.status, url.path()));
                    return None;
                }
                Err(
                    mandara::plugin::http::FetchError::Timeout(_)
                    | mandara::plugin::http::FetchError::Transport(_),
                ) if retry_transient => {
                    retry_transient = false;
                }
                Err(_) => {
                    log("Novelia HTTP transport/policy failure");
                    return None;
                }
            }
        }
    }

    fn get<T: serde::de::DeserializeOwned>(&self, url: &Url) -> Option<T> {
        let bytes = self.fetch(url, true)?;
        match serde_json::from_slice(&bytes) {
            Ok(value) => Some(value),
            Err(_) => {
                log("Novelia returned an invalid JSON payload");
                None
            }
        }
    }

    fn web(&self, provider: &str, novel: &str) -> Option<WebNovel> {
        self.get(&self.url(&["api", "novel", provider, novel])?)
    }

    fn wenku(&self, novel: &str) -> Option<WenkuNovel> {
        self.get(&self.url(&["api", "wenku", novel])?)
    }
}

fn log(message: &str) {
    mandara::plugin::store::log(message);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    page_number: u64,
    items: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebOutline {
    provider_id: String,
    novel_id: String,
    title_jp: String,
    #[serde(default)]
    title_zh: Option<String>,
}

#[derive(Deserialize)]
struct Author {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TocItem {
    title_jp: String,
    #[serde(default)]
    title_zh: Option<String>,
    #[serde(default)]
    chapter_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebNovel {
    title_jp: String,
    #[serde(default)]
    title_zh: Option<String>,
    authors: Vec<Author>,
    introduction_jp: String,
    #[serde(default)]
    introduction_zh: Option<String>,
    toc: Vec<TocItem>,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebChapter {
    title_jp: String,
    #[serde(default)]
    title_zh: Option<String>,
    paragraphs: Vec<String>,
    #[serde(default)]
    gpt_paragraphs: Option<Vec<String>>,
    #[serde(default)]
    sakura_paragraphs: Option<Vec<String>>,
    #[serde(default)]
    youdao_paragraphs: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WenkuVolume {
    volume_id: String,
    #[serde(default)]
    gpt: u64,
    #[serde(default)]
    sakura: u64,
    #[serde(default)]
    youdao: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WenkuNovel {
    // Deliberately ignore bibliographic volumes/ASINs and other metadata.
    volume_jp: Vec<WenkuVolume>,
}

impl WenkuNovel {
    fn upload(&self, volume_id: &str) -> Option<&WenkuVolume> {
        self.volume_jp.iter().find(|v| v.volume_id == volume_id)
    }
}

fn preferred(original: &str, translated: Option<&str>) -> String {
    translated
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(original)
        .to_string()
}

// Encode each upstream ID component, including Wenku volumeId filenames.
// Preserve exact spelling; reject malformed escapes rather than aliasing IDs.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn decode(s: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let b = s.as_bytes()[i];
        if b == b'%' {
            bytes.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            bytes.push(b);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

#[derive(Debug, PartialEq)]
enum Id {
    Web(String, String),
    Volume(String, String),
}

impl Id {
    fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.splitn(3, ':');
        let kind = parts.next()?;
        let first = decode(parts.next()?)?;
        let second = decode(parts.next()?)?;
        match kind {
            "web" => Some(Self::Web(first, second)),
            "wenku-volume" => Some(Self::Volume(first, second)),
            _ => None,
        }
    }
}

fn web_id(p: &str, n: &str) -> String {
    format!("web:{}:{}", encode(p), encode(n))
}
fn volume_id(n: &str, volume: &str) -> String {
    format!("wenku-volume:{}:{}", encode(n), encode(volume))
}

impl WebNovel {
    fn book(&self, id: String) -> BookEntry {
        BookEntry {
            id,
            title: preferred(&self.title_jp, self.title_zh.as_deref()),
            authors: self.authors.iter().map(|a| a.name.clone()).collect(),
            description: Some(preferred(
                &self.introduction_jp,
                self.introduction_zh.as_deref(),
            )),
            cover_url: None,
            extra: Some(
                json!({"original_title": self.title_jp, "tags": self.keywords}).to_string(),
            ),
            series: None,
            volume_no: 0,
        }
    }

    fn chapters(&self) -> impl Iterator<Item = &TocItem> {
        self.toc
            .iter()
            .filter(|c| c.chapter_id.as_ref().is_some_and(|id| !id.is_empty()))
    }
}

fn selected_paragraphs<'a>(chapter: &'a WebChapter, settings: &Settings) -> Option<&'a [String]> {
    if settings.translation == 4 {
        return Some(&chapter.paragraphs);
    }
    for t in settings.translators() {
        let candidate = match *t {
            "gpt" => &chapter.gpt_paragraphs,
            "sakura" => &chapter.sakura_paragraphs,
            _ => &chapter.youdao_paragraphs,
        };
        if let Some(lines) = candidate
            && lines.len() == chapter.paragraphs.len()
            && (lines.is_empty() || lines.iter().any(|s| !s.trim().is_empty()))
        {
            return Some(lines);
        }
    }
    // Automatic mode has an explicit Japanese fallback. Fixed translators
    // return unavailable, never cache a "translation missing" banner.
    (settings.translation == 0).then_some(chapter.paragraphs.as_slice())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn paragraphs_html(
    original: &[String],
    selected: &[String],
    mut image: impl FnMut(&str) -> Option<String>,
) -> String {
    original
        .iter()
        .zip(selected)
        .map(|(jp, text)| {
            if let Some(url) = jp.strip_prefix("<图片>") {
                image(url.trim())
                    .map(|src| format!("<figure><img src=\"{}\"/></figure>", escape(&src)))
                    .unwrap_or_default()
            } else {
                format!("<p>{}</p>", escape(text).replace('\n', "<br/>"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn stored_image(settings: &Settings, raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let key = format!("image:v1:{url}");
    if let Some(reference) = mandara::plugin::state::get(&key)
        .ok()
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        return Some(reference);
    }
    // Credentials are for Novelia's API, never for illustration hosts.
    if let Some(bytes) = settings.fetch(&url, false) {
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some("image/jpeg")
        } else if bytes.starts_with(b"GIF8") {
            Some("image/gif")
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            Some("image/webp")
        } else {
            None
        };
        if let Some(mime) = mime {
            let id = mandara::plugin::store::store_image(&bytes, mime);
            let reference = format!("/api/images/{id}");
            // Never cache download failures or remote-URL fallbacks.
            if mandara::plugin::state::set(&key, reference.as_bytes()).is_err() {
                log("Novelia image cache write failed");
            }
            return Some(reference);
        }
    }
    Some(url.to_string())
}

fn file_url(settings: &Settings, novel: &str, volume: &WenkuVolume) -> Option<Url> {
    let translators: Vec<_> = settings
        .translators()
        .iter()
        .filter(|t| match **t {
            "gpt" => volume.gpt > 0,
            "sakura" => volume.sakura > 0,
            _ => volume.youdao > 0,
        })
        .copied()
        .collect();
    if translators.is_empty() {
        return None;
    }
    let mut url = settings.url(&["api", "wenku", novel, "file", &volume.volume_id])?;
    let mut params = url.query_pairs_mut();
    params
        .append_pair("mode", "zh")
        .append_pair("translationsMode", "priority")
        .append_pair("filename", &volume.volume_id);
    for translator in translators {
        params.append_pair("translations", translator);
    }
    drop(params);
    Some(url)
}

// The upstream provides page counts, not exact totals. Slice across page
// boundaries for arbitrary host offsets; use a page-count estimate until the
// last page is fetched, then report its exact item count.
fn search_pages<T>(
    offset: u32,
    limit: u32,
    mut fetch: impl FnMut(u32) -> Option<Page<T>>,
) -> Option<(u64, Vec<T>)> {
    let mut page = offset / PAGE_SIZE;
    let mut skip = (offset % PAGE_SIZE) as usize;
    let mut items = Vec::new();
    let mut total = 0;
    // A capped result window intersects at most two upstream pages. Bound
    // requests even if an inconsistent response contains empty middle pages.
    while items.len() < limit.min(PAGE_SIZE) as usize && page <= offset / PAGE_SIZE + 1 {
        let result = fetch(page)?;
        total = result.page_number.saturating_mul(u64::from(PAGE_SIZE));
        if u64::from(page) >= result.page_number {
            break;
        }
        if u64::from(page) + 1 == result.page_number {
            total = u64::from(page) * u64::from(PAGE_SIZE) + result.items.len() as u64;
        }
        let take = limit.min(PAGE_SIZE) as usize - items.len();
        items.extend(result.items.into_iter().skip(skip).take(take));
        if u64::from(page) + 1 >= result.page_number {
            break;
        }
        page += 1;
        skip = 0;
    }
    Some((total, items))
}

struct NoveliaPlugin;
impl Guest for NoveliaPlugin {
    fn name() -> String {
        "Novelia（轻小说机翻）".into()
    }
    fn capabilities() -> Vec<String> {
        ["search", "lookup", "content", "book-file"]
            .map(String::from)
            .to_vec()
    }
    fn source_info() -> SourceInfo {
        SourceInfo {
            kind: "search".into(),
            id_kind: "opaque".into(),
            id_hint: Some(
                "网络小说：web:来源:书号；文库仅正文：wenku-volume:系列书号:编码后的 volumeId"
                    .into(),
            ),
            search_hint: Some("搜索 Novelia 书名 / 作者".into()),
        }
    }
    fn config_schema() -> Vec<ConfigField> {
        use mandara::plugin::types::ConfigKind;
        vec![
            ConfigField {
                key: "base-url".into(),
                label: "站点地址".into(),
                kind: ConfigKind::Text,
                default: Some(json!(BASE).to_string()),
                required: true,
                hint: Some("Novelia 站点根地址（不含 /api）".into()),
            },
            ConfigField {
                key: "providers".into(),
                label: "网络小说来源".into(),
                kind: ConfigKind::ListOfString,
                default: Some(json!(PROVIDERS).to_string()),
                required: false,
                hint: Some("仅用于网络小说搜索；留空列表不搜索任何来源".into()),
            },
            ConfigField {
                key: "translation".into(),
                label: "正文译文".into(),
                kind: ConfigKind::EnumOptions(vec![
                    "自动：GPT → Sakura → 有道 → 日文".into(),
                    "仅 GPT".into(),
                    "仅 Sakura".into(),
                    "仅有道".into(),
                    "日文原文（仅网络小说）".into(),
                ]),
                default: Some("0".into()),
                required: true,
                hint: Some(
                    "固定译者缺失时不导入该章节；文库仅下载已有译文。修改后需重新物化正文".into(),
                ),
            },
            ConfigField {
                key: "username".into(),
                label: "账号（邮箱或用户名）".into(),
                kind: ConfigKind::Text,
                default: Some(json!("").to_string()),
                required: false,
                hint: Some("网络小说搜索需要登录；账号和密码均留空时匿名访问".into()),
            },
            ConfigField {
                key: "password".into(),
                label: "密码".into(),
                kind: ConfigKind::Text,
                default: Some(json!("").to_string()),
                required: false,
                hint: Some(
                    "与账号同时填写；凭据保存在实例配置中，请保护管理员权限和数据库备份".into(),
                ),
            },
            ConfigField {
                key: "auth-url".into(),
                label: "认证服务地址".into(),
                kind: ConfigKind::Text,
                default: Some(json!(AUTH_BASE).to_string()),
                required: true,
                hint: Some("官方统一认证服务；仅填写可信地址，不含 /api；建议使用 HTTPS".into()),
            },
        ]
    }
    fn declare(_config: Vec<ConfigValue>) -> Option<Vec<DeclaredBook>> {
        None
    }
    fn search_books(
        config: Vec<ConfigValue>,
        query: String,
        offset: u32,
        limit: u32,
    ) -> SearchResult {
        let settings = Settings::from_values(&config);
        let empty = || SearchResult {
            total: 0,
            items: vec![],
        };
        if limit == 0 {
            return empty();
        }
        let result = search_pages::<WebOutline>(offset, limit, |page| {
            let mut url = settings.url(&["api", "novel"])?;
            url.query_pairs_mut()
                .append_pair("page", &page.to_string())
                .append_pair("pageSize", &PAGE_SIZE.to_string())
                .append_pair("query", &query)
                .append_pair("provider", &settings.providers.join(","))
                .append_pair("type", "0")
                .append_pair("level", "0")
                .append_pair("translate", "0")
                .append_pair("sort", "0");
            settings.get(&url)
        });
        result
            .map(|(total, items)| SearchResult {
                total,
                items: items
                    .into_iter()
                    .map(|n| {
                        CatalogEntry::Single(BookEntry {
                            id: web_id(&n.provider_id, &n.novel_id),
                            title: preferred(&n.title_jp, n.title_zh.as_deref()),
                            authors: vec![],
                            description: None,
                            cover_url: None,
                            extra: Some(json!({"original_title": n.title_jp}).to_string()),
                            series: None,
                            volume_no: 0,
                        })
                    })
                    .collect(),
            })
            .unwrap_or_else(empty)
    }
    fn get_series(_config: Vec<ConfigValue>, _series_id: String) -> Option<SeriesResult> {
        None
    }
    fn get_book(config: Vec<ConfigValue>, book_id: String) -> Option<BookEntry> {
        let Id::Web(p, n) = Id::parse(&book_id)? else {
            return None;
        };
        Some(
            Settings::from_values(&config)
                .web(&p, &n)?
                .book(web_id(&p, &n)),
        )
    }
    fn get_content(config: Vec<ConfigValue>, content_id: String) -> Option<DeclaredContent> {
        let settings = Settings::from_values(&config);
        match Id::parse(&content_id)? {
            Id::Web(p, n) => {
                let novel = settings.web(&p, &n)?;
                Some(DeclaredContent {
                    id: web_id(&p, &n),
                    mode: ContentMode::Chapters,
                    chapters: novel
                        .chapters()
                        .map(|c| DeclaredChapter {
                            title: preferred(&c.title_jp, c.title_zh.as_deref()),
                            format: "html".into(),
                            content: None,
                        })
                        .collect(),
                })
            }
            Id::Volume(n, v) => {
                let novel = settings.wenku(&n)?;
                let volume = novel.upload(&v)?;
                file_url(&settings, &n, volume)?;
                Some(DeclaredContent {
                    id: volume_id(&n, &v),
                    mode: ContentMode::File,
                    chapters: vec![],
                })
            }
        }
    }
    fn get_chapter(config: Vec<ConfigValue>, content_id: String, index: u32) -> Option<Chapter> {
        let Id::Web(p, n) = Id::parse(&content_id)? else {
            return None;
        };
        let settings = Settings::from_values(&config);
        let novel = settings.web(&p, &n)?;
        let chapter_id = novel
            .chapters()
            .nth(index as usize)?
            .chapter_id
            .as_deref()?;
        let chapter: WebChapter =
            settings.get(&settings.url(&["api", "novel", &p, &n, "chapter", chapter_id])?)?;
        let Some(selected) = selected_paragraphs(&chapter, &settings) else {
            log("Novelia selected translation is unavailable");
            return None;
        };
        Some(Chapter {
            title: preferred(&chapter.title_jp, chapter.title_zh.as_deref()),
            content: paragraphs_html(&chapter.paragraphs, selected, |url| {
                stored_image(&settings, url)
            }),
            format: "html".into(),
        })
    }
    fn get_book_file(config: Vec<ConfigValue>, content_id: String) -> Option<BookFile> {
        let Id::Volume(n, v) = Id::parse(&content_id)? else {
            return None;
        };
        let settings = Settings::from_values(&config);
        let novel = settings.wenku(&n)?;
        let volume = novel.upload(&v)?;
        let bytes = settings.fetch(&file_url(&settings, &n, volume)?, true)?;
        let filename = &volume.volume_id;
        let mime = if filename.ends_with(".epub") && bytes.starts_with(b"PK\x03\x04") {
            "application/epub+zip"
        } else if filename.ends_with(".txt")
            && std::str::from_utf8(&bytes).ok().is_some_and(|s| {
                let start = s
                    .trim_start_matches('\u{feff}')
                    .trim_start()
                    .to_ascii_lowercase();
                !start.starts_with("<!doctype html")
                    && !start.starts_with("<html")
                    && !start.starts_with("{\"error\"")
                    && !s.is_empty()
            })
        {
            "text/plain; charset=utf-8"
        } else {
            log("Novelia returned an invalid book download");
            return None;
        };
        Some(BookFile {
            filename: filename.clone(),
            mime: mime.into(),
            bytes,
        })
    }
    fn identify_upload(
        _config: Vec<ConfigValue>,
        _filename: String,
        _file_hash: String,
    ) -> Option<BookEntry> {
        None
    }
}

export!(NoveliaPlugin);

#[cfg(test)]
mod tests;
