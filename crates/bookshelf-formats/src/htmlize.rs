//! Canonical chapter HTML: text → HTML normalization at the ingest
//! boundary (storage unification, docs/storage-unification-design.md
//! §4.1). Every chapter stored in the library is sanitized HTML; plain
//! text sources (txt uploads, chapter-mode plugins) are converted here,
//! once, instead of being special-cased in every reader.
//!
//! Plugin text-chapter conventions (documented in docs/plugins.md) are
//! expanded by [`plugin_text_to_html`]:
//! - a `[插图] 共 N 张` head line is dropped (redundant metadata);
//! - a plate-list line `N. <url>` becomes `<figure><img src="…"></figure>`;
//! - an inline `[插图NN] <url>` mark becomes the same figure at its
//!   position in the paragraph flow.

/// Escape text for HTML content (attribute values are escaped with it
/// too; the URLs we emit cannot contain quotes by construction).
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `true` when `s` is a plate-list line: `N. <http(s) URL>`.
fn is_plate_list_line(s: &str) -> bool {
    let Some((num, rest)) = s.trim().split_once('.') else {
        return false;
    };
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let url = rest.trim();
    url.starts_with("http://") || url.starts_with("https://")
}

/// `true` when `s` is the plate-chapter head line: `[插图] 共 N 张`.
fn is_plate_head(s: &str) -> bool {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("[插图]") else {
        return false;
    };
    let rest = rest.trim();
    let Some(count) = rest.strip_prefix("共") else {
        return false;
    };
    let count = count.trim().trim_end_matches("张").trim();
    !count.is_empty() && count.chars().all(|c| c.is_ascii_digit())
}

/// Find the first inline illustration mark `[插图NN] <url>` in `s`.
/// Returns `(start, end, url)` with byte offsets; `end` is just past the
/// URL.
fn find_mark(s: &str) -> Option<(usize, usize, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' && s[i..].starts_with("[插图") {
            let mut j = i + "[插图".len();
            let digits_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start && s[j..].starts_with(']') {
                let mut k = j + 1;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                for scheme in ["https://", "http://"] {
                    if s[k..].starts_with(scheme) {
                        let url_end = s[k..]
                            .find([' ', '\t', '\n', ']'])
                            .map(|p| k + p)
                            .unwrap_or(s.len());
                        if url_end > k {
                            return Some((i, url_end, &s[k..url_end]));
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn figure(url: &str) -> String {
    format!("<figure><img src=\"{}\"/></figure>", escape_html(url))
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

/// Convert plugin chapter text (plain text plus the illustration
/// conventions) to canonical HTML.
pub fn plugin_text_to_html(content: &str) -> String {
    // Drop the plate-chapter head line up front.
    let content = content
        .lines()
        .filter(|l| !is_plate_head(l))
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = String::with_capacity(content.len() + 64);
    for para in content.split("\n\n") {
        // Accumulate escaped text lines; a figure flushes them as one <p>.
        let mut text = String::new();
        for line in para.split('\n') {
            if is_plate_list_line(line) {
                let url = line.trim().split_once('.').unwrap().1.trim();
                if !text.trim().is_empty() {
                    out.push_str("<p>");
                    out.push_str(&escape_html(&text).replace('\n', "<br/>"));
                    out.push_str("</p>\n");
                    text.clear();
                }
                out.push_str(&figure(url));
                out.push('\n');
                continue;
            }
            // Inline marks: emit the text between figures, then the figures.
            let mut rest = line;
            loop {
                match find_mark(rest) {
                    Some((start, end, url)) => {
                        text.push_str(&rest[..start]);
                        if !text.trim().is_empty() {
                            out.push_str("<p>");
                            out.push_str(&escape_html(text.trim()).replace('\n', "<br/>"));
                            out.push_str("</p>\n");
                            text.clear();
                        }
                        out.push_str(&figure(url));
                        out.push('\n');
                        rest = &rest[end..];
                    }
                    None => {
                        text.push_str(rest);
                        break;
                    }
                }
            }
            text.push('\n');
        }
        if !text.trim().is_empty() {
            out.push_str("<p>");
            out.push_str(&escape_html(text.trim()).replace('\n', "<br/>"));
            out.push_str("</p>\n");
        }
    }
    out
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
    fn plate_head_is_dropped_and_list_becomes_figures() {
        let html = plugin_text_to_html(
            "[插图] 共 2 张\n1. https://pic.example/a.jpg\n2. https://pic.example/b.jpg",
        );
        assert_eq!(
            html,
            "<figure><img src=\"https://pic.example/a.jpg\"/></figure>\n\
             <figure><img src=\"https://pic.example/b.jpg\"/></figure>\n"
        );
    }

    #[test]
    fn inline_marks_become_figures_at_position() {
        let html = plugin_text_to_html(
            "她抬起头。[插图001] https://pic.example/a.jpg 远处的塔楼映在水面。",
        );
        assert_eq!(
            html,
            "<p>她抬起头。</p>\n\
             <figure><img src=\"https://pic.example/a.jpg\"/></figure>\n\
             <p>远处的塔楼映在水面。</p>\n"
        );
    }

    #[test]
    fn prose_without_marks_matches_plain_conversion() {
        let t = "段落一。\n\n段落二。";
        assert_eq!(plugin_text_to_html(t), text_to_html(t));
    }

    #[test]
    fn bracket_text_without_url_stays_text() {
        // A `[插图…]` without a following URL is ordinary text.
        let html = plugin_text_to_html("他说[插图]这个词。[插图001]not-a-url");
        assert_eq!(html, "<p>他说[插图]这个词。[插图001]not-a-url</p>\n");
    }

    #[test]
    fn non_http_plate_urls_stay_text() {
        let html = plugin_text_to_html("1. ftp://pic.example/a.jpg");
        assert_eq!(html, "<p>1. ftp://pic.example/a.jpg</p>\n");
    }
}
