//! Bridge XML empty-element syntax into the HTML parser without changing text.
use std::borrow::Cow;

use scraper::Html;

pub(crate) fn parse_document(source: &str) -> Html {
    Html::parse_document(&html_source(source))
}

fn html_source(source: &str) -> Cow<'_, str> {
    // Permit publication DOCTYPEs, but never resolve external DTDs or expand
    // custom entities. Malformed/unsupported XML retains the HTML recovery path.
    if !source.contains("/>") || source.contains("<!ENTITY") {
        return Cow::Borrowed(source);
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 1_000_000,
        entity_resolver: None,
    };
    let Ok(xml) = roxmltree::Document::parse_with_options(source, options) else {
        return Cow::Borrowed(source);
    };
    let mut out = String::new();
    let mut copied = 0;
    for node in xml.descendants().filter(|n| n.is_element()) {
        let tag = node.tag_name();
        if !matches!(tag.namespace(), None | Some("http://www.w3.org/1999/xhtml"))
            || matches!(
                tag.name(),
                "area"
                    | "base"
                    | "br"
                    | "col"
                    | "embed"
                    | "hr"
                    | "img"
                    | "input"
                    | "link"
                    | "meta"
                    | "param"
                    | "source"
                    | "track"
                    | "wbr"
            )
        {
            continue;
        }
        let range = node.range();
        let element = &source[range.clone()];
        if !element.ends_with("/>") {
            continue;
        }
        // XML supplies element boundaries: do not match lookalikes in comments,
        // CDATA, text, or attribute values, and retain qualified closing names.
        let name = element[1..]
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap();
        out.push_str(&source[copied..range.end - 2]);
        out.push_str("></");
        out.push_str(name);
        out.push('>');
        copied = range.end;
    }
    if copied == 0 {
        Cow::Borrowed(source)
    } else {
        out.push_str(&source[copied..]);
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_empty_html_elements_with_doctype_and_quoted_delimiters() {
        let source = r#"<!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml"><head><script data-note="> />" src="kobo.js" /><style/></head><body><iframe/><p/><p>正文</p></body></html>"#;
        let normalized = html_source(source);
        assert!(normalized.contains(r#"src="kobo.js" ></script><style></style>"#));
        assert!(normalized.contains("<iframe></iframe><p></p><p>正文</p>"));
        let html = parse_document(source);
        assert_eq!(
            html.select(&scraper::Selector::parse("p").unwrap()).count(),
            2
        );
    }

    #[test]
    fn leaves_foreign_elements_void_tags_and_lookalikes_unchanged() {
        let source = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><meta charset="utf-8"/></head><body><!-- <script/> --><p title="&lt;script/&gt;"><![CDATA[<script/>]]></p><br/><img src="pic.png"/><svg xmlns="http://www.w3.org/2000/svg"><path d="M0,0"/></svg><math xmlns="http://www.w3.org/1998/Math/MathML"><mi/></math></body></html>"#;
        assert!(matches!(html_source(source), Cow::Borrowed(_)));
        let with_empty_span = source.replace("<body>", "<body><span/>");
        assert_eq!(
            html_source(&with_empty_span),
            with_empty_span.replace("<span/>", "<span></span>")
        );
    }

    #[test]
    fn retains_html_recovery_for_non_xml_input_and_custom_entities() {
        for source in [
            "<html><head><script></script></head><body><p>正文<br/></body></html>",
            r#"<!DOCTYPE html [<!ENTITY text "<script/>">]><html><body>&text;<p>正文</p></body></html>"#,
        ] {
            assert!(matches!(html_source(source), Cow::Borrowed(_)));
        }
    }

    #[test]
    fn does_not_fetch_external_doctypes_or_entities() {
        let source = r#"<!DOCTYPE html SYSTEM "file:///nonexistent/mandara.dtd"><html><head><script/></head><body><p>正文</p></body></html>"#;
        assert!(html_source(source).contains("<script></script>"));
        let entity = r#"<!DOCTYPE html [<!ENTITY secret SYSTEM "file:///etc/passwd">]><html><body><script/>&secret;</body></html>"#;
        assert!(matches!(html_source(entity), Cow::Borrowed(_)));
    }
}
