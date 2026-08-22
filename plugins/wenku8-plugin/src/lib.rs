//! Guest side of the `bookshelf:plugin/bookshelf-plugin` world (v3) for
//! **wenku8.net** (轻小说文库), fetched through the host's `http.fetch`
//! import — the only network a plugin gets.
//!
//! Site notes (researched, verified against live pages):
//! - Pages are **GBK**-encoded (decoded with `encoding_rs`).
//! - Book page  `{base}/book/{id}.htm` — metadata (login-free).
//! - TOC        `{base}/modules/article/reader.php?aid={id}` — `<td
//!   class="vcss">` volume rows + `<td class="ccss">` chapter rows
//!   carrying `...&cid=N` links. **Needs a same-host `Referer`** (403
//!   without).
//! - Chapter    `...reader.php?aid={id}&cid={cid}` — `#content` holds
//!   prose (`<br/>`-separated, `&nbsp;` indents) or, in the **插图**
//!   chapters, `<div class="divimage"><a href="IMG"><img class=
//!   "imagecontent" src="IMG"></a></div>` plates.
//! - wenku8 puts an `插图` (color plates) chapter at the *end of every
//!   volume* (`<后记>` then `插图`). The plugin keeps them **in that
//!   original position** by default (`end`), or moves them to the
//!   volume front like the physical book / [linovelib2epub's
//!   Wenku8Spider] (`front`), or drops them (`skip`) — configurable.
//! - Search, listings and the txt/epub downloads are all **login-walled**
//!   (guests get redirects to login.php / Cloudflare); only `book/` and
//!   `reader.php` work anonymously. So this plugin does **not** declare
//!   `search` — books are looked up by their numeric id (the number in
//!   the wenku8 URL, e.g. `3617` for `/book/3617.htm`) via the library
//!   source browser's manual-id entry.
//!
//! Capabilities: `["lookup", "content"]` (metadata + chapter mode).
//! `get-chapter` re-fetches the (small) TOC to map the host's index to a
//! concrete `cid` — the guest is stateless, so the map can't be cached;
//! the host's DB is the cache, so this happens once per chapter at most.
//!
//! Deploy (host side needs the allow list):
//! ```sh
//! BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS=www.wenku8.net ./scripts/build-plugins.sh wenku8
//! cp plugins-built/wenku8.wasm data/plugins/
//! # register: POST /api/plugins/instances {"id": "wenku8", "wasm_file": "wenku8.wasm"}
//! ```

wit_bindgen::generate!({
    world: "bookshelf-plugin",
    path: "../../crates/bookshelf-plugin/wit",
});

const DEFAULT_BASE_URL: &str = "https://www.wenku8.net";
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// Config schema field order — the host injects values in this order.
const FIELDS: [&str; 3] = ["base-url", "referer", "illustration-placement"];

/// fn values() reads the injected config once per call.
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

/// 插图 placement policy (index into the config enum options).
#[derive(PartialEq, Clone, Copy)]
enum Placement {
    End,   // keep wenku8's own order (插图 at the end of every volume) — default
    Front, // move 插图 chapters to the front of their volume (like the print book)
    Skip,  // do not include 插图 chapters at all
}

fn placement() -> Placement {
    let idx = FIELDS
        .iter()
        .position(|k| *k == "illustration-placement")
        .unwrap();
    match values().get(idx) {
        Some(ConfigValue::EnumIndex(1)) => Placement::Front,
        Some(ConfigValue::EnumIndex(2)) => Placement::Skip,
        _ => Placement::End,
    }
}

fn log_error(context: &str, url: &str, err: &str) {
    bookshelf::plugin::store::log(&format!("{context} `{url}` failed: {err}"));
}

// ---------------------------------------------------------------------------
// HTTP + decoding
// ---------------------------------------------------------------------------

/// One remote `http.fetch` with a browser UA and an optional same-host
/// `Referer` (wenku8's reader.php returns 403 without one).
///
/// Transient problems (transport errors, 5xx, anti-bot 403 spikes) are
/// retried once — see the retry guidance in docs/plugin-http-api-design.md
/// §4.3; the call budget covers two attempts.
fn http_get(url: &str, referer: &str) -> Result<(u16, Vec<u8>), String> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut headers = vec![bookshelf::plugin::http::Header {
            name: "User-Agent".into(),
            value: DEFAULT_UA.into(),
        }];
        if !referer.is_empty() {
            headers.push(bookshelf::plugin::http::Header {
                name: "Referer".into(),
                value: referer.into(),
            });
        }
        let request = bookshelf::plugin::http::Request {
            method: "GET".into(),
            url: url.into(),
            headers,
            body: None,
            timeout_ms: Some(15_000),
        };
        match bookshelf::plugin::http::fetch(&request) {
            // Retry once on transient application states (5xx, anti-bot
            // 403) and on transport-level errors; 404/4xx are final.
            Ok(resp) if attempt == 1 && (resp.status == 403 || resp.status >= 500) => continue,
            Ok(resp) => return Ok((resp.status, resp.body)),
            Err(_) if attempt == 1 => continue,
            Err(e) => return Err(format!("{e:?}")),
        }
    }
}

/// Decode a GBK/GB18030 page (all wenku8 pages are GBK).
fn decode_page(bytes: &[u8]) -> String {
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    text.into_owned()
}

/// Fetch `url`, decode GBK; `Ok(html)` only on HTTP 200.
fn fetch_page(context: &str, url: &str, referer: &str) -> Option<String> {
    match http_get(url, referer) {
        Ok((200, body)) => Some(decode_page(&body)),
        Ok((status, _)) => {
            if status != 404 {
                log_error(context, url, &format!("status {status}"));
            }
            None
        }
        Err(e) => {
            log_error(context, url, &e);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Small HTML helpers (wenku8's markup is regular XHTML-ish; hand-rolled is
// enough and keeps the wasm small)
// ---------------------------------------------------------------------------

/// Byte index just after `needle`, scanning from `from` (ASCII markers —
/// indexes stay on char boundaries).
fn find_after(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let i = hay[from..].find(needle)? + from;
    Some(i + needle.len())
}

/// Inner HTML of `<div id="content">…</div>`, depth-aware (the content
/// div nests `<div class="divimage">` plates).
fn content_div(html: &str) -> Option<String> {
    let after_tag = find_after(html, "<div id=\"content\"", 0)?;
    let start = find_after(html, ">", after_tag)?;
    let bytes = html.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if html[i..].starts_with("<div") || html[i..].starts_with("<DIV") {
                depth += 1;
                i += html[i..]
                    .find('>')
                    .map(|g| g + 1)
                    .unwrap_or(bytes.len() - i);
                continue;
            }
            if html[i..].starts_with("</div") || html[i..].starts_with("</DIV") {
                depth -= 1;
                if depth == 0 {
                    return Some(html[start..i].to_string());
                }
                i += html[i..]
                    .find('>')
                    .map(|g| g + 1)
                    .unwrap_or(bytes.len() - i);
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Drop the `<ul id="contentdp">…</ul>` ad banner wenku8 puts at the top
/// of every chapter body.
fn strip_contentdp(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    loop {
        let Some(p) = rest.find("contentdp") else {
            out.push_str(rest);
            break;
        };
        let tag_start = rest[..p].rfind("<ul").unwrap_or(p);
        let Some(after_open) = rest[tag_start..].find('>') else {
            out.push_str(rest);
            break;
        };
        let after_open = tag_start + after_open + 1;
        let Some(close) = rest[after_open..].find("</ul>") else {
            out.push_str(rest);
            break;
        };
        let close = after_open + close + "</ul>".len();
        out.push_str(&rest[..tag_start]);
        rest = &rest[close..];
    }
    out
}

/// A small named-entity table covering what wenku8 pages actually use.
/// (Numeric `&#N;`/`&#xH;` references are handled separately.)
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("nbsp;", ' '),
    ("amp;", '&'),
    ("lt;", '<'),
    ("gt;", '>'),
    ("quot;", '"'),
    ("apos;", '\''),
    ("hellip;", '…'),
    ("mdash;", '—'),
    ("ndash;", '–'),
    ("lsquo;", '‘'),
    ("rsquo;", '’'),
    ("ldquo;", '“'),
    ("rdquo;", '”'),
    ("middot;", '·'),
    ("bull;", '•'),
    ("times;", '×'),
    ("copy;", '©'),
    ("deg;", '°'),
];

/// Decode `&nbsp;`, `&#8226;`, `&#x2026;` … into characters.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp + 1..];
        let mut consumed = 0usize;
        let mut pushed: Option<char> = None;
        // numeric decimal / hex
        if let Some(rest_after) = tail.strip_prefix('#') {
            let (radix, digits) = if let Some(hex) = rest_after.strip_prefix('x') {
                (16u32, hex)
            } else if let Some(hex) = rest_after.strip_prefix('X') {
                (16u32, hex)
            } else {
                (10u32, rest_after)
            };
            if let Some(semi) = digits.find(';')
                && let Ok(code) = u32::from_str_radix(&digits[..semi], radix)
                && let Some(ch) = char::from_u32(code)
            {
                pushed = Some(ch);
                consumed = 1 + 1 + semi + 1; // '&' '#' [digits] ';'
            }
        }
        if pushed.is_none() {
            for (name, ch) in NAMED_ENTITIES {
                if let Some(t) = tail.strip_prefix(name) {
                    let _ = t;
                    pushed = Some(*ch);
                    consumed = 1 + name.len(); // '&' + "name;"
                    break;
                }
            }
        }
        match pushed {
            Some(ch) => {
                out.push(ch);
                rest = &tail[consumed - 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Turn inner HTML into plain text: block-ish tags become newlines, the
/// rest is dropped, entities decoded, whitespace collapsed per line.
fn html_to_text(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tail = &rest[lt + 1..];
        let Some(gt) = tail.find('>') else {
            out.push_str(tail);
            break;
        };
        let name: String = tail[..gt]
            .trim()
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if matches!(
            name.as_str(),
            "br" | "p"
                | "div"
                | "td"
                | "tr"
                | "li"
                | "ul"
                | "ol"
                | "hr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
        ) && !out.ends_with('\n')
        {
            out.push('\n');
        }
        rest = &tail[gt + 1..];
    }
    out.push_str(rest);
    decode_entities(&out)
}

/// Final clean-up: trim lines, drop empty ones, join with `\n`.
fn tidy(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if lines.last().map(|l| !l.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            continue;
        }
        lines.push(line);
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// TOC + ordering
// ---------------------------------------------------------------------------

/// One chapter row of the TOC: its `cid` (needed to fetch the body) and
/// its title (what the reader shows).
struct ChapterRef {
    cid: u32,
    title: String,
}

/// A volume: its 1-based ordinal (for disambiguating repeated 插图
/// chapters in the flat TOC) + its chapter rows in wenku8's own order
/// (插图 last).
struct Volume {
    vol_no: usize,
    chapters: Vec<ChapterRef>,
}

/// One entry of the flat reading order.
struct OrderedChapter {
    cid: u32,
    title: String,
    illustration: bool,
}

/// Is this chapter an illustration chapter? wenku8 names these exactly
/// `插图` (some books: `插画`, `彩插`); tolerate a short suffix/prefix
/// (e.g. `第二卷 插图`).
fn is_illustration(title: &str) -> bool {
    let t: String = title.chars().filter(|c| !c.is_whitespace()).collect();
    ["插图", "插画", "彩插"]
        .iter()
        .any(|k| t == *k || (t.starts_with(k) && t.len() < k.len() + 6))
}

/// Parse the reader.php TOC: volume (`vcss`) + chapter (`ccss`) rows; a
/// row without a `cid` link is skipped (cannot be fetched).
fn parse_toc(html: &str) -> Option<Vec<Volume>> {
    let mut vols: Vec<Volume> = Vec::new();
    let mut i = 0usize;
    loop {
        let rel = &html[i..];
        let v = rel.find("<td class=\"vcss\"");
        let c = rel.find("<td class=\"ccss\"");
        let next = match (v, c) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let pos = i + next;
        if rel[next..].starts_with("<td class=\"vcss\"") {
            let after = find_after(html, ">", pos)?;
            let end = html[after..].find("</td>")? + after;
            // volume titles are not representable in the flat plugin TOC;
            // only the 1-based ordinal is kept (for 插图 disambiguation)
            vols.push(Volume {
                vol_no: vols.len() + 1,
                chapters: Vec::new(),
            });
            i = end;
        } else {
            let after = find_after(html, ">", pos)?;
            let (cid, title) = if let Some(a) = find_after(html, "href=\"", after) {
                let href_end = html[a..].find('"')? + a;
                let href = &html[a..href_end];
                let cid = href.rsplit("cid=").next()?.parse::<u32>().ok()?;
                let t_start = find_after(html, ">", href_end)?;
                let t_end = html[t_start..].find("</a>")? + t_start;
                let title = decode_entities(&html[t_start..t_end]).trim().to_string();
                (cid, title)
            } else {
                // unlinked row: nothing to fetch — skip it
                i = html[after..].find("</td>").map(|e| after + e)?;
                continue;
            };
            if let Some(last) = vols.last_mut() {
                last.chapters.push(ChapterRef { cid, title });
            }
            // advance past this row's `</td>`
            i = html[pos..].find("</td>").map(|e| pos + e).unwrap_or(after);
        }
    }
    if vols.is_empty() {
        return None;
    }
    Some(vols)
}

/// The flat reading order derived from the TOC under the configured 插图
/// policy: `(cid, title)` pairs, volumes in order. When a book has more
/// than one volume, each 插图 chapter's title is prefixed with its volume
/// number (`第2卷 插图`) — the host's plugin TOC is flat, so otherwise
/// repeated `插图` entries would be indistinguishable.
fn build_order(vols: &[Volume], placement: Placement) -> Vec<OrderedChapter> {
    let multi_volume = vols.len() > 1;
    let mut out = Vec::new();
    for vol in vols {
        let (mut ill, mut normal): (Vec<_>, Vec<_>) =
            vol.chapters.iter().partition(|c| is_illustration(&c.title));
        let mut push = |chs: &mut Vec<&ChapterRef>| {
            for c in chs.drain(..) {
                let illustration = is_illustration(&c.title);
                let title = if illustration && multi_volume {
                    format!("第{}卷 {}", vol.vol_no, c.title)
                } else {
                    c.title.clone()
                };
                out.push(OrderedChapter {
                    cid: c.cid,
                    title,
                    illustration,
                });
            }
        };
        match placement {
            Placement::Front => {
                push(&mut ill);
                push(&mut normal);
            }
            Placement::End => {
                push(&mut normal);
                push(&mut ill);
            }
            Placement::Skip => {
                push(&mut normal);
            }
        }
    }
    out
}

fn toc_url(base: &str, book: &str) -> String {
    format!("{base}/modules/article/reader.php?aid={book}")
}

fn chapter_url(base: &str, book: &str, cid: u32) -> String {
    format!("{base}/modules/article/reader.php?aid={book}&cid={cid}")
}

/// Referer to send with wenku8 requests: the configured override, or a
/// same-host page (wenku8 checks it on reader.php).
fn referer_for(base: &str, book: &str) -> String {
    let cfg = config_string("referer", "");
    if cfg.is_empty() {
        format!("{base}/book/{book}.htm")
    } else {
        cfg
    }
}

/// Fetch + parse the TOC of `book`, returning the flat reading order.
fn fetch_order(book: &str) -> Option<Vec<OrderedChapter>> {
    let base = config_string("base-url", DEFAULT_BASE_URL);
    let html = fetch_page("toc", &toc_url(&base, book), &referer_for(&base, book))?;
    let vols = parse_toc(&html)?;
    Some(build_order(&vols, placement()))
}

// ---------------------------------------------------------------------------
// Chapter body extraction
// ---------------------------------------------------------------------------

/// 插图 chapters: list the plate image URLs, one per line (plugin
/// chapters are stored as text; the reader shows them as a URL list).
fn extract_image_urls(inner: &str) -> String {
    let mut urls: Vec<String> = Vec::new();
    let mut i = 0usize;
    while let Some(p) = inner[i..].find("class=\"divimage\"") {
        let start = i + p;
        let end = inner[start..]
            .find("</div>")
            .map(|e| start + e)
            .unwrap_or(inner.len());
        let block = &inner[start..end];
        if let Some(s) = find_after(block, "src=\"", 0) {
            let e = block[s..].find('"').map(|x| s + x).unwrap_or(block.len());
            urls.push(block[s..e].to_string());
        } else if let Some(s) = find_after(block, "<a href=\"", 0) {
            let e = block[s..].find('"').map(|x| s + x).unwrap_or(block.len());
            urls.push(block[s..e].to_string());
        }
        i = end;
    }
    // fallback: any <img> in the content (some books use plain imgs)
    if urls.is_empty() {
        let mut j = 0usize;
        while let Some(s) = find_after(&inner[j..], "<img src=\"", 0) {
            let s = j + s;
            let e = inner[s..].find('"').map(|x| s + x).unwrap_or(inner.len());
            let url = &inner[s..e];
            if url.contains("http") {
                urls.push(url.to_string());
            }
            j = e;
        }
    }
    if urls.is_empty() {
        return String::new();
    }
    let mut out = format!("[插图] 共 {} 张\n", urls.len());
    for (n, u) in urls.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", n + 1, u));
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Book-info page parsing
// ---------------------------------------------------------------------------

fn title_from_head(html: &str) -> Option<String> {
    // `<title>A - 小说作者 - 轻小说文库</title>` → first segment
    let start = find_after(html, "<title>", 0)?;
    let end = html[start..].find('<')? + start;
    let t = html[start..end]
        .split(" - ")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// Text of a `小说作者：…` (etc.) info cell.
fn info_field(inner: &str, label: &str) -> String {
    let Some(s) = find_after(inner, label, 0) else {
        return String::new();
    };
    let end = inner[s..].find('<').unwrap_or(inner.len() - s) + s;
    decode_entities(inner[s..end].trim())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_cover(inner: &str) -> Option<String> {
    let mut i = 0usize;
    while let Some(s) = find_after(&inner[i..], "<img src=\"", 0) {
        let s = i + s;
        let e = inner[s..].find('"').map(|x| s + x).unwrap_or(inner.len());
        let url = &inner[s..e];
        if url.contains("wenku8") {
            return Some(url.replace("http://", "https://"));
        }
        i = e;
    }
    None
}

/// Book-info page → `BookEntry`. `None` when the page is not a book page
/// (404 / login redirect).
fn parse_book_page(book: &str, html: &str) -> Option<BookEntry> {
    let inner = content_div(html)?;
    let title = (|| {
        let s = find_after(&inner, "<b>", 0)?;
        let e = inner[s..].find("</b>")? + s;
        let t = decode_entities(&inner[s..e])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (!t.is_empty()).then_some(t)
    })()
    .or_else(|| title_from_head(html))?;

    let author = info_field(&inner, "小说作者：");
    let authors: Vec<String> = author
        .split(['、', '，', ',', '/'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let description = if let Some(s) = find_after(&inner, "内容简介：", 0) {
        let end = inner[s..].find("</td>").unwrap_or(inner.len() - s) + s;
        let text = tidy(&html_to_text(&inner[s..end]));
        (!text.is_empty()).then(|| {
            let bytes = text.as_bytes();
            if bytes.len() > 4000 {
                text[..bytes.len().min(4000)].to_string()
            } else {
                text
            }
        })
    } else {
        None
    };

    Some(BookEntry {
        id: book.to_string(),
        title,
        authors,
        description,
        cover_url: extract_cover(&inner),
        content_source: None,
        content_id: None,
    })
}

/// Accept only plain numeric wenku8 book ids ("3617").
fn normalize_book_id(id: &str) -> Option<String> {
    let t = id.trim();
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 9 {
        Some(t.to_string())
    } else {
        bookshelf::plugin::store::log(&format!(
            "invalid wenku8 book id `{id}` (expected the numeric id from the book URL, e.g. 3617)"
        ));
        None
    }
}

// ---------------------------------------------------------------------------
// World exports
// ---------------------------------------------------------------------------

struct Wenku8Plugin;

impl Guest for Wenku8Plugin {
    fn name() -> String {
        "wenku8".into()
    }

    fn config_schema() -> Vec<ConfigField> {
        vec![
            ConfigField {
                key: "base-url".into(),
                label: "站点地址".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: Some(DEFAULT_BASE_URL.into()),
                required: true,
                hint: Some("轻小说文库根地址；镜像可改（如 https://www.wenku8.cc）".into()),
            },
            ConfigField {
                key: "referer".into(),
                label: "Referer 覆盖".into(),
                kind: bookshelf::plugin::config::ConfigKind::Text,
                default: None,
                required: false,
                hint: Some("留空 = 自动用同站点书页当 Referer（wenku8 校验，缺失会 403）".into()),
            },
            ConfigField {
                key: "illustration-placement".into(),
                label: "插画章节位置".into(),
                kind: bookshelf::plugin::config::ConfigKind::EnumOptions(vec![
                    "保留在卷尾（原文位置，与 wenku8 目录一致）".into(),
                    "移至卷首（如实体书彩插）".into(),
                    "跳过不收".into(),
                ]),
                default: Some("0".into()),
                required: false,
                hint: Some(
                    "wenku8 把每卷的插画章放在卷尾：默认保留原文位置；可选移到卷首或跳过".into(),
                ),
            },
        ]
    }

    fn capabilities() -> Vec<String> {
        // lookup + chapter-mode content. No `search`: wenku8's search and
        // listings are login-walled; books are materialized by id.
        vec!["lookup".into(), "content".into()]
    }

    fn declare() -> Option<Vec<DeclaredBook>> {
        None
    }

    fn search_books(_query: String, _offset: u32, _limit: u32) -> SearchResult {
        bookshelf::plugin::store::log(
            "wenku8 search is login-walled; materialize books by id instead (source browser manual-id entry)",
        );
        SearchResult {
            total: 0,
            items: Vec::new(),
        }
    }

    fn get_book(book_id: String) -> Option<BookEntry> {
        let book = normalize_book_id(&book_id)?;
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let url = format!("{base}/book/{book}.htm");
        let html = fetch_page("get-book", &url, &format!("{base}/"))?;
        parse_book_page(&book, &html)
    }

    fn chapter_titles(book_id: String) -> Vec<String> {
        let Some(book) = normalize_book_id(&book_id) else {
            return Vec::new();
        };
        fetch_order(&book)
            .map(|order| order.into_iter().map(|c| c.title).collect())
            .unwrap_or_default()
    }

    fn get_chapter(book_id: String, index: u32) -> Option<Chapter> {
        let book = normalize_book_id(&book_id)?;
        let base = config_string("base-url", DEFAULT_BASE_URL);
        let order = fetch_order(&book)?;
        let entry = order.get(index as usize)?;
        let url = chapter_url(&base, &book, entry.cid);
        let html = fetch_page("get-chapter", &url, &toc_url(&base, &book))?;
        let inner = strip_contentdp(&content_div(&html)?);
        let is_ill = entry.illustration || inner.contains("class=\"divimage\"");
        let content = if is_ill {
            extract_image_urls(&inner)
        } else {
            tidy(&html_to_text(&inner))
        };
        // Content must be non-empty to be a real chapter (the host skips
        // empty bodies).
        if content.is_empty() {
            return None;
        }
        Some(Chapter {
            title: entry.title.clone(),
            content,
        })
    }

    fn identify_upload(_filename: String, _file_hash: String) -> Option<BookEntry> {
        None
    }

    fn get_book_file(_book_id: String) -> Option<BookFile> {
        None // wenku8 txt/epub downloads require login; chapter mode only
    }
}

export!(Wenku8Plugin);
