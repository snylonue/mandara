//! Passive publication presentation, separate from package/reading-order parsing.
//! Output is a full document for a script-free, network-restricted sandboxed frame.
use crate::{
    ParsedImage,
    epub_uri::resolve,
    htmlize::{ALLOWED_TAGS, VOID_TAGS, escape_html, keep_link},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use cssparser::{Parser, ParserInput, ToCss, Token};
use ego_tree::NodeRef;
use epub::doc::EpubDoc;
use scraper::{Html, node::Node};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub document: String,
    pub reference: Option<String>,
    pub reason: &'static str,
}

pub(crate) struct Renderer<'a, R: Read + Seek> {
    pub doc: &'a mut EpubDoc<R>,
    pub images: &'a mut Vec<ParsedImage>,
    pub seen: &'a mut HashMap<String, usize>,
    pub diagnostics: &'a mut Vec<Diagnostic>,
    targets: Option<&'a HashMap<String, u32>>,
    styles: HashSet<String>,
    resources: HashSet<String>,
}

impl<'a, R: Read + Seek> Renderer<'a, R> {
    pub fn new(
        doc: &'a mut EpubDoc<R>,
        images: &'a mut Vec<ParsedImage>,
        seen: &'a mut HashMap<String, usize>,
        diagnostics: &'a mut Vec<Diagnostic>,
    ) -> Self {
        Self {
            doc,
            images,
            seen,
            diagnostics,
            targets: None,
            styles: HashSet::new(),
            resources: HashSet::new(),
        }
    }

    pub fn with_targets(mut self, targets: &'a HashMap<String, u32>) -> Self {
        self.targets = Some(targets);
        self
    }

    fn link(&mut self, base: &Path, href: &str) -> Option<String> {
        let trimmed = href.trim();
        if keep_link(trimmed) && !trimmed.starts_with('#') {
            return Some(trimmed.to_owned());
        }
        let reference = resolve(base, trimmed)?;
        let idx = self
            .targets
            .and_then(|targets| targets.get(&reference.path))
            .copied();
        let fragment = reference
            .fragment
            .as_ref()
            .map(|f| {
                format!(
                    "#{}",
                    percent_encoding::utf8_percent_encode(f, percent_encoding::NON_ALPHANUMERIC)
                )
            })
            .unwrap_or_default();
        if reference.path == base.to_string_lossy() && reference.fragment.is_some() {
            return Some(fragment);
        }
        if let Some(idx) = idx {
            return Some(format!("epub:chapter/{idx}{fragment}"));
        }
        self.note(base, Some(href), "unreachable-internal-link");
        None
    }

    fn note(&mut self, base: &Path, reference: Option<&str>, reason: &'static str) {
        self.diagnostics.push(Diagnostic {
            document: base.to_string_lossy().into_owned(),
            reference: reference.map(str::to_owned),
            reason,
        });
    }

    pub fn document(&mut self, source: &str, base: &Path, layout: &str) -> String {
        let parsed = Html::parse_document(source);
        let mut out = String::from("<!doctype html>");
        self.node(&mut out, &parsed.root_element(), base, 0, false, layout);
        out
    }

    fn node(
        &mut self,
        out: &mut String,
        node: &NodeRef<'_, Node>,
        base: &Path,
        depth: usize,
        in_svg: bool,
        layout: &str,
    ) {
        if depth > 256 {
            self.note(base, None, "unsupported-nesting-depth");
            return;
        }
        match node.value() {
            Node::Text(t) => out.push_str(&escape_html(&t.text)),
            Node::Element(el) => {
                let tag = el.name();
                let svg = in_svg || tag == "svg";
                if matches!(
                    tag,
                    "script"
                        | "iframe"
                        | "object"
                        | "embed"
                        | "form"
                        | "input"
                        | "button"
                        | "select"
                        | "textarea"
                        | "base"
                        | "template"
                        | "noscript"
                        | "animate"
                        | "animateMotion"
                        | "animateTransform"
                        | "set"
                        | "foreignObject"
                ) {
                    self.note(base, Some(tag), "blocked-active-content");
                    return;
                }
                if matches!(tag, "audio" | "video" | "canvas") {
                    self.note(base, Some(tag), "unsupported-media");
                    // Canvas fallback text is visible when scripting is disabled.
                    for child in node.children() {
                        self.node(out, &child, base, depth + 1, svg, layout);
                    }
                    return;
                }
                if tag == "link" {
                    if el.attr("rel").is_some_and(|v| {
                        v.split_ascii_whitespace()
                            .any(|v| v.eq_ignore_ascii_case("stylesheet"))
                    }) && let Some(href) = el.attr("href")
                    {
                        let css = self.stylesheet(base, href, 0);
                        out.push_str("<style>");
                        out.push_str(&css.replace('<', "\\3c "));
                        out.push_str("</style>");
                    }
                    return;
                }
                if tag == "style" {
                    let css: String = node
                        .children()
                        .filter_map(|n| match n.value() {
                            Node::Text(t) => Some(t.text.to_string()),
                            _ => None,
                        })
                        .collect();
                    let css = self.css(&css, base, 0);
                    out.push_str("<style>");
                    out.push_str(&css.replace('<', "\\3c "));
                    out.push_str("</style>");
                    return;
                }
                if tag == "meta" {
                    if el
                        .attr("name")
                        .is_some_and(|n| n.eq_ignore_ascii_case("viewport"))
                        && let Some(content) = el.attr("content")
                    {
                        out.push_str(&format!(
                            "<meta name=\"viewport\" content=\"{}\">",
                            escape_html(content)
                        ));
                    }
                    return;
                }
                let allowed = ALLOWED_TAGS.contains(&tag)
                    || matches!(
                        tag,
                        "html"
                            | "head"
                            | "body"
                            | "title"
                            | "section"
                            | "article"
                            | "aside"
                            | "header"
                            | "footer"
                            | "main"
                            | "nav"
                            | "address"
                            | "caption"
                            | "colgroup"
                            | "col"
                            | "bdi"
                            | "bdo"
                            | "wbr"
                            | "abbr"
                            | "del"
                            | "ins"
                            | "time"
                            | "details"
                            | "summary"
                            | "kbd"
                            | "samp"
                            | "var"
                            | "picture"
                    )
                    || (svg && SVG_TAGS.contains(&tag))
                    || el.name.ns.as_ref() == "http://www.w3.org/1998/Math/MathML";
                if allowed {
                    out.push('<');
                    out.push_str(tag);
                    if tag == "html" {
                        out.push_str(&format!(" data-epub-layout=\"{}\"", escape_html(layout)));
                    }
                    for (name, value) in el.attrs() {
                        let name_lower = name.to_ascii_lowercase();
                        if name_lower.starts_with("on")
                            || matches!(
                                name_lower.as_str(),
                                "srcset" | "action" | "formaction" | "target" | "data-epub-layout"
                            )
                        {
                            continue;
                        }
                        let global = matches!(
                            name_lower.as_str(),
                            "id" | "class"
                                | "lang"
                                | "xml:lang"
                                | "dir"
                                | "title"
                                | "role"
                                | "epub:type"
                        ) || name_lower.starts_with("aria-");
                        let passive = matches!(
                            name_lower.as_str(),
                            "alt"
                                | "width"
                                | "height"
                                | "colspan"
                                | "rowspan"
                                | "align"
                                | "start"
                                | "value"
                                | "scope"
                                | "headers"
                                | "open"
                                | "datetime"
                        );
                        let rewritten = match name_lower.as_str() {
                            "style" => Some(self.css(value, base, 0)),
                            "src" if tag == "img" => self.image(base, value),
                            "href" if tag == "a" && !svg => self.link(base, value),
                            "href" if svg && tag == "image" => self.passive_url(base, value, 0),
                            "href" if svg => value.starts_with('#').then(|| value.to_owned()),
                            _ if global
                                || passive
                                || (svg && SVG_ATTRS.contains(&name_lower.as_str()))
                                || (el.name.ns.as_ref()
                                    == "http://www.w3.org/1998/Math/MathML"
                                    && MATH_ATTRS.contains(&name_lower.as_str())) =>
                            {
                                Some(value.to_owned())
                            }
                            _ => None,
                        };
                        if let Some(value) = rewritten {
                            let value = if svg
                                && matches!(
                                    name_lower.as_str(),
                                    "fill"
                                        | "stroke"
                                        | "filter"
                                        | "clip-path"
                                        | "mask"
                                        | "marker-start"
                                        | "marker-mid"
                                        | "marker-end"
                                ) {
                                self.css(&value, base, 0)
                            } else {
                                value
                            };
                            out.push(' ');
                            out.push_str(if name == "xml:lang" { "lang" } else { name });
                            out.push_str("=\"");
                            out.push_str(&escape_html(&value));
                            out.push('"');
                        }
                    }
                    out.push('>');
                    if tag == "head" {
                        out.push_str("<meta charset=\"utf-8\">");
                    }
                }
                for child in node.children() {
                    self.node(out, &child, base, depth + 1, svg, layout);
                }
                if allowed && !VOID_TAGS.contains(&tag) && !matches!(tag, "col" | "wbr") {
                    out.push_str("</");
                    out.push_str(tag);
                    out.push('>');
                }
            }
            _ => {}
        }
    }

    fn resource(&mut self, base: &Path, href: &str) -> Option<(String, Vec<u8>, String)> {
        let reference = match resolve(base, href) {
            Some(r) => r,
            None => {
                self.note(base, Some(href), "blocked-non-container-url");
                return None;
            }
        };
        let mime = self.doc.get_resource_mime_by_path(&reference.path);
        let Some(mime) = mime else {
            self.note(base, Some(href), "missing-manifest-resource");
            return None;
        };
        let Some(bytes) = self.doc.get_resource_by_path(&reference.path) else {
            self.note(base, Some(href), "missing-container-resource");
            return None;
        };
        Some((reference.path, bytes, mime))
    }

    fn image(&mut self, base: &Path, href: &str) -> Option<String> {
        if href.starts_with("data:") {
            return self.data_url(base, href);
        }
        let (path, bytes, mime) = self.resource(base, href)?;
        if !matches!(
            mime.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml"
        ) {
            self.note(base, Some(href), "unsupported-image-media-type");
            return None;
        }
        let bytes = if mime == "image/svg+xml" {
            self.svg_bytes(&bytes, Path::new(&path), 0)?
        } else {
            bytes
        };
        let n = if let Some(n) = self.seen.get(&path) {
            *n
        } else {
            let n = self.images.len();
            self.images.push(ParsedImage { bytes, mime });
            self.seen.insert(path, n);
            n
        };
        Some(format!("image:{n}"))
    }

    fn data_url(&mut self, base: &Path, href: &str) -> Option<String> {
        let (header, payload) = href.split_once(',')?;
        let mime = header
            .strip_prefix("data:")?
            .split(';')
            .next()?
            .to_ascii_lowercase();
        if !matches!(
            mime.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
                | "image/svg+xml"
                | "font/ttf"
                | "font/otf"
                | "font/woff"
                | "font/woff2"
        ) {
            self.note(base, Some(header), "blocked-data-url");
            return None;
        }
        let bytes = if header.split(';').any(|p| p.eq_ignore_ascii_case("base64")) {
            STANDARD.decode(payload).ok()?
        } else {
            percent_encoding::percent_decode_str(payload).collect()
        };
        let bytes = if mime == "image/svg+xml" {
            self.svg_bytes(&bytes, base, 0)?
        } else {
            bytes
        };
        Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
    }

    fn svg_bytes(&mut self, bytes: &[u8], base: &Path, depth: usize) -> Option<Vec<u8>> {
        if depth > 16 {
            self.note(base, None, "cyclic-resource-reference");
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let key = base.to_string_lossy().into_owned();
        if !self.resources.insert(key.clone()) {
            self.note(base, None, "cyclic-resource-reference");
            return None;
        }
        let parsed = Html::parse_fragment(text);
        let svg = parsed
            .select(&scraper::Selector::parse("svg").unwrap())
            .next();
        let Some(svg) = svg else {
            self.resources.remove(&key);
            return None;
        };
        let mut out = String::new();
        self.node(&mut out, &svg, base, depth, true, "reflowable");
        // An SVG image is an XML resource, unlike inline HTML foreign content.
        // The HTML serializer uses explicit closing tags, and declares its namespace.
        self.resources.remove(&key);
        Some(
            out.replacen("<svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"", 1)
                .into_bytes(),
        )
    }

    fn passive_url(&mut self, base: &Path, href: &str, depth: usize) -> Option<String> {
        if href.starts_with('#') {
            return Some(href.to_owned());
        }
        if href.starts_with("data:") {
            return self.data_url(base, href);
        }
        let (path, bytes, mime) = self.resource(base, href)?;
        let bytes = if mime == "image/svg+xml" {
            self.svg_bytes(&bytes, Path::new(&path), depth + 1)?
        } else {
            bytes
        };
        if !matches!(
            mime.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
                | "image/svg+xml"
                | "font/ttf"
                | "font/otf"
                | "font/woff"
                | "font/woff2"
                | "application/vnd.ms-opentype"
                | "application/font-sfnt"
                | "application/font-woff"
        ) {
            self.note(base, Some(href), "unsupported-passive-resource");
            return None;
        }
        Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
    }

    fn stylesheet(&mut self, base: &Path, href: &str, depth: usize) -> String {
        if depth > 16 {
            self.note(base, Some(href), "cyclic-stylesheet-import");
            return String::new();
        }
        let Some((path, bytes, mime)) = self.resource(base, href) else {
            return String::new();
        };
        if mime != "text/css" {
            self.note(base, Some(href), "unsupported-stylesheet-media-type");
            return String::new();
        }
        if bytes.is_empty() {
            self.note(base, Some(href), "empty-stylesheet");
        }
        if !self.styles.insert(path.clone()) {
            self.note(base, Some(href), "cyclic-stylesheet-import");
            return String::new();
        }
        let text = String::from_utf8_lossy(&bytes);
        let css = self.css(&text, Path::new(&path), depth + 1);
        self.styles.remove(&path);
        css
    }

    fn css(&mut self, css: &str, base: &Path, depth: usize) -> String {
        let mut input = ParserInput::new(css);
        self.css_tokens(&mut Parser::new(&mut input), base, depth)
    }

    fn css_tokens<'i>(&mut self, parser: &mut Parser<'i, '_>, base: &Path, depth: usize) -> String {
        if depth > 32 {
            self.note(base, None, "unsupported-css-nesting");
            return String::new();
        }
        let mut out = String::new();
        while let Ok(token) = parser.next_including_whitespace_and_comments().cloned() {
            match token {
                Token::AtKeyword(ref name) if name.eq_ignore_ascii_case("import") => {
                    let href = parser.next().ok().cloned().and_then(|token| match token {
                        Token::QuotedString(s) | Token::UnquotedUrl(s) => Some(s.to_string()),
                        Token::Function(name) if name.eq_ignore_ascii_case("url") => parser
                            .parse_nested_block(|p| {
                                p.expect_string_cloned()
                                    .map(|s| s.to_string())
                                    .map_err(cssparser::ParseError::<()>::from)
                            })
                            .ok(),
                        _ => None,
                    });
                    let mut media = String::new();
                    while let Ok(token) = parser.next_including_whitespace_and_comments().cloned() {
                        if token == Token::Semicolon {
                            break;
                        }
                        media.push_str(&token.to_css_string());
                    }
                    if let Some(href) = href {
                        let css = self.stylesheet(base, &href, depth + 1);
                        if media.trim().is_empty() {
                            out.push_str(&css);
                        } else {
                            out.push_str(&format!("@media {media}{{{css}}}"));
                        }
                    }
                }
                Token::UnquotedUrl(url) => {
                    if let Some(url) = self.passive_url(base, &url, depth) {
                        out.push_str(&format!(
                            "url({})",
                            Token::QuotedString(url.as_str().into()).to_css_string()
                        ));
                    } else {
                        out.push_str("url(\"\")");
                    }
                }
                Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                    let href: Option<String> = parser
                        .parse_nested_block(|p| {
                            p.expect_string_cloned()
                                .map(|s| s.to_string())
                                .map_err(cssparser::ParseError::<()>::from)
                        })
                        .ok();
                    if let Some(url) = href.and_then(|href| self.passive_url(base, &href, depth)) {
                        out.push_str(&format!(
                            "url({})",
                            Token::QuotedString(url.as_str().into()).to_css_string()
                        ));
                    } else {
                        out.push_str("url(\"\")");
                    }
                }
                Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock => {
                    let opening = token.to_css_string();
                    let closing = match token {
                        Token::SquareBracketBlock => "]",
                        Token::CurlyBracketBlock => "}",
                        _ => ")",
                    };
                    let nested: Result<String, cssparser::ParseError<'i, ()>> =
                        parser.parse_nested_block(|p| Ok(self.css_tokens(p, base, depth + 1)));
                    if let Ok(nested) = nested {
                        out.push_str(&opening);
                        out.push_str(&nested);
                        out.push_str(closing);
                    }
                }
                Token::BadUrl(_) | Token::BadString(_) => {
                    self.note(base, None, "invalid-css-token");
                }
                _ => out.push_str(&token.to_css_string()),
            }
        }
        out
    }
}

const SVG_TAGS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "image",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "clipPath",
    "mask",
    "pattern",
    "linearGradient",
    "radialGradient",
    "stop",
    "filter",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feFlood",
    "feGaussianBlur",
    "feMerge",
    "feMergeNode",
    "feOffset",
    "feFuncR",
    "feFuncG",
    "feFuncB",
    "feFuncA",
    "title",
    "desc",
];
const SVG_ATTRS: &[&str] = &[
    "viewbox",
    "preserveaspectratio",
    "x",
    "y",
    "x1",
    "x2",
    "y1",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "d",
    "points",
    "transform",
    "fill",
    "fill-rule",
    "fill-opacity",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-opacity",
    "opacity",
    "offset",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "mask",
    "filter",
    "patternunits",
    "patterntransform",
    "gradientunits",
    "gradienttransform",
    "spreadmethod",
    "font-family",
    "font-size",
    "font-weight",
    "text-anchor",
    "dx",
    "dy",
    "rotate",
    "textlength",
    "lengthadjust",
    "in",
    "in2",
    "result",
    "stddeviation",
    "mode",
    "type",
    "values",
    "operator",
    "k1",
    "k2",
    "k3",
    "k4",
    "flood-color",
    "flood-opacity",
];
const MATH_ATTRS: &[&str] = &[
    "display",
    "mathvariant",
    "mathsize",
    "mathcolor",
    "mathbackground",
    "stretchy",
    "fence",
    "separator",
    "lspace",
    "rspace",
    "accent",
    "accentunder",
    "columnalign",
    "rowalign",
    "columnspacing",
    "rowspacing",
    "linethickness",
    "notation",
    "encoding",
];
