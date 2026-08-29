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
//!     plates); the host passes it through and only annotates image
//!     references (`annotate_image_dims` in the server).
//!
//! The wenku8 plugin is the only `"html"` producer today — it expands its
//! own 插图 conventions guest-side instead of the host knowing them (see
//! `plugins/wenku8-plugin/src/lib.rs`).

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
/// `format` (`"text"` or `"html"`, see the module docs). Unknown formats
/// fall back to plain text — a broken plugin never injects raw markup.
pub fn plugin_to_html(format: &str, content: &str) -> String {
    match format {
        "html" => content.to_string(),
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
    fn html_format_is_passed_through_unchanged() {
        let html = "<p>第一段。</p>\n<figure><img src=\"/api/images/abc\"/></figure>\n";
        assert_eq!(plugin_to_html("html", html), html);
    }

    #[test]
    fn unknown_format_falls_back_to_text() {
        assert_eq!(plugin_to_html("markdown", "a < b"), text_to_html("a < b"));
    }
}
