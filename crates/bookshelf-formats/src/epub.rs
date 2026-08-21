//! EPUB parsing per the EPUB standard (OPF spine + NCX/nav TOC).
//!
//! What this does, and why:
//!
//! - **Reading order = OPF spine** ([EPUB 3.3 § 3.17](https://www.w3.org/TR/epub-33/#sec-spine));
//!   items with `linear="no"` (cover pages, nav docs, …) are auxiliary and
//!   never become chapters.
//! - **Chapter titles come from the table of contents**: the EPUB 3 nav
//!   document (`<nav epub:type="toc">`) or, for EPUB 2, the NCX `navMap`.
//!   Entries map to spine documents by resource path; documents without a
//!   TOC entry fall back to their first heading, then to a numbered title.
//! - **Content stays XHTML**: chapters are stored as sanitized HTML
//!   fragments so the reading UI can render paragraphs, headings, ruby,
//!   tables and images the way the EPUB author intended. Internal image
//!   resources are inlined as `data:` URIs (resources live inside the OCF
//!   container per [EPUB 3.3 § 3.2](https://www.w3.org/TR/epub-33/#sec-ocf)).
//! - Output is a safe whitelist: scripts, styles, event handlers and
//!   foreign-content tags are stripped; only reachable link targets are kept.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use bookshelf_core::error::{Error, Result};
use bookshelf_core::model::{ChapterFormat, TocNode};
use ego_tree::NodeRef;
use epub::doc::{EpubDoc, NavPoint};
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};

use crate::{ParsedBook, ParsedChapter};

/// XHTML / HTML content types accepted as spine documents.
const XHTML_MIMES: [&str; 2] = ["application/xhtml+xml", "text/html"];
/// Manifest media type of an EPUB 2 NCX TOC document.
const NCX_MIME: &str = "application/x-dtbncx+xml";

/// Tags that are dropped **with their whole subtree**.
const DROP_TAGS: [&str; 25] = [
    "script", "style", "link", "meta", "base", "noscript", "title", "head", "iframe", "object",
    "embed", "form", "input", "button", "select", "textarea", "audio", "video", "source", "track",
    "svg", "canvas", "template", "math", "map",
];

/// Tags kept in chapter content, per the safe subset of HTML usable in the
/// reading UI. Ruby and friends are kept because CJK epubs rely on them.
const ALLOWED_TAGS: [&str; 47] = [
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
const VOID_TAGS: [&str; 3] = ["br", "hr", "img"];

/// Allowed attributes per tag; the empty slice means "none".
fn allowed_attrs(tag: &str) -> &'static [&'static str] {
    match tag {
        "a" => &["href", "title", "id"],
        "img" => &["src", "alt", "title", "id"],
        "th" | "td" => &["colspan", "rowspan", "align", "id"],
        "table" => &["align", "id"],
        _ => &["id"],
    }
}

/// Parse epub bytes into a normalized book.
pub fn parse(bytes: &[u8]) -> Result<ParsedBook> {
    // `EpubDoc` works on paths; write the upload to a temp file.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "bookshelf-{pid}-{stamp}.epub",
        pid = std::process::id()
    ));
    fs::write(&tmp, bytes)?;
    let result = parse_path(&tmp);
    let _ = fs::remove_file(&tmp);
    result
}

fn parse_path(path: &Path) -> Result<ParsedBook> {
    let mut doc =
        epub::doc::EpubDoc::new(path).map_err(|e| Error::InvalidArgument(format!("{e}")))?;

    let title = doc
        .get_title()
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "Untitled".into());

    let authors = doc
        .mdata("creator")
        .map(|m| vec![m.value.trim().to_string()])
        .unwrap_or_default();

    let description = doc.mdata("description").map(|m| m.value.trim().to_string());

    // TOC (nav doc / NCX) → tree, with a flattened path → label map for
    // chapter titles (leaf entries win for documents with nested entries).
    let branches = extract_toc_tree(&mut doc);
    let mut flat = Vec::new();
    flatten_branches(&branches, &mut flat);
    let title_by_path: HashMap<String, String> =
        flat.into_iter().map(|(p, l)| (path_key(&p), l)).collect();

    // Spine order is the reading order; `linear="no"` items (cover, nav,
    // acknowledgements, …) are auxiliary and must not become chapters.
    let spine: Vec<(String, String, PathBuf)> = doc
        .spine
        .iter()
        .filter(|s| s.linear)
        .filter_map(|s| {
            doc.resources
                .get(&s.idref)
                .map(|r| (s.idref.clone(), r.mime.clone(), r.path.clone()))
        })
        .collect();
    let spine_pos: HashMap<String, usize> = spine
        .iter()
        .enumerate()
        .map(|(i, (_, _, p))| (path_key(p), i))
        .collect();

    let mut chapters = Vec::new();
    let mut kept_spine = Vec::new(); // spine positions that became chapters
    for (chapter_idx, (id, mime, resource_path)) in spine.into_iter().enumerate() {
        if !XHTML_MIMES.contains(&mime.as_str()) {
            continue;
        }
        let Some((html, _mime)) = doc.get_resource_str(&id) else {
            continue;
        };
        let normalized = path_key(&resource_path);
        let in_toc = title_by_path.contains_key(&normalized);
        let heading = extract_heading(&html);
        // Front matter / auxiliary pages (title page, book TOC, blank
        // sheets) are not part of the reading order: per EPUB semantics the
        // TOC determines the chapter set. Docs without a TOC entry are
        // kept only when they carry a real heading of their own (some
        // sloppy books omit TOC entries for real chapters).
        if !in_toc && heading.is_none() {
            continue;
        }
        let content = clean_xhtml(&mut doc, &html, &resource_path);
        if !has_readable_content(&content) {
            continue;
        }

        let title = title_by_path
            .get(&normalized)
            .cloned()
            .or(heading)
            .unwrap_or_else(|| format!("第 {} 章", chapter_idx + 1));

        chapters.push(ParsedChapter {
            title,
            format: ChapterFormat::Html,
            content,
        });
        kept_spine.push(chapter_idx);
    }

    if chapters.is_empty() {
        return Err(Error::InvalidArgument(
            "epub has no readable chapters".into(),
        ));
    }

    // Map the TOC tree onto the final chapter list; synthesize a flat TOC
    // when the ebook ships no usable TOC.
    let mut toc = remap_branches(&branches, &spine_pos, &kept_spine, None);
    if toc.is_empty() {
        toc = chapters
            .iter()
            .enumerate()
            .map(|(i, c)| TocNode {
                title: c.title.clone(),
                idx: Some(i as u32),
                children: Vec::new(),
            })
            .collect();
    }

    Ok(ParsedBook {
        title,
        authors,
        description,
        cover_url: None, // cover extraction (bytes) is not stored yet
        chapters,
        toc,
    })
}

// ---- table of contents ------------------------------------------------

/// TOC tree branch while parsing; `path` is the container path of the
/// target document (`None` for pure group entries).
struct TocBranch {
    title: String,
    path: Option<PathBuf>,
    children: Vec<TocBranch>,
}

/// Extract the book's table of contents per the EPUB standard: the EPUB 3
/// nav document when present, otherwise the NCX `navMap`, finally the
/// flattened `EpubDoc` NCX parse. Returns the tree with hierarchy intact
/// (`playOrder` respected for NCX).
fn extract_toc_tree<R: Read + Seek>(doc: &mut EpubDoc<R>) -> Vec<TocBranch> {
    // EPUB 3: the nav document listed in the manifest with the `nav` property.
    if doc.version == epub::doc::EpubVersion::Version3_0 {
        if let Some(nav_id) = doc.get_nav_id() {
            if let Some((html, _)) = doc.get_resource_str(&nav_id) {
                let nav_path = doc
                    .resources
                    .get(&nav_id)
                    .map(|r| r.path.clone())
                    .unwrap_or_default();
                let branches = parse_nav_branches(&html, &nav_path);
                if !branches.is_empty() {
                    return branches;
                }
            }
        }
    }

    // EPUB 2: the NCX document from the manifest.
    let ncx_path = doc
        .resources
        .values()
        .find(|r| r.mime == NCX_MIME)
        .map(|r| r.path.clone());
    if let Some(ncx_path) = ncx_path {
        if let Some(xml) = doc.get_resource_str_by_path(&ncx_path) {
            let branches = parse_ncx_branches(&xml, &ncx_path);
            if !branches.is_empty() {
                return branches;
            }
        }
    }

    // Last resort: epub-rs' own NCX parse.
    navpoint_branches(&doc.toc)
}

/// Branch list of an EPUB 3 nav document. `<nav epub:type="toc">` navs win;
/// otherwise the first nav element is used. `href` values are relative to
/// the nav document's directory.
fn parse_nav_branches(html: &str, nav_path: &Path) -> Vec<TocBranch> {
    let frag = Html::parse_document(html);
    let nav = frag
        .select(&Selector::parse(r#"nav[epub\:type=toc]"#).expect("static selector"))
        .next()
        .or_else(|| {
            frag.select(&Selector::parse("nav").expect("static selector"))
                .next()
        });
    let Some(nav) = nav else {
        return Vec::new();
    };
    let base = nav_path.parent().unwrap_or(Path::new(""));
    collect_nav_branches(nav, base)
}

#[allow(clippy::only_used_in_recursion)]
fn collect_nav_branches(root: scraper::ElementRef<'_>, base: &Path) -> Vec<TocBranch> {
    let a_sel = Selector::parse("a[href]").expect("static selector");
    let mut out = Vec::new();
    // Iterate `li` at exactly one depth level: the direct `li` children of
    // a nested `ol` (`root` is the nav element or a `li`; recursion into a
    // `li` visits its sub-list).
    for li in root
        .child_elements()
        .filter(|e| e.value().name() == "ol")
        .flat_map(|ol| ol.child_elements())
        .filter(|e| e.value().name() == "li")
    {
        let (title, path) = match li.select(&a_sel).next() {
            Some(a) => {
                let t: String = a.text().collect::<String>().trim().to_string();
                let href = a.value().attr("href").unwrap_or("");
                if href.starts_with('#') {
                    (t, None)
                } else {
                    (t, Some(base.join(href)))
                }
            }
            None => {
                let t: String = li.text().collect::<String>().trim().to_string();
                (t, None)
            }
        };
        if title.is_empty() && path.is_none() {
            continue;
        }
        // A `li` may contain a nested `ol`; recurse into the `li` itself
        // (its sub-lists are its `ol` children).
        let children: Vec<TocBranch> = collect_nav_branches(li, base);
        out.push(TocBranch {
            title,
            path,
            children,
        });
    }
    out
}

/// Branch tree of an EPUB 2 NCX `navMap`. `src` values are relative to the
/// NCX document's directory; sibling order follows `playOrder`.
fn parse_ncx_branches(xml: &str, ncx_path: &Path) -> Vec<TocBranch> {
    let frag = Html::parse_fragment(xml);
    let base = ncx_path.parent().unwrap_or(Path::new(""));
    // The top-level `navPoint` elements live inside `navMap`; start there
    // so `collect_ncx_branches` sees exactly one level per call.
    let parent = frag
        .select(&Selector::parse("navmap").expect("static selector"))
        .next()
        .unwrap_or(frag.root_element());
    collect_ncx_branches(&parent, base)
}

fn collect_ncx_branches(parent: &scraper::ElementRef<'_>, base: &Path) -> Vec<TocBranch> {
    let navlabel_sel = Selector::parse("navlabel text").expect("static selector");
    let content_sel = Selector::parse("content").expect("static selector");
    let mut ordered: Vec<(usize, TocBranch)> = Vec::new();
    // NCX is XML, but html5ever parses it as HTML: `<content …/>` does not
    // self-close there, so nested `navPoint`s end up inside their
    // ancestor's `<content>`. Descendant selection still sees them;
    // entries whose nearest `navPoint` ancestor is not the recursion root
    // belong to that ancestor's subtree and are skipped at this level
    // (they are collected by the recursion below).
    for np in parent.select(&Selector::parse("navpoint").expect("static selector")) {
        if let Some(anc) = nearest_navpoint_ancestor(np) {
            if anc != *parent {
                continue;
            }
        }
        let order = np
            .value()
            .attr("playorder")
            .and_then(|o| o.parse::<usize>().ok())
            .unwrap_or(usize::MAX);
        let title: String = np
            .select(&navlabel_sel)
            .next()
            .map(|t| t.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let href = np
            .select(&content_sel)
            .next()
            .and_then(|c| c.value().attr("src"))
            .map(|s| s.to_string());
        let path = href.filter(|h| !h.starts_with('#')).map(|h| base.join(h));
        let children = collect_ncx_branches(&np, base);
        ordered.push((
            order,
            TocBranch {
                title,
                path,
                children,
            },
        ));
    }
    ordered.sort_by_key(|(o, _)| *o);
    ordered.into_iter().map(|(_, b)| b).collect()
}

/// Nearest ancestor element named `navpoint`, if any.
fn nearest_navpoint_ancestor(el: scraper::ElementRef<'_>) -> Option<scraper::ElementRef<'_>> {
    let mut cur = el.parent();
    while let Some(p) = cur {
        if let Node::Element(e) = p.value() {
            if e.name() == "navpoint" {
                return ElementRef::wrap(p);
            }
        }
        cur = p.parent();
    }
    None
}

/// Branch tree from epub-rs' parsed NCX (`NavPoint.content` is already
/// joined with the root base dir).
fn navpoint_branches(points: &[NavPoint]) -> Vec<TocBranch> {
    points
        .iter()
        .map(|p| TocBranch {
            title: p.label.clone(),
            path: Some(p.content.clone()),
            children: navpoint_branches(&p.children),
        })
        .collect()
}

/// Flatten a branch tree into `(path, label)` pairs, depth-first,
/// **children last** so document-level title lookups prefer the deepest
/// (most specific) entry for a document.
fn flatten_branches(branches: &[TocBranch], out: &mut Vec<(PathBuf, String)>) {
    for b in branches {
        if let Some(path) = &b.path {
            out.push((path.clone(), b.title.clone()));
        }
        flatten_branches(&b.children, out);
    }
}

/// Canonical key for container paths: normalized components joined with
/// `/`, tolerant of `.`/`..`/separator differences.
fn path_key(p: &Path) -> String {
    p.components()
        .filter_map(|c| match c {
            Component::CurDir | Component::RootDir | Component::Prefix(_) => None,
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Map the parsed branch tree onto the final chapter list.
///
/// - a branch whose document became a chapter keeps its title and gets the
///   chapter's index; fragment entries pointing into the same document as
///   an ancestor are dropped (our reader navigates per chapter);
/// - group branches (no target) are kept when they have surviving children;
/// - branches whose document was filtered out (front matter, image-only
///   pages, …) are spliced away, their children lifting up one level.
fn remap_branches(
    branches: &[TocBranch],
    spine_pos: &HashMap<String, usize>,
    kept: &[usize],
    parent_idx: Option<u32>,
) -> Vec<TocNode> {
    let mut out = Vec::new();
    for b in branches {
        let this_idx: Option<u32> = b.path.as_ref().and_then(|p| {
            let pos = spine_pos.get(&path_key(p))?;
            kept.binary_search(pos).ok().map(|i| i as u32)
        });
        match this_idx {
            None => {
                // Group entry (or its doc was dropped): keep children.
                out.extend(remap_branches(&b.children, spine_pos, kept, parent_idx));
            }
            Some(i) => {
                if parent_idx == Some(i) {
                    continue; // same-document fragment: already reachable
                }
                let children = remap_branches(&b.children, spine_pos, kept, Some(i));
                out.push(TocNode {
                    title: b.title.clone(),
                    idx: Some(i),
                    children,
                });
            }
        }
    }
    out
}

// ---- XHTML sanitization -----------------------------------------------

/// Normalize a spine XHTML document into a safe HTML fragment by walking
/// the parsed tree and re-serializing it from scratch:
///
/// - keeps structure (paragraphs, headings, lists, tables, ruby, …);
/// - drops scripts, styles, forms, foreign content and any event handler /
///   `style` attribute (whitelist approach);
/// - unknown tags are unwrapped (their text content survives);
/// - inlines referenced container images as `data:` URIs;
/// - drops cross-document links (chapter-local anchors and http(s)/mailto
///   links survive).
fn clean_xhtml<R: Read + Seek>(doc: &mut EpubDoc<R>, html: &str, doc_path: &Path) -> String {
    let parsed = Html::parse_document(html);
    let body = parsed
        .select(&Selector::parse("body").expect("static selector"))
        .next()
        .unwrap_or(parsed.root_element());
    let base = doc_path.parent().unwrap_or(Path::new(""));

    let mut out = String::with_capacity(html.len());
    for child in body.children() {
        sanitize_node(&mut out, &child, doc, base);
    }
    out
}

fn sanitize_node<R: Read + Seek>(
    out: &mut String,
    node: &NodeRef<'_, scraper::node::Node>,
    doc: &mut EpubDoc<R>,
    base: &Path,
) {
    match node.value() {
        Node::Text(t) => escape_html(out, &t.text),
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
                    let Some(value) = rewrite_attr(doc, base, &tag, &name, value) else {
                        continue;
                    };
                    out.push(' ');
                    out.push_str(&name);
                    out.push_str("=\"");
                    escape_html(out, &value);
                    out.push('"');
                }
                out.push('>');
            }
            for child in node.children() {
                sanitize_node(out, &child, doc, base);
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

/// Rewrite an attribute value per tag semantics. Returns `None` when the
/// attribute must be dropped.
fn rewrite_attr<R: Read + Seek>(
    doc: &mut EpubDoc<R>,
    base: &Path,
    tag: &str,
    name: &str,
    value: &str,
) -> Option<String> {
    match (tag, name) {
        ("img", "src") => inline_image(doc, base, value),
        ("a", "href") if !keep_link(value) => None,
        _ => Some(value.to_string()),
    }
}

/// Resolve a (possibly relative) epub image reference against the current
/// document's directory and inline it as a `data:` URI. Non-container
/// schemes (http, …) are left untouched.
fn inline_image<R: Read + Seek>(doc: &mut EpubDoc<R>, base: &Path, src: &str) -> Option<String> {
    if src.starts_with("data:") {
        return Some(src.to_string());
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return None; // don't leak the reader's IP to third-party hosts
    }
    let path = if let Some(rel) = src.strip_prefix('/') {
        // Container-root-relative (non-standard but seen in the wild).
        PathBuf::from(rel)
    } else {
        base.join(src)
    };
    let key = path_key(&path);
    let Some(bytes) = doc.get_resource_by_path(&key) else {
        return None; // broken reference: drop the image rather than a 404 icon
    };
    let mime = doc
        .get_resource_mime_by_path(&key)
        .unwrap_or_else(|| "image/png".to_string());
    // Restrict to raster images we can render inline.
    if !(mime.starts_with("image/") && !mime.contains("svg")) {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

/// Which `href` values survive sanitization: in-page anchors and
/// http(s)/mailto links (opened by the reader in a new tab).
fn keep_link(href: &str) -> bool {
    let h = href.trim();
    h.starts_with('#')
        || h.starts_with("http://")
        || h.starts_with("https://")
        || h.starts_with("mailto:")
}

/// Escape a text/attribute value for safe inclusion in HTML.
fn escape_html(out: &mut String, s: &str) {
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

/// Does the sanitized fragment carry meaningful content? Image-only pages
/// (illustration plates — 插图/扉页) are auxiliary material, not chapters:
/// a chapter must contain readable text.
fn has_readable_content(html: &str) -> bool {
    let frag = Html::parse_document(html);
    let root = frag.root_element();
    let text: String = root.text().collect();
    !text.trim().is_empty()
}

/// Best-effort chapter heading extraction: the first non-empty heading in
/// document order (per EPUB semantics the chapter title is the first
/// heading element of the document).
fn extract_heading(html: &str) -> Option<String> {
    let frag = Html::parse_document(html);
    let Ok(selector) = Selector::parse("h1, h2, h3, h4, h5, h6") else {
        return None;
    };
    for el in frag.select(&selector) {
        let text: String = el.text().collect();
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    struct Fixture {
        version: &'static str,
        spine: Vec<(String, bool)>, // (file name, linear)
        /// (label, file name, children) — children are nested TOC entries.
        toc: Vec<(String, String, Vec<(String, String)>)>,
    }

    /// Minimal but spec-shaped EPUB: container.xml + OPF + (NCX or nav) +
    /// XHTML chapter documents. Written with Stored entries (no compression).
    fn build_epub(fx: &Fixture) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            let add = |w: &mut ZipWriter<_>, path: &str, data: &str| {
                w.start_file(path, opts).unwrap();
                w.write_all(data.as_bytes()).unwrap();
            };
            add(&mut w, "mimetype", "application/epub+zip");
            add(
                &mut w,
                "META-INF/container.xml",
                r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
            );

            {
                w.start_file("OEBPS/images/pic.png", opts).unwrap();
                w.write_all(PNG_1PX).unwrap();
            }

            let mut manifest = String::new();
            let mut spine = String::new();
            for (i, (file, linear)) in fx.spine.iter().enumerate() {
                manifest.push_str(&format!(
                    r#"<item id="c{i}" href="{file}" media-type="application/xhtml+xml"/>"#
                ));
                spine.push_str(&format!(
                    r#"<itemref idref="c{i}"{} />"#,
                    if *linear { "" } else { r#" linear="no""# }
                ));
            }
            let toc_file = if fx.version == "3.0" {
                "nav.xhtml"
            } else {
                "toc.ncx"
            };
            if fx.version == "3.0" {
                manifest.push_str(&format!(
                    r#"<item id="nav" href="{toc_file}" media-type="application/xhtml+xml" properties="nav"/>"#
                ));
            } else {
                manifest.push_str(&format!(
                    r#"<item id="ncx" href="{toc_file}" media-type="application/x-dtbncx+xml"/>"#
                ));
            }
            let spine_xml = if fx.version == "3.0" {
                format!(r#"<spine>{spine}</spine>"#)
            } else {
                // EPUB 2: spine declares its TOC via the `toc` attribute.
                format!(r#"<spine toc="ncx">{spine}</spine>"#)
            };
            add(
                &mut w,
                "OEBPS/content.opf",
                &format!(
                    r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="{v}" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">test-{v}</dc:identifier>
    <dc:title>测试书</dc:title>
    <dc:creator>测试作者</dc:creator>
  </metadata>
  <manifest>{manifest}</manifest>
  {spine_xml}
</package>"#,
                    v = fx.version,
                ),
            );

            if fx.version == "3.0" {
                let mut nav_items = String::new();
                for (label, file, children) in &fx.toc {
                    nav_items.push_str(&format!(r#"<li><a href="{file}">{label}</a>"#));
                    if !children.is_empty() {
                        nav_items.push_str("<ol>");
                        for (clabel, cfile) in children {
                            nav_items
                                .push_str(&format!(r#"<li><a href="{cfile}">{clabel}</a></li>"#));
                        }
                        nav_items.push_str("</ol>");
                    }
                    nav_items.push_str("</li>");
                }
                add(
                    &mut w,
                    "OEBPS/nav.xhtml",
                    &format!(
                        r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>目录</title></head>
<body><nav epub:type="toc"><ol>{nav_items}</ol></nav></body>
</html>"#
                    ),
                );
            } else {
                let mut nav_points = String::new();
                let mut order = 0usize;
                for (label, file, children) in &fx.toc {
                    nav_points.push_str(&format!(
                        r#"<navPoint id="np{order}" playOrder="{order}"><navLabel><text>{label}</text></navLabel><content src="{file}"/>"#
                    ));
                    order += 1;
                    for (clabel, cfile) in children {
                        nav_points.push_str(&format!(
                            r#"<navPoint id="np{order}" playOrder="{order}"><navLabel><text>{clabel}</text></navLabel><content src="{cfile}"/></navPoint>"#
                        ));
                        order += 1;
                    }
                    nav_points.push_str("</navPoint>");
                }
                add(
                    &mut w,
                    "OEBPS/toc.ncx",
                    &format!(
                        r#"<?xml version="1.0"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>{nav_points}</navMap>
</ncx>"#
                    ),
                );
            }

            let chapter_body = r##"<body>
  <script>alert("xss")</script>
  <h1>正文标题 {i}</h1>
  <p>第一段，内有<ruby>注音<rt>zhùyīn</rt></ruby>和<strong>强调</strong>。</p>
  <p><img src="images/pic.png" alt="插图"/></p>
  <p onclick="alert(1)" style="color:red">干净段落</p>
  <a href="#sec1">段内锚点</a> <a href="ch2.xhtml">跨章链接</a> <a href="https://example.com">外部</a>
  <svg><circle r="1"/><text>svg 文本</text></svg>
  <table><tr><th>列</th></tr><tr><td>值</td></tr></table>
</body>"##;
            for (i, (file, _)) in fx.spine.iter().enumerate() {
                let doc = if file.starts_with("pic") {
                    // Illustration plate: image only, no text.
                    r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>插图</title></head><body><p><img src="images/pic.png" alt="插图"/></p></body></html>"#
                } else if file.starts_with("cover") {
                    r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>封面</title></head><body><p>封面图片</p></body></html>"#
                } else if file.starts_with("title") {
                    // Front matter page: linked in the spine but not in the
                    // TOC, no heading of its own.
                    r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>书名页</title></head><body><p>测试书</p><p>测试作者 著</p></body></html>"#
                } else {
                    &format!(
                        r###"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>章节</title></head>{chapter_body}"###
                    )
                };
                let doc = doc.replace("{i}", &i.to_string());
                add(&mut w, &format!("OEBPS/{file}"), &doc);
            }
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn parses_epub2_spine_toc_and_html() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            spine: vec![
                ("cover.xhtml".into(), false),
                ("title.xhtml".into(), true), // front matter: no TOC entry, no heading
                ("ch1.xhtml".into(), true),
                ("ch2.xhtml".into(), true),
                ("ch2b.xhtml".into(), true),
            ],
            toc: vec![
                ("第一章 起点".into(), "ch1.xhtml".into(), vec![]),
                (
                    "第二章 风起".into(),
                    "ch2.xhtml".into(),
                    vec![("第二章之二".into(), "ch2b.xhtml".into())],
                ),
            ],
        });
        let book = parse(&bytes).unwrap();
        eprintln!(
            "DBG toc={:?}",
            book.toc
                .iter()
                .map(|n| (
                    n.title.clone(),
                    n.idx,
                    n.children.len(),
                    n.children
                        .iter()
                        .map(|c| c.title.clone())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );

        // Cover (linear="no") and unlinked front matter must not become
        // chapters; spine order kept.
        assert_eq!(book.chapters.len(), 3);
        assert_eq!(book.chapters[0].title, "第一章 起点");
        assert_eq!(book.chapters[1].title, "第二章 风起");
        assert_eq!(book.chapters[2].title, "第二章之二");

        // TOC stays a tree: 第二章 风起 groups its 第二章之二 child; the
        // same-document fragment entry is deduped against its parent.
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].title, "第一章 起点");
        assert_eq!(book.toc[0].idx, Some(0));
        assert!(book.toc[0].children.is_empty());
        assert_eq!(book.toc[1].title, "第二章 风起");
        assert_eq!(book.toc[1].idx, Some(1));
        assert_eq!(book.toc[1].children.len(), 1);
        assert_eq!(book.toc[1].children[0].title, "第二章之二");
        assert_eq!(book.toc[1].children[0].idx, Some(2));

        // HTML structure preserved, junk stripped.
        let c0 = &book.chapters[0];
        assert_eq!(c0.format, ChapterFormat::Html);
        assert!(c0.content.contains("<p>第一段"));
        assert!(c0.content.contains("<ruby>注音<rt>zhùyīn</rt></ruby>"));
        assert!(c0.content.contains("<strong>强调</strong>"));
        assert!(!c0.content.contains("<script"));
        assert!(!c0.content.contains("onclick"));
        assert!(!c0.content.contains("style="));
        assert!(!c0.content.contains("svg"));
        assert!(c0.content.contains("<table>"));

        // Inline image becomes a data URI.
        assert!(c0.content.contains("data:image/png;base64,"));
        // Links: in-page anchor and http survive; cross-chapter href is
        // dropped (the reader renders one chapter at a time).
        assert!(c0.content.contains(r##"<a href="#sec1">"##));
        assert!(c0
            .content
            .contains(r#"<a href="https://example.com">外部</a>"#));
        assert!(c0.content.contains("<a>跨章链接</a>"));
    }

    #[test]
    fn parses_epub3_nav_toc() {
        let bytes = build_epub(&Fixture {
            version: "3.0",
            spine: vec![
                ("pic.xhtml".into(), true), // image-only 插图 page, linked in TOC
                ("ch1.xhtml".into(), true),
                ("ch2.xhtml".into(), true),
                ("ch2b.xhtml".into(), true),
            ],
            toc: vec![
                ("插图".into(), "pic.xhtml".into(), vec![]),
                ("序章".into(), "ch1.xhtml".into(), vec![]),
                (
                    "第二卷".into(),
                    "ch2.xhtml".into(),
                    vec![("终章".into(), "ch2b.xhtml".into())],
                ),
            ],
        });
        let book = parse(&bytes).unwrap();
        // Image-only pages never become chapters, nor TOC entries.
        assert_eq!(book.chapters.len(), 3);
        assert_eq!(book.chapters[0].title, "序章");
        assert_eq!(book.chapters[1].title, "第二卷");
        assert_eq!(book.chapters[2].title, "终章");
        // Nested nav items keep their hierarchy.
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].title, "序章");
        assert_eq!(book.toc[0].idx, Some(0));
        assert_eq!(book.toc[1].title, "第二卷");
        assert_eq!(book.toc[1].idx, Some(1));
        assert_eq!(book.toc[1].children.len(), 1);
        assert_eq!(book.toc[1].children[0].title, "终章");
        assert_eq!(book.toc[1].children[0].idx, Some(2));
        assert!(book.chapters[0].content.contains("<h1>正文标题 1</h1>"));
        assert!(book.chapters[0].content.contains("data:image/png;base64,"));
    }

    #[test]
    fn falls_back_to_first_heading_without_toc() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            spine: vec![("ch1.xhtml".into(), true)],
            toc: vec![],
        });
        let book = parse(&bytes).unwrap();
        assert_eq!(book.chapters.len(), 1);
        assert_eq!(book.chapters[0].title, "正文标题 0");
        // No usable TOC → flat synthesized TOC from the chapters.
        assert_eq!(book.toc.len(), 1);
        assert_eq!(book.toc[0].idx, Some(0));
        assert_eq!(book.toc[0].title, "正文标题 0");
    }

    #[test]
    fn rejects_non_epub() {
        let err = parse(b"not a zip").unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }
}
