//! Novelia's public JSON API, adapted to Mandara's stateless WIT world.
//! Web novels use chapter mode; Wenku editions use upstream ASINs for identity.
//! Upload filenames are download locators, resolved separately from identity.

use serde::Deserialize;
use serde_json::json;
use unicode_normalization::UnicodeNormalization;
use url::Url;

wit_bindgen::generate!({
    world: "mandara-plugin",
    path: "../../crates/mandara-plugin/wit",
});

const BASE: &str = "https://n.novelia.cc";
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
    wenku: bool,
    providers: Vec<String>,
    translation: u32,
    token: String,
    bindings: Option<Vec<FileBinding>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct FileBinding {
    novel_id: String,
    asin: String,
    filename: String,
}

impl Settings {
    fn load() -> Self {
        let values = mandara::plugin::config::configure();
        let text = |i, default: &str| match values.get(i) {
            Some(ConfigValue::Text(s)) if !s.trim().is_empty() => s.trim().to_string(),
            _ => default.into(),
        };
        let choice = |i| match values.get(i) {
            Some(ConfigValue::EnumIndex(n)) => *n,
            _ => 0,
        };
        Self {
            base: text(0, BASE),
            wenku: choice(1) == 1,
            providers: match values.get(2) {
                Some(ConfigValue::StringList(p)) => p.clone(),
                _ => PROVIDERS.iter().map(|p| (*p).into()).collect(),
            },
            translation: choice(3),
            token: text(4, ""),
            // Invalid explicit mappings fail closed rather than guessing a file.
            bindings: serde_json::from_str(&text(5, "[]")).ok(),
        }
    }

    fn url(&self, segments: &[&str]) -> Option<Url> {
        let mut url = Url::parse(&self.base).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .ok()?
            .pop_if_empty()
            .extend(segments);
        Some(url)
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
        let mut headers = vec![Header {
            name: "User-Agent".into(),
            value: "snylonue/mandara Novelia plugin".into(),
        }];
        if authenticated && !self.token.is_empty() {
            headers.push(Header {
                name: "Authorization".into(),
                value: format!("Bearer {}", self.token),
            });
        }
        // Do not retry authorization failures or log response bodies/tokens.
        for attempt in 0..2 {
            match fetch(&Request {
                method: "GET".into(),
                url: url.to_string(),
                headers: headers.clone(),
                body: None,
                timeout_ms: Some(30_000),
            }) {
                Ok(r) if r.status == 200 => return Some(r.body),
                Ok(r) if r.status >= 500 && attempt == 0 => continue,
                Ok(r) => {
                    log(&format!("Novelia HTTP {} at {}", r.status, url.path()));
                    return None;
                }
                Err(
                    mandara::plugin::http::FetchError::Timeout(_)
                    | mandara::plugin::http::FetchError::Transport(_),
                ) if attempt == 0 => continue,
                Err(_) => {
                    log("Novelia HTTP transport/policy failure");
                    return None;
                }
            }
        }
        None
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
struct WenkuOutline {
    id: String,
    title: String,
    title_zh: String,
    cover: Option<String>,
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
struct WenkuEdition {
    asin: String,
    title: String,
    title_zh: Option<String>,
    cover: Option<String>,
    publisher: Option<String>,
    imprint: Option<String>,
    publish_at: Option<i64>,
}

fn valid_asin(asin: &str) -> bool {
    asin.len() == 10
        && asin
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WenkuNovel {
    title: String,
    title_zh: String,
    cover: Option<String>,
    authors: Vec<String>,
    artists: Vec<String>,
    introduction: String,
    #[serde(default)]
    keywords: Vec<String>,
    publisher: Option<String>,
    imprint: Option<String>,
    volumes: Vec<WenkuEdition>,
    volume_jp: Vec<WenkuVolume>,
}

fn preferred(original: &str, translated: Option<&str>) -> String {
    translated
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(original)
        .to_string()
}

// Opaque IDs encode upstream identifiers, never upload filenames. Reject
// malformed escapes and old filename IDs rather than silently rebinding them.
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
    Wenku(String),
    Volume(String, String),
}

impl Id {
    fn parse(raw: &str) -> Option<Self> {
        let parts: Vec<_> = raw.split(':').collect();
        let component = |s: &str| {
            decode(s).filter(|s| {
                !s.is_empty() && s != "." && s != ".." && !s.chars().any(char::is_control)
            })
        };
        match parts.as_slice() {
            ["web", p, n] => Some(Self::Web(component(p)?, component(n)?)),
            ["wenku", n] => Some(Self::Wenku(component(n)?)),
            ["wenku-asin", n, asin] if valid_asin(asin) => {
                Some(Self::Volume(component(n)?, (*asin).into()))
            }
            _ => None,
        }
    }
}

fn web_id(p: &str, n: &str) -> String {
    format!("web:{}:{}", encode(p), encode(n))
}
fn volume_id(n: &str, asin: &str) -> String {
    format!("wenku-asin:{}:{asin}", encode(n))
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

impl WenkuNovel {
    fn series(&self, id: &str) -> SeriesEntry {
        SeriesEntry {
            id: format!("wenku:{}", encode(id)),
            title: preferred(&self.title, Some(&self.title_zh)),
            authors: self.authors.clone(),
            description: Some(self.introduction.clone()),
            cover_url: self.cover.clone(),
            extra: Some(json!({"original_title": self.title, "tags": self.keywords}).to_string()),
        }
    }

    fn edition(&self, asin: &str) -> Option<&WenkuEdition> {
        if !valid_asin(asin) {
            return None;
        }
        let mut matches = self.volumes.iter().filter(|v| v.asin == asin);
        let edition = matches.next()?;
        matches.next().is_none().then_some(edition)
    }

    fn books(&self, id: &str) -> Vec<BookEntry> {
        self.volumes
            .iter()
            .enumerate()
            .filter(|(_, v)| self.edition(&v.asin).is_some())
            .map(|(i, v)| BookEntry {
                id: volume_id(id, &v.asin),
                title: preferred(&v.title, v.title_zh.as_deref()),
                authors: self.authors.clone(),
                description: Some(self.introduction.clone()),
                cover_url: v.cover.clone().or_else(|| self.cover.clone()),
                extra: Some(
                    json!({
                        "original_title": v.title, "illustrators": self.artists,
                        "publisher": v.publisher.as_ref().or(self.publisher.as_ref()),
                        "novelia_asin": v.asin, "novelia_publish_at": v.publish_at,
                    })
                    .to_string(),
                ),
                series: Some(self.series(id)),
                volume_no: i as u32 + 1,
            })
            .collect()
    }

    fn title_key(&self, title: &str, edition: &WenkuEdition) -> String {
        let mut normalized: String = title.nfkc().collect();
        // Strip only explicitly known publisher/imprint suffixes, not arbitrary
        // subtitles, volume numbers or fuzzy prefixes.
        for label in [
            &edition.imprint,
            &self.imprint,
            &edition.publisher,
            &self.publisher,
        ]
        .into_iter()
        .flatten()
        {
            let suffix = format!("({})", label.nfkc().collect::<String>());
            if let Some(prefix) = normalized.trim_end().strip_suffix(&suffix) {
                normalized = prefix.trim_end().to_string();
            }
        }
        normalized.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn upload_key(&self, filename: &str, edition: &WenkuEdition) -> Option<String> {
        let stem = filename
            .strip_suffix(".epub")
            .or_else(|| filename.strip_suffix(".txt"))?;
        let mut stem: String = stem.nfkc().collect();
        for author in &self.authors {
            let suffix = format!(" - {}", author.nfkc().collect::<String>());
            if let Some(prefix) = stem.strip_suffix(&suffix) {
                stem = prefix.to_string();
            }
        }
        Some(self.title_key(&stem, edition))
    }

    fn upload_matches(&self, upload: &WenkuVolume, edition: &WenkuEdition) -> bool {
        let Some(key) = self.upload_key(&upload.volume_id, edition) else {
            return false;
        };
        !key.is_empty()
            && std::iter::once(edition.title.as_str())
                .chain(edition.title_zh.as_deref())
                .any(|title| !title.is_empty() && self.title_key(title, edition) == key)
    }

    fn upload<'a>(
        &'a self,
        novel: &str,
        asin: &str,
        settings: &Settings,
    ) -> Option<&'a WenkuVolume> {
        let edition = self.edition(asin)?;
        let bindings = settings.bindings.as_ref()?;
        let mut bindings = bindings
            .iter()
            .filter(|b| b.novel_id == novel && b.asin == asin);
        if let Some(binding) = bindings.next() {
            if bindings.next().is_some() {
                return None;
            }
            let mut uploads = self
                .volume_jp
                .iter()
                .filter(|v| v.volume_id == binding.filename);
            let upload = uploads.next()?;
            return uploads.next().is_none().then_some(upload);
        }
        let mut uploads = self
            .volume_jp
            .iter()
            .filter(|v| self.upload_matches(v, edition));
        let upload = uploads.next()?;
        if uploads.next().is_some() {
            return None;
        }
        // One upload must identify exactly one edition in the catalog as well.
        (self
            .volumes
            .iter()
            .filter(|v| self.upload_matches(upload, v))
            .count()
            == 1)
            .then_some(upload)
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
            return Some(format!("/api/images/{id}"));
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
            id_hint: Some("网络小说：web:来源:书号；文库系列：wenku:书号".into()),
            search_hint: Some("搜索 Novelia 书名 / 作者".into()),
        }
    }
    fn config_schema() -> Vec<ConfigField> {
        use mandara::plugin::config::ConfigKind;
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
                key: "catalog".into(),
                label: "搜索目录".into(),
                kind: ConfigKind::EnumOptions(vec!["网络小说".into(), "文库小说".into()]),
                default: Some("0".into()),
                required: true,
                hint: Some("可注册两个实例，分别搜索网络小说和文库小说".into()),
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
                key: "access-token".into(),
                label: "访问令牌（可选）".into(),
                kind: ConfigKind::Text,
                default: Some(json!("").to_string()),
                required: false,
                hint: Some(
                    "网络小说搜索需要登录；可填写登录会话的 Bearer 令牌，不要填写密码".into(),
                ),
            },
            ConfigField {
                key: "volume-files".into(), label: "文库下载文件映射（可选）".into(),
                kind: ConfigKind::Text, default: Some(json!("[]").to_string()), required: false,
                hint: Some("JSON 数组：[{\"novel-id\":\"系列书号\",\"asin\":\"ASIN\",\"filename\":\"文件名.epub\"}]；仅在标题无法唯一匹配时使用".into()),
            },
        ]
    }
    fn declare() -> Option<Vec<DeclaredBook>> {
        None
    }
    fn search_books(query: String, offset: u32, limit: u32) -> SearchResult {
        let settings = Settings::load();
        let empty = || SearchResult {
            total: 0,
            items: vec![],
        };
        if limit == 0 {
            return empty();
        }
        if settings.wenku {
            let result = search_pages::<WenkuOutline>(offset, limit, |page| {
                let mut url = settings.url(&["api", "wenku"])?;
                url.query_pairs_mut()
                    .append_pair("page", &page.to_string())
                    .append_pair("pageSize", &PAGE_SIZE.to_string())
                    .append_pair("query", &query)
                    .append_pair("level", "0");
                settings.get(&url)
            });
            result
                .map(|(total, items)| SearchResult {
                    total,
                    items: items
                        .into_iter()
                        .map(|n| {
                            CatalogEntry::Series(SeriesEntry {
                                id: format!("wenku:{}", encode(&n.id)),
                                title: preferred(&n.title, Some(&n.title_zh)),
                                authors: vec![],
                                description: None,
                                cover_url: n.cover,
                                extra: Some(json!({"original_title": n.title}).to_string()),
                            })
                        })
                        .collect(),
                })
                .unwrap_or_else(empty)
        } else {
            if settings.providers.is_empty() {
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
    }
    fn get_series(series_id: String) -> Option<SeriesResult> {
        let Id::Wenku(n) = Id::parse(&series_id)? else {
            return None;
        };
        let novel = Settings::load().wenku(&n)?;
        Some(SeriesResult {
            series: novel.series(&n),
            books: novel.books(&n),
        })
    }
    fn get_book(book_id: String) -> Option<BookEntry> {
        let settings = Settings::load();
        match Id::parse(&book_id)? {
            Id::Web(p, n) => Some(settings.web(&p, &n)?.book(web_id(&p, &n))),
            Id::Volume(n, v) => settings
                .wenku(&n)?
                .books(&n)
                .into_iter()
                .find(|b| b.id == volume_id(&n, &v)),
            Id::Wenku(_) => None,
        }
    }
    fn get_content(content_id: String) -> Option<DeclaredContent> {
        let settings = Settings::load();
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
                let volume = novel.upload(&n, &v, &settings)?;
                file_url(&settings, &n, volume)?;
                Some(DeclaredContent {
                    id: volume_id(&n, &v),
                    mode: ContentMode::File,
                    chapters: vec![],
                })
            }
            Id::Wenku(_) => None,
        }
    }
    fn get_chapter(content_id: String, index: u32) -> Option<Chapter> {
        let Id::Web(p, n) = Id::parse(&content_id)? else {
            return None;
        };
        let settings = Settings::load();
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
    fn get_book_file(content_id: String) -> Option<BookFile> {
        let Id::Volume(n, v) = Id::parse(&content_id)? else {
            return None;
        };
        let settings = Settings::load();
        let novel = settings.wenku(&n)?;
        let volume = novel.upload(&n, &v, &settings)?;
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
    fn identify_upload(_filename: String, _file_hash: String) -> Option<BookEntry> {
        None
    }
}

export!(NoveliaPlugin);

#[cfg(test)]
mod tests;
