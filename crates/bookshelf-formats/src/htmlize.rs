//! Canonical chapter HTML: text → HTML normalization at the ingest
//! boundary (storage unification, docs/storage-unification-design.md
//! §4.1). Every chapter stored in the library is sanitized HTML; plain
//! text sources (txt uploads, text-format plugin chapters) are converted
//! here, once, instead of being special-cased in every reader.
//!
//! Plugins declare how their chapter body is interpreted via the WIT
//! `chapter.format`:
//!
//!   - `"text"` (default) — the body is plain text; the host escapes it
//!     and wraps blank-line-separated paragraphs in `<p>` (single
//!     newlines inside a paragraph become `<br/>`).
//!   - `"html"` — the plugin already emits final HTML (escaped text and
//!     known safe tags, e.g. `<figure><img>` for its illustration
//!     plates). The host **sanitizes it through the same whitelist as
//!     epub content** ([`sanitize_html_fragment`]) and then annotates
//!     image references; a broken or malicious plugin cannot inject
//!     scripts, event handlers or foreign-content markup into the reader.
//!
//! The wenku8 plugin is the only `"html"` producer today — it expands its
//! own 插图 conventions guest-side instead of the host knowing them (see
//! `plugins/wenku8-plugin/src/lib.rs`).

use ego_tree::NodeRef;
use scraper::Html;
use scraper::node::Node;

/// Tags that are dropped **with their whole subtree**.
pub const DROP_TAGS: [&str; 25] = [
    "script", "style", "link", "meta", "base", "noscript", "title", "head", "iframe", "object",
    "embed", "form", "input", "button", "select", "textarea", "audio", "video", "source", "track",
    "svg", "canvas", "template", "math", "map",
];

/// Tags kept in chapter content, per the safe subset of HTML usable in the
/// reading UI. Ruby and friends are kept because CJK epubs rely on them.
pub const ALLOWED_TAGS: [&str; 47] = [
    "p",
    "div",
    "span",
    "br",
    "hr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "em",
    "strong",
    "i",
    "b",
    "u",
    "s",
    "small",
    "sub",
    "sup",
    "mark",
    "blockquote",
    "pre",
    "code",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "img",
    "a",
    "figure",
    "figcaption",
    "ruby",
    "rb",
    "rt",
    "rp",
    "q",
    "cite",
];

/// Elements serialized without a closing tag.
pub const VOID_TAGS: [&str; 3] = ["br", "hr", "img"];

/// Allowed attributes per tag; the empty slice means "none".
pub fn allowed_attrs(tag: &str) -> &'static [&'static str] {
    match tag {
        "a" => &["href", "title", "id"],
        "img" => &["src", "alt", "title", "id"],
        "th" | "td" => &["colspan", "rowspan", "align", "id"],
        "table" => &["align", "id"],
        _ => &["id"],
    }
}

/// Which `href` values survive sanitization: in-page anchors and
/// http(s)/mailto links (opened by the reader in a new tab).
pub fn keep_link(href: &str) -> bool {
    let h = href.trim();
    h.starts_with('#')
        || h.starts_with("http://")
        || h.starts_with("https://")
        || h.starts_with("mailto:")
}

/// Escape a text/attribute value for safe inclusion in HTML.
fn escape_html_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// Rewrite a single attribute value; return `None` to drop the attribute.
/// `FnMut` because the epub rewriter mutates its image-extraction state.
pub type AttrRewriter<'a> = dyn FnMut(&str, &str, &str) -> Option<String> + 'a;

/// Sanitize an untrusted HTML fragment into the safe reading-UI subset:
/// parse it, walk the tree and re-serialize from scratch.
///
/// - keeps structure (paragraphs, headings, lists, tables, ruby, figures, …);
/// - drops scripts, styles, forms, foreign content and any event handler /
///   `style` attribute (whitelist approach);
/// - unknown tags are unwrapped (their text content survives);
/// - `rewrite_attr` may rewrite/drop attribute values per source semantics
///   (e.g. epub image extraction and link filtering; the plugin-HTML path
///   passes an identity rewriter that only drops `href`s to non-http(s)/
///   non-anchor targets).
///
/// This is the single shared sanitizer for epub spine documents and
/// `chapter.format = "html"` plugin bodies.
pub fn sanitize_html_fragment(html: &str, rewrite_attr: &mut AttrRewriter<'_>) -> String {
    let parsed = Html::parse_fragment(html);
    let root = parsed.root_element();
    let mut out = String::with_capacity(html.len());
    for child in root.children() {
        sanitize_node(&mut out, &child, rewrite_attr);
    }
    out
}

fn sanitize_node(
    out: &mut String,
    node: &NodeRef<'_, scraper::node::Node>,
    rewrite_attr: &mut AttrRewriter<'_>,
) {
    match node.value() {
        Node::Text(t) => escape_html_into(out, &t.text),
        Node::Element(el) => {
            let tag = el.name().to_ascii_lowercase();
            if DROP_TAGS.contains(&tag.as_str()) {
                return; // subtree dropped entirely
            }
            let allowed = ALLOWED_TAGS.contains(&tag.as_str());
            if allowed {
                out.push('<');
                out.push_str(&tag);
                for (name, value) in el.attrs() {
                    let name = name.to_ascii_lowercase();
                    if !allowed_attrs(&tag).contains(&name.as_str()) {
                        continue;
                    }
                    let Some(value) = rewrite_attr(&tag, &name, value) else {
                        continue;
                    };
                    out.push(' ');
                    out.push_str(&name);
                    out.push_str("=\"");
                    escape_html_into(out, &value);
                    out.push('"');
                }
                out.push('>');
            }
            for child in node.children() {
                sanitize_node(out, &child, rewrite_attr);
            }
            if allowed && !VOID_TAGS.contains(&tag.as_str()) {
                out.push_str("</");
                out.push_str(&tag);
                out.push('>');
            }
        }
        // Comments, doctype, processing instructions are not content.
        _ => {}
    }
}

/// Rewrite callback for plugin HTML: keep `img.src` verbatim, keep
/// `a.href` only when it is an in-page anchor / http(s) / mailto link,
/// everything else passes through.
pub fn plugin_attr_rewriter(tag: &str, name: &str, value: &str) -> Option<String> {
    match (tag, name) {
        ("a", "href") if !keep_link(value) => None,
        _ => Some(value.to_string()),
    }
}

/// Escape text for HTML content (attribute values are escaped with it
/// too; the URLs we emit cannot contain quotes by construction).
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Convert plain-text chapter content to canonical HTML: blank-line
/// paragraph split, each paragraph wrapped in `<p>`; single newlines
/// inside a paragraph become `<br/>`.
pub fn text_to_html(content: &str) -> String {
    let mut out = String::with_capacity(content.len() + 32);
    for para in content.split("\n\n") {
        let para = para.trim_matches('\n');
        if para.trim().is_empty() {
            continue;
        }
        out.push_str("<p>");
        out.push_str(&escape_html(para).replace('\n', "<br/>"));
        out.push_str("</p>\n");
    }
    out
}

/// Normalize a plugin chapter body to canonical HTML according to its
/// `format` (`"text"` or `"html"`, see the module docs). `"html"` bodies
/// are run through the shared whitelist sanitizer — a broken or malicious
/// plugin cannot inject raw markup into the reader. Unknown formats fall
/// back to plain text.
pub fn plugin_to_html(format: &str, content: &str) -> String {
    match format {
        "html" => sanitize_html_fragment(content, &mut plugin_attr_rewriter),
        _ => text_to_html(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_paragraphs_are_wrapped_and_escaped() {
        let html = text_to_html("第一段。<>&\n\n第二段。");
        assert_eq!(html, "<p>第一段。&lt;&gt;&amp;</p>\n<p>第二段。</p>\n");
    }

    #[test]
    fn single_newlines_become_breaks() {
        let html = text_to_html("对话一。\n对话二。");
        assert_eq!(html, "<p>对话一。<br/>对话二。</p>\n");
    }

    #[test]
    fn blank_runs_collapse() {
        let html = text_to_html("甲。\n\n\n\n\n乙。");
        assert_eq!(html, "<p>甲。</p>\n<p>乙。</p>\n");
    }

    #[test]
    fn empty_text_stays_empty() {
        assert_eq!(text_to_html(""), "");
        assert_eq!(text_to_html("\n \n"), "");
    }

    #[test]
    fn text_format_goes_through_the_text_converter() {
        assert_eq!(
            plugin_to_html("text", "a < b\n\nc"),
            text_to_html("a < b\n\nc")
        );
    }

    #[test]
    fn html_format_is_sanitized() {
        // Safe structure survives (self-closing `<img/>` is re-serialized in
        // HTML5 void form).
        let html = "<p>第一段。</p>\n<figure><img src=\"/api/images/abc\"/></figure>\n";
        assert_eq!(
            plugin_to_html("html", html),
            "<p>第一段。</p>\n<figure><img src=\"/api/images/abc\"></figure>\n"
        );
    }

    #[test]
    fn html_format_strips_script_and_handlers() {
        // A broken or malicious plugin cannot inject scripts, event
        // handlers, styles or foreign-content markup into the reader.
        let html = concat!(
            "<p>正文</p>",
            "<script>alert(1)</script>",
            "<p onclick=\"alert(2)\" style=\"color:red\">干净</p>",
            "<svg><text>svg 文本</text></svg>",
            "<iframe src=\"https://evil.example\"></iframe>",
            "<form><input name=\"x\"></form>",
        );
        let out = plugin_to_html("html", html);
        assert_eq!(out, "<p>正文</p><p>干净</p>");
    }

    #[test]
    fn html_format_unwraps_unknown_tags_and_filters_hrefs() {
        // Unknown tags lose their markup but keep their text; `href`s
        // must be in-page anchors / http(s) / mailto.
        let html = concat!(
            "<p>a <custom>文字</custom></p>",
            "<a href=\"javascript:alert(1)\">坏</a> ",
            "<a href=\"https://example.com\">好</a> ",
            "<a href=\"#sec1\">锚</a>",
        );
        let out = plugin_to_html("html", html);
        assert_eq!(
            out,
            "<p>a 文字</p><a>坏</a> <a href=\"https://example.com\">好</a> <a href=\"#sec1\">锚</a>"
        );
    }

    #[test]
    fn unknown_format_falls_back_to_text() {
        assert_eq!(plugin_to_html("markdown", "a < b"), text_to_html("a < b"));
    }
}
