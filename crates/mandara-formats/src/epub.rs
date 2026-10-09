//! EPUB parsing per the EPUB standard (OPF spine + NCX/nav TOC).
//!
//! What this does, and why:
//!
//! - **Reading order = OPF spine** ([EPUB 3.3 § 3.17](https://www.w3.org/TR/epub-33/#sec-spine));
//!   items with `linear="no"` (cover pages, nav docs, …) are auxiliary and
//!   remain addressable auxiliary documents outside the default reading order.
//! - **Chapter titles come from the table of contents**: the EPUB 3 nav
//!   document (`<nav epub:type="toc">`) or, for EPUB 2, the NCX `navMap`.
//!   Entries map to spine documents by resource path; documents without a
//!   TOC entry fall back to their first heading, then to a numbered title.
//! - **Passive presentation is preserved**: chapters are full sanitized
//!   documents for isolated script-free frames. CSS imports and resource URLs
//!   are resolved against their containing documents; extracted images use
//!   ingest placeholders and CSS resources are embedded passive data URLs.
//! - Scripts, forms, event handlers and remote resources are blocked. SVG,
//!   MathML, document semantics and publication styles remain supported.
//!   The audit entry point reports blocked and unsupported behavior.

use crate::epub_uri::{Reference, resolve};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use crate::ParsedImage;
use epub::doc::{EpubDoc, NavPoint};
use mandara_core::error::{Error, Result};
use mandara_core::ext::BookExt;
use mandara_core::model::TocNode;
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};

pub use crate::epub_render::Diagnostic;
use crate::epub_render::Renderer;
use crate::{CoverImage, ParsedBook, ParsedChapter};

/// XHTML / HTML content types accepted as spine documents.
const XHTML_MIMES: [&str; 2] = ["application/xhtml+xml", "text/html"];
/// Manifest media type of an EPUB 2 NCX TOC document.
const NCX_MIME: &str = "application/x-dtbncx+xml";

struct SpineDocument {
    spine_idx: usize,
    resource_path: PathBuf,
    content: String,
    heading: Option<String>,
    linear: bool,
    layout: String,
}

/// Parse EPUB bytes using the safe passive-presentation policy.
pub fn parse(bytes: &[u8]) -> Result<ParsedBook> {
    parse_with_diagnostics(bytes).map(|(book, _)| book)
}

/// Read-only audit entry point: distinguish broken references, unsupported
/// media and blocked active content from publication content that is retained.
pub fn parse_with_diagnostics(bytes: &[u8]) -> Result<(ParsedBook, Vec<Diagnostic>)> {
    parse_with_policy(bytes, ParsePolicy::Compatible)
}

/// Strict reference interpretation or bounded, diagnostic producer recovery.
#[derive(Clone, Copy, Debug)]
pub enum ParsePolicy {
    Strict,
    Compatible,
}

pub fn parse_with_policy(
    bytes: &[u8],
    policy: ParsePolicy,
) -> Result<(ParsedBook, Vec<Diagnostic>)> {
    let mut diagnostics = Vec::new();
    let mut container = crate::epub_container::Container::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::InvalidArgument(format!("invalid epub container: {e}")))?;
    let mut doc = epub::doc::EpubDoc::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| Error::InvalidArgument(format!("{e}")))?;

    // epub-rs stores manifest hrefs as filesystem joins. Canonicalize them
    // once as URLs before any resource access (including covers and nav).
    for resource in doc.resources.values_mut() {
        if let Some(reference) = resolve(Path::new(""), &resource.path.to_string_lossy()) {
            resource.path = PathBuf::from(reference.path);
        }
    }

    let title = doc.get_title().unwrap_or_else(|| "Untitled".into());

    let mut authors = Vec::new();
    for creator in doc
        .metadata
        .iter()
        .filter(|m| m.property.eq_ignore_ascii_case("creator"))
    {
        let roles: Vec<_> = creator
            .refined
            .iter()
            .filter(|r| r.property.eq_ignore_ascii_case("role"))
            .collect();
        let name = creator.value.trim();
        let author = roles.is_empty()
            || roles
                .iter()
                .any(|r| matches!(r.value.to_ascii_lowercase().as_str(), "aut" | "author"));
        if author && !name.is_empty() && !authors.iter().any(|a| a == name) {
            authors.push(name.to_owned());
        }
    }

    let description = doc.mdata("description").map(|m| m.value.trim().to_string());

    // Extended metadata from the OPF dc elements (bangumi/douban-style;
    // docs/metadata-ext-design.md). Values are stored verbatim except the
    // ISBN, which is validated + canonicalized; anything unparseable or
    // invalid is skipped — a broken field never fails the upload.
    let ext = extract_opf_ext(&doc);

    // TOC (nav doc / NCX) → tree, with a flattened path → label map for
    // chapter titles (leaf entries win for documents with nested entries).
    let references = crate::epub_reference::ReferenceResolver::new(
        doc.root_file.clone(),
        doc.resources
            .values()
            .map(|r| (path_key(&r.path), r.mime.clone())),
        matches!(policy, ParsePolicy::Compatible),
        &mut diagnostics,
    );
    let mut branches = extract_toc_tree(&mut doc, &mut container);
    recover_navigation(&mut branches, &references, &container, &mut diagnostics);
    let mut flat = Vec::new();
    flatten_branches(&branches, &mut flat);
    for (reference, _) in &flat {
        if doc.get_resource_mime_by_path(&reference.path).is_none() {
            diagnostics.push(Diagnostic {
                document: path_key(&doc.root_file),
                reference: Some(reference.path.clone()),
                reason: "missing-navigation-resource",
            });
        }
    }
    for item in &doc.spine {
        if !doc.resources.contains_key(&item.idref) {
            diagnostics.push(Diagnostic {
                document: path_key(&doc.root_file),
                reference: Some(item.idref.clone()),
                reason: "undefined-spine-idref",
            });
        }
    }
    let mut title_by_path: HashMap<String, String> = flat
        .iter()
        .filter(|(p, _)| p.fragment.is_none())
        .map(|(p, l)| (p.path.clone(), l.clone()))
        .collect();
    // Fragment-only TOCs still name their containing document. Prefer the
    // first section only when no document-level label is available.
    for (reference, label) in flat {
        title_by_path.entry(reference.path).or_insert(label);
    }

    // Spine order is the reading order; `linear="no"` items (cover, nav,
    // acknowledgements, …) remain addressable outside the default order.
    let package = container.text(&doc.root_file);
    let fallback_ids: HashMap<String, String> = package
        .as_deref()
        .and_then(|s| roxmltree::Document::parse(s).ok())
        .map(|xml| {
            xml.descendants()
                .filter(|n| n.has_tag_name("item"))
                .filter_map(|n| {
                    Some((
                        n.attribute("id")?.to_owned(),
                        n.attribute("fallback")?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut aliases = HashMap::new();
    let mut fallback_spine = Vec::new();
    for item in &doc.spine {
        let mut id = item.idref.clone();
        let mut visited = std::collections::HashSet::new();
        while let Some(resource) = doc.resources.get(&id) {
            if XHTML_MIMES.contains(&resource.mime.as_str()) || resource.mime == "image/svg+xml" {
                break;
            }
            if !visited.insert(id.clone()) {
                diagnostics.push(Diagnostic {
                    document: path_key(&doc.root_file),
                    reference: Some(item.idref.clone()),
                    reason: "cyclic-manifest-fallback",
                });
                break;
            }
            let Some(fallback) = fallback_ids.get(&id) else {
                break;
            };
            let Some(target) = doc.resources.get(fallback) else {
                diagnostics.push(Diagnostic {
                    document: path_key(&doc.root_file),
                    reference: Some(fallback.clone()),
                    reason: "missing-manifest-fallback",
                });
                break;
            };
            aliases.insert(path_key(&resource.path), path_key(&target.path));
            id = fallback.clone();
        }
        let mut resolved = item.clone();
        resolved.idref = id;
        fallback_spine.push(resolved);
    }
    let spine: Vec<(String, String, PathBuf, String, bool)> = fallback_spine
        .iter()
        .filter_map(|s| {
            doc.resources.get(&s.idref).map(|r| {
                let override_layout = s
                    .properties
                    .as_deref()
                    .unwrap_or("")
                    .split_ascii_whitespace()
                    .find_map(|p| p.strip_prefix("rendition:layout-"));
                let layout = override_layout
                    .or_else(|| doc.mdata("rendition:layout").map(|m| m.value.as_str()))
                    .unwrap_or("reflowable")
                    .to_owned();
                (
                    s.idref.clone(),
                    r.mime.clone(),
                    r.path.clone(),
                    layout,
                    s.linear,
                )
            })
        })
        .collect();
    let mut spine_pos: HashMap<String, usize> = spine
        .iter()
        .enumerate()
        .map(|(i, (_, _, p, _, _))| (path_key(p), i))
        .collect();

    // Embedded images, in first-reference order; the same container
    // resource referenced from several documents becomes one entry.
    let mut images: Vec<ParsedImage> = Vec::new();
    let mut seen_images: HashMap<String, usize> = HashMap::new();
    let mut documents = Vec::new();
    for (spine_idx, (_id, mime, resource_path, layout, linear)) in spine.iter().cloned().enumerate()
    {
        if !XHTML_MIMES.contains(&mime.as_str()) && mime != "image/svg+xml" {
            diagnostics.push(Diagnostic {
                document: path_key(&resource_path),
                reference: Some(mime),
                reason: "unsupported-spine-media-type",
            });
            continue;
        }
        let Some(html) = container.text(&resource_path) else {
            diagnostics.push(Diagnostic {
                document: path_key(&resource_path),
                reference: None,
                reason: if container.contains(&resource_path) {
                    "invalid-spine-text-encoding"
                } else {
                    "missing-spine-resource"
                },
            });
            continue;
        };
        let content = html.clone();
        documents.push(SpineDocument {
            spine_idx,
            resource_path,
            content,
            heading: extract_heading(&html),
            linear,
            layout,
        });
    }

    // Preserve explicitly linked publication documents outside the spine as
    // auxiliary resources. Navigation documents can legally live there.
    let mut queue: Vec<Reference> = title_by_path
        .keys()
        .map(|path| Reference {
            path: path.clone(),
            query: None,
            fragment: None,
        })
        .collect();
    let selector = Selector::parse("a[href]").expect("static selector");
    let mut scanned = 0;
    let mut known: std::collections::HashSet<String> = documents
        .iter()
        .map(|d| path_key(&d.resource_path))
        .collect();
    loop {
        while scanned < documents.len() {
            let d = &documents[scanned];
            let parsed = Html::parse_document(&d.content);
            queue.extend(parsed.select(&selector).filter_map(|a| {
                a.value().attr("href").and_then(|h| {
                    references.resolve(&container, &d.resource_path, h, &mut diagnostics)
                })
            }));
            scanned += 1;
        }
        if queue.is_empty() {
            break;
        }
        queue.sort_by(|a, b| a.path.cmp(&b.path));
        let references = std::mem::take(&mut queue);
        for reference in references {
            if known.contains(&reference.path) {
                continue;
            }
            let Some(mime) = doc.get_resource_mime_by_path(&reference.path) else {
                continue;
            };
            if !XHTML_MIMES.contains(&mime.as_str()) && mime != "image/svg+xml" {
                continue;
            }
            known.insert(reference.path.clone());
            let Some(html) = container.text(Path::new(&reference.path)) else {
                continue;
            };
            let pos = spine.len() + documents.len();
            spine_pos.insert(reference.path.clone(), pos);
            documents.push(SpineDocument {
                spine_idx: pos,
                resource_path: PathBuf::from(reference.path),
                heading: extract_heading(&html),
                content: html,
                linear: false,
                layout: "reflowable".into(),
            });
        }
    }
    // Auxiliary resources remain addressable but never join the default order.
    documents.sort_by_key(|d| !d.linear);
    let mut chapter_by_path: HashMap<String, u32> = documents
        .iter()
        .enumerate()
        .map(|(idx, d)| (path_key(&d.resource_path), idx as u32))
        .collect();
    // Repeatedly propagate chained foreign-resource aliases onto the final
    // supported document; unresolved/cyclic chains stay diagnostic failures.
    for _ in 0..aliases.len() {
        let mut ordered: Vec<_> = aliases.iter().collect();
        ordered.sort_by(|a, b| a.0.cmp(b.0));
        for (source, target) in ordered {
            if let Some(title) = title_by_path.get(source).cloned() {
                title_by_path.entry(target.clone()).or_insert(title);
            }
            if let Some(idx) = chapter_by_path.get(target).copied() {
                chapter_by_path.insert(source.clone(), idx);
            }
            if let Some(pos) = spine_pos.get(target).copied() {
                spine_pos.insert(source.clone(), pos);
            }
        }
    }
    let mut chapters = Vec::new();
    let mut kept_spine = Vec::new(); // spine positions that became chapters
    for document in documents {
        let normalized = path_key(&document.resource_path);
        let title = title_by_path
            .get(&normalized)
            .cloned()
            .or(document.heading)
            .unwrap_or_else(|| format!("第 {} 章", chapters.len() + 1));

        chapters.push(ParsedChapter {
            title,
            linear: document.linear,
            content: Renderer::new(
                &mut doc,
                &mut container,
                &mut images,
                &mut seen_images,
                &mut diagnostics,
            )
            .with_targets(&chapter_by_path)
            .with_references(&references)
            .document(&document.content, &document.resource_path, &document.layout),
        });
        kept_spine.push(document.spine_idx);
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
            .filter(|(_, c)| c.linear)
            .map(|(i, c)| TocNode {
                title: c.title.clone(),
                idx: Some(i as u32),
                frag: None,
                children: Vec::new(),
            })
            .collect();
    }

    // Cover image: EPUB3 `cover-image` property, EPUB2 `<meta name="cover">`.
    let cover = doc
        .get_cover_id()
        .and_then(|id| doc.resources.get(&id))
        .and_then(|resource| {
            container
                .bytes(&resource.path)
                .map(|bytes| (bytes, resource.mime.clone()))
        })
        .filter(|(bytes, _)| !bytes.is_empty())
        .map(|(bytes, mime)| CoverImage { bytes, mime });

    Ok((
        ParsedBook {
            title,
            authors,
            description,
            cover_url: None,
            cover,
            images,
            chapters,
            toc,
            ext,
        },
        diagnostics,
    ))
}

/// Extended metadata from the OPF's Dublin Core elements: ISBN (first
/// `dc:identifier` that validates, `urn:isbn:` prefix allowed), publisher,
/// date, language, and translator-credited creators (`opf:role="trl"` /
/// EPUB3 `role` refinement).
fn extract_opf_ext<R: Read + Seek>(doc: &epub::doc::EpubDoc<R>) -> BookExt {
    let mut ext = BookExt::default();
    let md = &doc.metadata;
    let by_property = |name: &str| {
        md.iter()
            .find(|d| d.property.eq_ignore_ascii_case(name))
            .map(|d| d.value.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    ext.publisher = by_property("publisher");
    ext.pub_date = by_property("date");
    ext.language = by_property("language");
    for ident in md
        .iter()
        .filter(|d| d.property.eq_ignore_ascii_case("identifier"))
    {
        let raw = ident.value.trim();
        let candidate = raw
            .strip_prefix("urn:isbn:")
            .or_else(|| raw.strip_prefix("URN:ISBN:"))
            .unwrap_or(raw);
        if let Ok(isbn) = mandara_core::ext::normalize_isbn(candidate) {
            ext.isbn = Some(isbn);
            break;
        }
    }
    let translators: Vec<String> = md
        .iter()
        .filter(|d| {
            d.property.eq_ignore_ascii_case("creator")
                || d.property.eq_ignore_ascii_case("contributor")
        })
        .filter(|d| {
            d.refined.iter().any(|r| {
                r.property.eq_ignore_ascii_case("role")
                    && matches!(r.value.to_lowercase().as_str(), "trl" | "translator")
            })
        })
        .map(|d| d.value.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    ext.translators = (!translators.is_empty()).then_some(translators);
    let mut illustrators = Vec::new();
    let mut contributors = Vec::new();
    for item in md.iter().filter(|d| {
        d.property.eq_ignore_ascii_case("creator") || d.property.eq_ignore_ascii_case("contributor")
    }) {
        let name = item.value.trim();
        if name.is_empty() {
            continue;
        }
        let roles: Vec<_> = item
            .refined
            .iter()
            .filter(|r| r.property.eq_ignore_ascii_case("role"))
            .collect();
        if roles
            .iter()
            .any(|r| matches!(r.value.to_ascii_lowercase().as_str(), "ill" | "illustrator"))
            && !illustrators.iter().any(|i| i == name)
        {
            illustrators.push(name.to_owned());
        }
        contributors.push(serde_json::json!({ "name": name, "kind": item.property,
            "language": item.lang, "roles": roles.iter().map(|r| serde_json::json!({ "value": r.value, "scheme": r.scheme })).collect::<Vec<_>>() }));
    }
    ext.illustrators = (!illustrators.is_empty()).then_some(illustrators);
    if !contributors.is_empty() {
        ext.extra.insert(
            "epub_contributors".into(),
            serde_json::Value::Array(contributors),
        );
    }
    let _ = ext.sanitize();
    ext
}

// ---- table of contents ------------------------------------------------

/// TOC tree branch while parsing; `path` is the container path of the
/// target document (`None` for pure group entries).
struct TocBranch {
    title: String,
    path: Option<Reference>,
    source: Option<(PathBuf, String)>,
    children: Vec<TocBranch>,
}

/// Extract the book's table of contents per the EPUB standard: the EPUB 3
/// nav document when present, otherwise the NCX `navMap`, finally the
/// flattened `EpubDoc` NCX parse. Returns the tree with hierarchy intact
/// (`playOrder` respected for NCX).
fn extract_toc_tree<R: Read + Seek>(
    doc: &mut EpubDoc<R>,
    container: &mut crate::epub_container::Container<R>,
) -> Vec<TocBranch> {
    // EPUB 3: the nav document listed in the manifest with the `nav` property.
    if doc.version == epub::doc::EpubVersion::Version3_0
        && let Some(nav_id) = doc.get_nav_id()
        && let Some(html) = doc
            .resources
            .get(&nav_id)
            .and_then(|r| container.text(&r.path))
    {
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

    // EPUB 2 selects the NCX through spine@toc, not HashMap iteration.
    let package = container.text(&doc.root_file);
    let toc_id = package
        .as_deref()
        .and_then(|xml| roxmltree::Document::parse(xml).ok())
        .and_then(|xml| {
            xml.descendants()
                .find(|n| n.has_tag_name("spine"))
                .and_then(|n| n.attribute("toc"))
                .map(str::to_owned)
        });
    let ncx_path = toc_id
        .as_ref()
        .and_then(|id| doc.resources.get(id))
        .filter(|r| r.mime == NCX_MIME)
        .map(|r| r.path.clone())
        .or_else(|| {
            doc.resources
                .values()
                .filter(|r| r.mime == NCX_MIME)
                .map(|r| r.path.clone())
                .min()
        });
    if let Some(ncx_path) = ncx_path
        && let Some(xml) = container.text(&ncx_path)
    {
        let branches = parse_ncx_branches(&xml, &ncx_path);
        if !branches.is_empty() {
            return branches;
        }
    }

    // Last resort: epub-rs' own NCX parse.
    navpoint_branches(&doc.toc)
}

/// Select the EPUB 3 TOC by namespace and semantic token list. A typed
/// landmarks/page-list navigation is never a substitute for the TOC.
fn parse_nav_branches(html: &str, nav_path: &Path) -> Vec<TocBranch> {
    if let Ok(xml) = roxmltree::Document::parse(html) {
        let nav = xml.descendants().find(|n| {
            n.has_tag_name(("http://www.w3.org/1999/xhtml", "nav"))
                && n.attribute(("http://www.idpf.org/2007/ops", "type"))
                    .is_some_and(|v| v.split_ascii_whitespace().any(|v| v == "toc"))
        });
        let nav = nav.or_else(|| {
            xml.descendants().find(|n| {
                n.has_tag_name("nav")
                    && n.attribute(("http://www.idpf.org/2007/ops", "type"))
                        .is_none()
            })
        });
        return nav
            .map(|n| collect_xml_nav(n, nav_path))
            .unwrap_or_default();
    }
    // Recovery for producer HTML that is not namespace-well-formed XML.
    let parsed = Html::parse_document(html);
    let selector = Selector::parse("nav").expect("static selector");
    let navs: Vec<_> = parsed.select(&selector).collect();
    let nav = navs
        .iter()
        .copied()
        .find(|n| {
            n.value()
                .attr("epub:type")
                .is_some_and(|v| v.split_ascii_whitespace().any(|v| v == "toc"))
        })
        .or_else(|| {
            navs.iter()
                .copied()
                .find(|n| n.value().attr("epub:type").is_none())
        });
    nav.map(|nav| collect_nav_branches(nav, nav_path))
        .unwrap_or_default()
}

fn xml_text(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn collect_xml_nav(root: roxmltree::Node<'_, '_>, base: &Path) -> Vec<TocBranch> {
    root.children()
        .filter(|n| n.has_tag_name("ol"))
        .flat_map(|ol| ol.children().filter(|n| n.has_tag_name("li")))
        .map(|li| {
            let own = li
                .children()
                .take_while(|n| !n.has_tag_name("ol"))
                .find_map(|n| {
                    n.descendants()
                        .find(|n| n.has_tag_name("a") && n.attribute("href").is_some())
                });
            let (title, path, source) = if let Some(a) = own {
                (
                    xml_text(a),
                    a.attribute("href").and_then(|h| resolve(base, h)),
                    a.attribute("href")
                        .map(|h| (base.to_path_buf(), h.to_owned())),
                )
            } else {
                (
                    li.children()
                        .filter(|n| !n.has_tag_name("ol"))
                        .map(xml_text)
                        .collect::<String>(),
                    None,
                    None,
                )
            };
            TocBranch {
                title,
                path,
                source,
                children: collect_xml_nav(li, base),
            }
        })
        .collect()
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
        // The entry's own link is the first `a[href]` before the nested
        // list: a direct-child `a`, or one inside a non-`ol` wrapper.
        // (Searching all descendants would grab the first *child* entry's
        // link for href-less group `<li>`s.)
        let own_link = li
            .child_elements()
            .take_while(|e| e.value().name() != "ol")
            .find_map(|e| {
                if e.value().name() == "a" {
                    Some(e)
                } else {
                    // `select` excludes the element itself; a link inside
                    // a non-list wrapper is still this entry's own link.
                    e.select(&a_sel).next()
                }
            });
        let (title, path, source) = match own_link {
            Some(a) => {
                let t: String = a.text().collect::<String>().trim().to_string();
                let href = a.value().attr("href").unwrap_or("");
                (
                    t,
                    resolve(base, href),
                    Some((base.to_path_buf(), href.to_owned())),
                )
            }
            None => {
                // No link: a group entry. Take the li's own text but
                // exclude the nested list, so the group title is not the
                // concatenation of all its descendant entries.
                let mut txt = String::new();
                for child in li.children() {
                    match child.value() {
                        Node::Text(x) => txt.push_str(&x.text),
                        Node::Element(e) if e.name() != "ol" => {
                            if let Some(c) = ElementRef::wrap(child) {
                                txt.push_str(&c.text().collect::<String>());
                            }
                        }
                        _ => {}
                    }
                }
                (txt.trim().to_string(), None, None)
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
            source,
            children,
        });
    }
    out
}

/// Branch tree of an EPUB 2 NCX `navMap`. `src` values are relative to the
/// NCX document's directory; sibling order follows `playOrder`.
fn parse_ncx_branches(xml: &str, ncx_path: &Path) -> Vec<TocBranch> {
    if let Ok(parsed) = roxmltree::Document::parse(xml)
        && let Some(map) = parsed.descendants().find(|n| n.has_tag_name("navMap"))
    {
        return collect_xml_ncx(map, ncx_path);
    }
    let frag = Html::parse_fragment(xml);
    let base = ncx_path;
    // The top-level `navPoint` elements live inside `navMap`; start there
    // so `collect_ncx_branches` sees exactly one level per call.
    let parent = frag
        .select(&Selector::parse("navmap").expect("static selector"))
        .next()
        .unwrap_or(frag.root_element());
    collect_ncx_branches(&parent, base)
}

fn collect_xml_ncx(parent: roxmltree::Node<'_, '_>, base: &Path) -> Vec<TocBranch> {
    let mut points: Vec<_> = parent
        .children()
        .filter(|n| n.has_tag_name("navPoint"))
        .map(|point| {
            let order = point
                .attribute("playOrder")
                .and_then(|o| o.parse::<usize>().ok())
                .unwrap_or(usize::MAX);
            let title = point
                .children()
                .find(|n| n.has_tag_name("navLabel"))
                .map(xml_text)
                .unwrap_or_default();
            let href = point
                .children()
                .find(|n| n.has_tag_name("content"))
                .and_then(|n| n.attribute("src"));
            let path = href.and_then(|h| resolve(base, h));
            let source = href.map(|h| (base.to_path_buf(), h.to_owned()));
            (
                order,
                TocBranch {
                    title,
                    path,
                    source,
                    children: collect_xml_ncx(point, base),
                },
            )
        })
        .collect();
    points.sort_by_key(|(order, _)| *order);
    points.into_iter().map(|(_, branch)| branch).collect()
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
        if let Some(anc) = nearest_navpoint_ancestor(np)
            && anc != *parent
        {
            continue;
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
        // The entry's own `<content>` is the first one before any nested
        // `navPoint`: html5ever (HTML mode) hoists nested navPoints into
        // the previous `<content/>`, so a descendant search would make
        // group navPoints steal their first child's target.
        let href = np
            .child_elements()
            .take_while(|e| e.value().name() != "navpoint")
            .find_map(|e| {
                if e.value().name() == "content" {
                    e.value().attr("src").map(|s| s.to_string())
                } else {
                    e.select(&content_sel)
                        .next()
                        .and_then(|c| c.value().attr("src").map(|s| s.to_string()))
                }
            });
        let path = href.as_deref().and_then(|h| resolve(base, h));
        let source = href.map(|h| (base.to_path_buf(), h));
        let children = collect_ncx_branches(&np, base);
        ordered.push((
            order,
            TocBranch {
                title,
                path,
                source,
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
        if let Node::Element(e) = p.value()
            && e.name() == "navpoint"
        {
            return ElementRef::wrap(p);
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
            path: resolve(Path::new(""), &p.content.to_string_lossy()),
            source: Some((PathBuf::new(), p.content.to_string_lossy().into_owned())),
            children: navpoint_branches(&p.children),
        })
        .collect()
}

fn recover_navigation<R: Read + Seek>(
    branches: &mut [TocBranch],
    references: &crate::epub_reference::ReferenceResolver,
    container: &crate::epub_container::Container<R>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for branch in branches {
        if let Some((base, href)) = &branch.source {
            branch.path = references.resolve(container, base, href, diagnostics);
        }
        recover_navigation(&mut branch.children, references, container, diagnostics);
    }
}

/// Flatten a branch tree into `(path, label)` pairs, depth-first,
/// **children last** so document-level title lookups prefer the deepest
/// (most specific) entry for a document.
fn flatten_branches(branches: &[TocBranch], out: &mut Vec<(Reference, String)>) {
    for b in branches {
        if let Some(path) = &b.path {
            out.push((path.clone(), b.title.clone()));
        }
        flatten_branches(&b.children, out);
    }
}

/// Key for a container path already canonicalized by the URL resolver.
fn path_key(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Map the parsed branch tree onto the final chapter list.
///
/// - a branch whose document became a chapter keeps its title and gets the
///   chapter's index;
/// - fragment entries (`file.html#section`) resolve to their containing
///   file's chapter and stay in the tree with the fragment attached, so
///   calibre-style per-section navPoints survive as structure (the reader
///   jumps into the section); exact duplicates of an ancestor entry's
///   target are dropped;
/// - branches without a chapter of their own (no target at all, or their
///   document was not kept — auxiliary `linear="no"` part pages, pages
///   absent from the spine, image-only leaves) stay as pure structure
///   groups (`idx = null`) when their children survive; only empty junk
///   (cover pages, blank sheets, …) disappears entirely.
fn remap_branches(
    branches: &[TocBranch],
    spine_pos: &HashMap<String, usize>,
    kept: &[usize],
    parent_key: Option<&Reference>,
) -> Vec<TocNode> {
    let mut out = Vec::new();
    for b in branches {
        let key = b.path.as_ref();
        let this_idx: Option<u32> = key.and_then(|k| {
            let pos = spine_pos.get(&k.path)?;
            kept.iter().position(|kept| kept == pos).map(|i| i as u32)
        });
        match this_idx {
            None => {
                // The branch has no chapter of its own: keep it as a
                // structure node when it still has children.
                let children = remap_branches(&b.children, spine_pos, kept, parent_key);
                if !children.is_empty() {
                    out.push(TocNode {
                        title: b.title.clone(),
                        idx: None,
                        frag: None,
                        children,
                    });
                }
            }
            Some(i) => {
                // Exact duplicate of an ancestor entry (same target):
                // already reachable, drop it.
                if key == parent_key {
                    out.extend(remap_branches(&b.children, spine_pos, kept, parent_key));
                    continue;
                }
                let frag = key.and_then(|k| k.fragment.clone());
                let children = remap_branches(&b.children, spine_pos, kept, key);
                out.push(TocNode {
                    title: b.title.clone(),
                    idx: Some(i),
                    frag,
                    children,
                });
            }
        }
    }
    out
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
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    /// Replace/add entries in a generated fixture without shipping real books.
    fn patch_epub(bytes: &[u8], replacements: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            if let Some((_, replacement)) = replacements.iter().find(|(p, _)| *p == entry.name()) {
                data = replacement.to_vec();
            }
            writer.start_file(entry.name(), opts).unwrap();
            writer.write_all(&data).unwrap();
        }
        for (path, data) in replacements {
            if archive.by_name(path).is_err() {
                writer.start_file(*path, opts).unwrap();
                writer.write_all(data).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn recovers_package_relative_producer_links_only_after_exact_resolution_fails() {
        for version in ["2.0", "3.0"] {
            let bytes = build_epub(&Fixture {
                version,
                metadata_extra: "",
                spine: vec![
                    ("text/main.xhtml".into(), true),
                    ("text/end.xhtml".into(), true),
                ],
                toc: vec![e("正文", "text/main.xhtml", vec![])],
            });
            let mut doc = EpubDoc::from_reader(std::io::Cursor::new(&bytes)).unwrap();
            let nav_name = if version == "2.0" {
                "toc.ncx"
            } else {
                "nav.xhtml"
            };
            let nav = doc
                .get_resource_str_by_path(format!("OEBPS/{nav_name}"))
                .unwrap();
            let opf = doc
                .get_resource_str_by_path("OEBPS/content.opf")
                .unwrap()
                .replace(
                    &format!("href=\"{nav_name}\""),
                    &format!("href=\"text/{nav_name}\""),
                )
                .replace(
                    "</manifest>",
                    r#"<item id="css" href="style.css" media-type="text/css"/></manifest>"#,
                );
            let main = r#"<html><head><link rel="stylesheet" href="style.css"/></head><body><p>正文<img src="images/pic.png"/><a href="text/end.xhtml#end">下一章</a></p></body></html>"#;
            let bytes = patch_epub(
                &bytes,
                &[
                    ("OEBPS/content.opf", opf.as_bytes()),
                    (&format!("OEBPS/text/{nav_name}"), nav.as_bytes()),
                    ("OEBPS/text/main.xhtml", main.as_bytes()),
                    (
                        "OEBPS/style.css",
                        b"body { margin:7em; background-image:url(images/pic.png) }",
                    ),
                ],
            );
            let (strict, diagnostics) = parse_with_policy(&bytes, ParsePolicy::Strict).unwrap();
            assert!(strict.images.is_empty());
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.reason == "missing-manifest-resource")
            );
            assert!(
                diagnostics
                    .iter()
                    .all(|d| !d.reason.starts_with("compatibility-"))
            );
            let (compatible, diagnostics) = parse_with_diagnostics(&bytes).unwrap();
            assert_eq!(compatible.chapters.len(), 2);
            assert_eq!(compatible.images.len(), 1);
            assert_eq!(compatible.toc[0].idx, Some(0));
            assert_eq!(compatible.toc[0].title, "正文");
            assert!(compatible.chapters[0].content.contains("margin:7em"));
            assert!(
                compatible.chapters[0]
                    .content
                    .contains("epub:chapter/1#end")
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.reason == "compatibility-opf-relative-reference")
            );
            assert!(
                diagnostics
                    .iter()
                    .all(|d| d.reason != "missing-manifest-resource"
                        && d.reason != "missing-navigation-resource")
            );
        }
    }

    #[test]
    fn resolves_foreign_spine_fallback_chains_and_navigation_aliases() {
        let bytes = build_epub(&Fixture {
            version: "3.0",
            metadata_extra: "",
            spine: vec![("foreign.bin".into(), true)],
            toc: vec![e("正文", "foreign.bin", vec![])],
        });
        let mut doc = EpubDoc::from_reader(std::io::Cursor::new(&bytes)).unwrap();
        let opf = doc.get_resource_str_by_path("OEBPS/content.opf").unwrap()
            .replace(r#"href="foreign.bin" media-type="application/xhtml+xml""#, r#"href="foreign.bin" media-type="application/octet-stream" fallback="mid""#)
            .replace("</manifest>", r#"<item id="mid" href="middle.bin" media-type="application/octet-stream" fallback="final"/><item id="final" href="fallback.xhtml" media-type="application/xhtml+xml"/></manifest>"#);
        let bytes = patch_epub(
            &bytes,
            &[
                ("OEBPS/content.opf", opf.as_bytes()),
                ("OEBPS/middle.bin", b"foreign resource"),
                (
                    "OEBPS/fallback.xhtml",
                    "<html><body><p>备用正文</p><a href=\"foreign.bin\">回到正文</a></body></html>"
                        .as_bytes(),
                ),
            ],
        );
        let book = parse(&bytes).unwrap();
        assert_eq!(book.chapters.len(), 1);
        assert!(book.chapters[0].linear);
        assert!(book.chapters[0].content.contains("备用正文"));
        assert!(book.chapters[0].content.contains("epub:chapter/0"));
        assert_eq!(book.toc[0].idx, Some(0));
        let cycle = patch_epub(
            &bytes,
            &[(
                "OEBPS/content.opf",
                opf.replace(r#"fallback="final""#, r#"fallback="c0""#)
                    .as_bytes(),
            )],
        );
        assert!(parse(&cycle).is_err());
    }

    #[test]
    fn preserves_multiple_creators_and_contributor_roles_in_package_order() {
        for version in ["2.0", "3.0"] {
            let extra = if version == "2.0" {
                r#"<dc:creator opf:role="aut">第二作者</dc:creator><dc:creator opf:role="aut">测试作者</dc:creator><dc:contributor opf:role="trl">译者</dc:contributor><dc:contributor opf:role="ill">画师</dc:contributor>"#
            } else {
                r##"<dc:creator id="author2">第二作者</dc:creator><meta refines="#author2" property="role" scheme="marc:relators">aut</meta><dc:contributor id="translator">译者</dc:contributor><meta refines="#translator" property="role" scheme="marc:relators">trl</meta><dc:creator id="illustrator">画师</dc:creator><meta refines="#illustrator" property="role" scheme="marc:relators">ill</meta>"##
            };
            let bytes = build_epub(&Fixture {
                version,
                metadata_extra: extra,
                toc: vec![],
                spine: vec![("ch.xhtml".into(), true)],
            });
            let book = parse(&bytes).unwrap();
            assert_eq!(book.authors, vec!["测试作者", "第二作者"]);
            assert_eq!(book.ext.translators, Some(vec!["译者".into()]));
            assert_eq!(book.ext.illustrators, Some(vec!["画师".into()]));
            let contributors = book.ext.extra["epub_contributors"].as_array().unwrap();
            assert_eq!(contributors[0]["name"], "测试作者");
            assert!(contributors.iter().any(|c| {
                c["roles"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["value"] == "ill")
            }));
        }
    }

    #[test]
    fn links_across_documents_and_auxiliary_footnotes_without_changing_linear_order() {
        for version in ["2.0", "3.0"] {
            let bytes = build_epub(&Fixture {
                version,
                metadata_extra: "",
                spine: vec![
                    ("Notes/foot.xhtml".into(), false),
                    ("Text/main.xhtml".into(), true),
                    ("Text/end.xhtml".into(), true),
                ],
                toc: vec![e(
                    "正文",
                    "Text/main.xhtml",
                    vec![e("注释", "Notes/foot.xhtml#note%20one", vec![])],
                )],
            });
            let mut doc = EpubDoc::from_reader(std::io::Cursor::new(&bytes)).unwrap();
            let opf = doc.get_resource_str_by_path("OEBPS/content.opf").unwrap()
                .replace("</manifest>", r#"<item id="extra" href="Notes/extra.xhtml" media-type="application/xhtml+xml"/></manifest>"#);
            let main = r##"<html><body><p id="back">正文<a href="../Notes/foot.xhtml?reader=1#note%20one">注释</a><a href="../Notes/extra.xhtml">附录</a><a href="#back">本页</a><a href="missing.xhtml">不存在</a></p></body></html>"##;
            let foot = r#"<html><body><p id="note one">注释<a href="../Text/main.xhtml#back">返回</a></p></body></html>"#;
            let bytes = patch_epub(
                &bytes,
                &[
                    ("OEBPS/content.opf", opf.as_bytes()),
                    ("OEBPS/Text/main.xhtml", main.as_bytes()),
                    ("OEBPS/Notes/foot.xhtml", foot.as_bytes()),
                    (
                        "OEBPS/Notes/extra.xhtml",
                        "<html><body>附录</body></html>".as_bytes(),
                    ),
                ],
            );
            let (book, diagnostics) = parse_with_diagnostics(&bytes).unwrap();
            assert_eq!(
                book.chapters.iter().map(|c| c.linear).collect::<Vec<_>>(),
                vec![true, true, false, false]
            );
            assert!(
                book.chapters[0]
                    .content
                    .contains("epub:chapter/2#note%20one")
            );
            assert!(book.chapters[0].content.contains("epub:chapter/3"));
            assert!(book.chapters[0].content.contains("href=\"#back\""));
            assert!(book.chapters[2].content.contains("epub:chapter/0#back"));
            assert_eq!(book.toc[0].children[0].idx, Some(2));
            assert_eq!(book.toc[0].children[0].frag.as_deref(), Some("note one"));
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.reason == "unreachable-internal-link")
            );
        }
    }

    #[test]
    fn preserves_css_semantics_svg_math_and_blocks_active_resources() {
        let bytes = build_epub(&Fixture {
            version: "3.0",
            metadata_extra: "",
            toc: vec![],
            spine: vec![("Text/ch.xhtml".into(), true), ("art.svg".into(), true)],
        });
        let mut doc = EpubDoc::from_reader(std::io::Cursor::new(&bytes)).unwrap();
        let opf = doc.get_resource_str_by_path("OEBPS/content.opf").unwrap()
            .replace(r#"href="art.svg" media-type="application/xhtml+xml""#, r#"href="art.svg" media-type="image/svg+xml""#)
            .replace("</manifest>", r#"<item id="style" href="Styles/main.css" media-type="text/css"/><item id="import" href="Styles/nested.css" media-type="text/css"/><item id="font" href="Fonts/test.woff2" media-type="font/woff2"/><item id="art" href="image.svg" media-type="image/svg+xml"/></manifest>"#);
        let xhtml = r##"<html lang="zh-CN" dir="rtl" class="book"><head><link rel="stylesheet" href="../Styles/main.css"/><style>.inline { letter-spacing:0.2em }</style><meta http-equiv="refresh" content="0;url=https://tracker.test"/></head><body><section class="inline" aria-label="正文" style="text-indent:2em"><p>正文<ruby>字<rt>zì</rt></ruby></p><img src="../image.svg"/><svg viewBox="0 0 10 10"><defs><path id="shape" d="M0,0L10,10"/></defs><use href="#shape"/><image href="../images/pic.png"/></svg><math xmlns="http://www.w3.org/1998/Math/MathML"><mfrac><mi>a</mi><mi>b</mi></mfrac></math><script>evil()</script><iframe src="https://tracker.test"/><form><input/></form><img onerror="evil()" src="https://tracker.test/a.png"/></section></body></html>"##;
        let css = r#"@import "nested.css"; @import url(https://tracker.test/t.css); @font-face{font-family:book;src:url('../Fonts/test.woff2')} body{writing-mode:vertical-rl} .book p{text-indent:2em;background-image:url('../images/pic.png?size=1')} .remote{background:url(https://tracker.test/p.png)}"#;
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/><script>evil()</script><image href="image.svg"/><foreignObject><p>unsafe</p></foreignObject></svg>"##;
        let bytes = patch_epub(
            &bytes,
            &[
                ("OEBPS/content.opf", opf.as_bytes()),
                ("OEBPS/Text/ch.xhtml", xhtml.as_bytes()),
                ("OEBPS/Styles/main.css", css.as_bytes()),
                (
                    "OEBPS/Styles/nested.css",
                    b"p{margin-bottom:2em} @import 'main.css';",
                ),
                ("OEBPS/Fonts/test.woff2", b"test-font"),
                ("OEBPS/image.svg", svg.as_bytes()),
                ("OEBPS/art.svg", svg.as_bytes()),
            ],
        );
        let (book, diagnostics) = parse_with_diagnostics(&bytes).unwrap();
        assert_eq!(book.chapters.len(), 2);
        let content = &book.chapters[0].content;
        assert!(content.starts_with("<!doctype html>"));
        assert!(content.contains("writing-mode:vertical-rl"));
        assert!(content.contains("margin-bottom:2em"));
        assert!(content.contains("data:font/woff2;base64,"));
        assert!(content.contains("data:image/png;base64,"));
        assert!(content.contains("aria-label=\"正文\""));
        assert!(content.contains("style=\"text-indent:2em\""));
        assert!(content.contains("viewBox=\"0 0 10 10\""));
        assert!(content.contains("<mfrac>"));
        assert!(content.contains("image:0"));
        assert!(!content.contains("tracker.test"));
        assert!(!content.contains("evil()"));
        assert!(!content.contains("http-equiv"));
        assert!(book.chapters[1].content.contains("<rect"));
        assert!(book.images.iter().any(|i| i.mime == "image/svg+xml"));
        let svg = std::str::from_utf8(&book.images[0].bytes).unwrap();
        roxmltree::Document::parse(svg).unwrap();
        assert!(!svg.contains("evil"));
        assert!(
            diagnostics
                .iter()
                .any(|d| d.reason == "blocked-active-content")
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.reason == "cyclic-stylesheet-import")
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.reason == "cyclic-resource-reference")
        );
    }

    #[test]
    fn selects_toc_tokens_and_namespace_not_first_landmarks_nav() {
        for tokens in ["toc", "toc landmarks", "landmarks toc"] {
            let xml = format!(
                r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:e="http://www.idpf.org/2007/ops"><body><nav e:type="landmarks"><ol><li><a href="cover.xhtml">封面</a></li></ol></nav><nav e:type="{tokens}"><ol><li><a href="../Text/ch.xhtml#section">章节</a></li></ol></nav></body></html>"#
            );
            let branches = parse_nav_branches(&xml, Path::new("OPS/Nav/nav.xhtml"));
            assert_eq!(branches.len(), 1);
            assert_eq!(branches[0].title, "章节");
            assert_eq!(branches[0].path.as_ref().unwrap().path, "OPS/Text/ch.xhtml");
        }
        let xml = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:e="http://www.idpf.org/2007/ops"><body><nav e:type="page-list"><ol><li><a href="a.xhtml">页码</a></li></ol></nav></body></html>"#;
        assert!(parse_nav_branches(xml, Path::new("OPS/nav.xhtml")).is_empty());
    }

    #[test]
    fn selects_spine_ncx_and_deterministic_malformed_package_fallback() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            metadata_extra: "",
            spine: vec![("ch.xhtml".into(), true)],
            toc: vec![e("指定目录", "ch.xhtml", vec![])],
        });
        let mut doc = EpubDoc::from_reader(std::io::Cursor::new(&bytes)).unwrap();
        let opf = doc.get_resource_str_by_path("OEBPS/content.opf").unwrap()
            .replace("</manifest>", r#"<item id="other" href="aaa.ncx" media-type="application/x-dtbncx+xml"/></manifest>"#);
        let other = r#"<ncx><navMap><navPoint><navLabel><text>备用目录</text></navLabel><content src="ch.xhtml"/></navPoint></navMap></ncx>"#;
        let selected = patch_epub(
            &bytes,
            &[
                ("OEBPS/content.opf", opf.as_bytes()),
                ("OEBPS/aaa.ncx", other.as_bytes()),
            ],
        );
        assert_eq!(parse(&selected).unwrap().chapters[0].title, "指定目录");
        let fallback = patch_epub(
            &selected,
            &[(
                "OEBPS/content.opf",
                opf.replace(r#"toc="ncx""#, r#"toc="missing""#).as_bytes(),
            )],
        );
        for _ in 0..8 {
            assert_eq!(parse(&fallback).unwrap().chapters[0].title, "备用目录");
        }
    }

    #[test]
    fn local_navigation_fragments_resolve_against_the_navigation_document() {
        let xml = r##"<html><body><nav epub:type="toc"><ol><li><a href="#section">小节</a></li></ol></nav></body></html>"##;
        let branches = parse_nav_branches(xml, Path::new("OPS/nav.xhtml"));
        assert_eq!(branches[0].path.as_ref().unwrap().path, "OPS/nav.xhtml");
        assert_eq!(
            branches[0].path.as_ref().unwrap().fragment.as_deref(),
            Some("section")
        );
        let ncx = r##"<ncx><navMap><navPoint><navLabel><text>小节</text></navLabel><content src="#section"/></navPoint></navMap></ncx>"##;
        assert_eq!(
            parse_ncx_branches(ncx, Path::new("OPS/toc.ncx"))[0]
                .path
                .as_ref()
                .unwrap()
                .path,
            "OPS/toc.ncx"
        );
    }

    #[test]
    fn preserves_headingless_short_blank_and_image_only_linear_documents() {
        for version in ["2.0", "3.0"] {
            let bytes = build_epub(&Fixture {
                version,
                metadata_extra: "",
                toc: vec![],
                spine: vec![
                    ("title.xhtml".into(), true),
                    ("short.xhtml".into(), true),
                    ("blank.xhtml".into(), true),
                    ("pic.xhtml".into(), true),
                    ("cover.xhtml".into(), false),
                ],
            });
            let bytes = patch_epub(
                &bytes,
                &[
                    (
                        "OEBPS/short.xhtml",
                        "<html><body><p>短篇。</p></body></html>".as_bytes(),
                    ),
                    ("OEBPS/blank.xhtml", b"<html><body></body></html>"),
                ],
            );
            let book = parse(&bytes).unwrap();
            assert_eq!(book.chapters.len(), 5);
            assert!(book.chapters[0].content.contains("测试作者"));
            assert!(book.chapters[1].content.contains("短篇。"));
            assert!(book.chapters[2].content.contains("<body></body>"));
            assert!(book.chapters[3].content.contains("image:0"));
            assert_eq!(book.toc.len(), 4);
        }
    }

    #[test]
    fn resolves_nested_nav_images_queries_and_encoded_names() {
        let bytes = build_epub(&Fixture {
            version: "3.0",
            spine: vec![("Text/chapter one.xhtml".into(), true)],
            toc: vec![],
            metadata_extra: "",
        });
        let mut doc = EpubDoc::from_reader(std::io::Cursor::new(&bytes)).unwrap();
        let opf = doc
            .get_resource_str_by_path("OEBPS/content.opf")
            .unwrap()
            .replace(
                "Text/chapter one.xhtml",
                "Text/./chapter%20one.xhtml?edition=1",
            )
            .replace("nav.xhtml", "Navigation/nav.xhtml");
        let bytes = patch_epub(&bytes, &[
            ("OEBPS/content.opf", opf.as_bytes()),
            ("OEBPS/Navigation/nav.xhtml", r#"<html><body><nav epub:type="toc"><ol><li><a href="../Text/chapter%20one.xhtml?reader=1#part%20two">测试章节</a></li></ol></nav></body></html>"#.as_bytes()),
            ("OEBPS/Text/chapter one.xhtml", r#"<html><body><h1>测试章节</h1><p id="part two">正文</p><img src="../images/./pic.png?size=1#xywh=0,0,1,1"/></body></html>"#.as_bytes()),
        ]);
        let book = parse(&bytes).unwrap();
        assert_eq!(book.chapters[0].title, "测试章节");
        assert_eq!(book.toc[0].idx, Some(0));
        assert_eq!(book.toc[0].frag.as_deref(), Some("part two"));
        assert_eq!(book.images.len(), 1);
        assert!(book.chapters[0].content.contains("image:0"));
    }

    const PNG_1PX: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// A TOC entry fixture: `label`, the spine file it links to
    /// ("" = pure group without a link), and its nested entries
    /// (arbitrary depth).
    struct TocFixture {
        label: String,
        file: String,
        children: Vec<TocFixture>,
    }

    /// Shorthand for building [`TocFixture`] trees.
    fn e(label: &str, file: &str, children: Vec<TocFixture>) -> TocFixture {
        TocFixture {
            label: label.into(),
            file: file.into(),
            children,
        }
    }

    struct Fixture {
        version: &'static str,
        spine: Vec<(String, bool)>, // (file name, linear)
        toc: Vec<TocFixture>,
        /// Raw XML injected into `<metadata>` (extra dc elements for the
        /// extended-metadata tests).
        metadata_extra: &'static str,
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
                // EPUB 3 cover: exactly one manifest item carries the
                // `cover-image` property.
                manifest.push_str(
                    r#"<item id="cover-image" href="images/pic.png" media-type="image/png" properties="cover-image"/>"#,
                );
            } else {
                manifest.push_str(&format!(
                    r#"<item id="ncx" href="{toc_file}" media-type="application/x-dtbncx+xml"/><item id="pic" href="images/pic.png" media-type="image/png"/>"#
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
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:identifier id="uid">test-{v}</dc:identifier>
    <dc:title>测试书</dc:title>
    <dc:creator>测试作者</dc:creator>
    {extra}
  </metadata>
  <manifest>{manifest}</manifest>
  {spine_xml}
</package>"#,
                    v = fx.version,
                    extra = fx.metadata_extra,
                ),
            );

            if fx.version == "3.0" {
                fn nav_li(entry: &TocFixture) -> String {
                    let head = if entry.file.is_empty() {
                        format!("<span>{}</span>", entry.label)
                    } else {
                        format!(r#"<a href="{}">{}</a>"#, entry.file, entry.label)
                    };
                    let inner = if entry.children.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "<ol>{}</ol>",
                            entry.children.iter().map(nav_li).collect::<String>()
                        )
                    };
                    format!("<li>{head}{inner}</li>")
                }
                let nav_items = fx.toc.iter().map(nav_li).collect::<String>();
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
                fn ncx_points(entries: &[TocFixture], order: &mut usize) -> String {
                    let mut out = String::new();
                    for entry in entries {
                        let id = *order;
                        *order += 1;
                        let content = if entry.file.is_empty() {
                            String::new()
                        } else {
                            format!(r#"<content src="{}"/>"#, entry.file)
                        };
                        out.push_str(&format!(
                            r#"<navPoint id="np{id}" playOrder="{id}"><navLabel><text>{}</text></navLabel>{content}{}</navPoint>"#,
                            entry.label,
                            ncx_points(&entry.children, order)
                        ));
                    }
                    out
                }
                let mut order = 0usize;
                let nav_points = ncx_points(&fx.toc, &mut order);
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
                } else if file.starts_with("main") {
                    // Main-content page without a heading, as produced by the
                    // affected EPUB.
                    r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>未知</title></head><body><p>正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容正文内容</p></body></html>"#
                } else if file.starts_with("note") {
                    // Auxiliary footnote page incorrectly listed in the NCX.
                    r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>未知</title></head><body><dl><dt>[<a>←</a>]</dt><dd><p>注</p></dd></dl></body></html>"#
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
            metadata_extra: "",
            toc: vec![
                e("第一章 起点", "ch1.xhtml", vec![]),
                e(
                    "第二章 风起",
                    "ch2.xhtml",
                    vec![e("第二章之二", "ch2b.xhtml", vec![])],
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

        // Only the non-linear cover is excluded; front matter remains in order.
        assert_eq!(book.chapters.len(), 5);
        assert_eq!(book.chapters[0].title, "第 1 章");
        assert_eq!(book.chapters[1].title, "第一章 起点");
        assert_eq!(book.chapters[2].title, "第二章 风起");

        // TOC stays a tree: 第二章 风起 groups its 第二章之二 child; the
        // same-document fragment entry is deduped against its parent.
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].title, "第一章 起点");
        assert_eq!(book.toc[0].idx, Some(1));
        assert!(book.toc[0].children.is_empty());
        assert_eq!(book.toc[1].title, "第二章 风起");
        assert_eq!(book.toc[1].idx, Some(2));
        assert_eq!(book.toc[1].children.len(), 1);
        assert_eq!(book.toc[1].children[0].title, "第二章之二");
        assert_eq!(book.toc[1].children[0].idx, Some(3));

        // HTML structure preserved, junk stripped.
        let c0 = &book.chapters[1];
        assert!(c0.content.contains("<p>第一段"));
        assert!(c0.content.contains("<ruby>注音<rt>zhùyīn</rt></ruby>"));
        assert!(c0.content.contains("<strong>强调</strong>"));
        assert!(!c0.content.contains("<script"));
        assert!(!c0.content.contains("onclick"));
        assert!(c0.content.contains("style=\"color:red\""));
        assert!(c0.content.contains("<svg>"));
        assert!(c0.content.contains("<table>"));

        // Embedded image becomes an ingest placeholder; the bytes land in
        // `book.images` for the server's image store.
        let placeholder = "image:0";
        assert!(c0.content.contains(&format!(r#"src="{placeholder}""#)));
        assert_eq!(book.images.len(), 1);
        assert!(book.images[0].mime.starts_with("image/"));
        assert!(!book.images[0].bytes.is_empty());
        // Links: in-page anchor and http survive; cross-chapter href is
        // dropped (the reader renders one chapter at a time).
        assert!(c0.content.contains(r##"<a href="#sec1">"##));
        assert!(
            c0.content
                .contains(r#"<a href="https://example.com">外部</a>"#)
        );
        assert!(
            c0.content
                .contains("<a href=\"epub:chapter/2\">跨章链接</a>")
        );
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
            metadata_extra: "",
            toc: vec![
                e("插图", "pic.xhtml", vec![]),
                e("序章", "ch1.xhtml", vec![]),
                e("第二卷", "ch2.xhtml", vec![e("终章", "ch2b.xhtml", vec![])]),
            ],
        });
        let book = parse(&bytes).unwrap();
        // Image-only linear pages are content, not auxiliary material.
        assert_eq!(book.chapters.len(), 4);
        assert_eq!(book.chapters[0].title, "插图");
        assert_eq!(book.chapters[1].title, "序章");
        assert_eq!(book.chapters[2].title, "第二卷");
        // Nested nav items keep their hierarchy.
        assert_eq!(book.toc.len(), 3);
        assert_eq!(book.toc[0].title, "插图");
        assert_eq!(book.toc[0].idx, Some(0));
        assert_eq!(book.toc[2].title, "第二卷");
        assert_eq!(book.toc[2].idx, Some(2));
        assert_eq!(book.toc[2].children.len(), 1);
        assert_eq!(book.toc[2].children[0].title, "终章");
        assert_eq!(book.toc[2].children[0].idx, Some(3));
        assert!(book.chapters[1].content.contains("<h1>正文标题 1</h1>"));
        assert!(book.chapters[1].content.contains("src=\"image:0\""));
        assert!(!book.images.is_empty());
    }

    /// Deep fixture: 卷 spans (href-less groups) at every level above the
    /// chapters — the typical Chinese light-novel TOC shape.
    fn deep_fixture(version: &'static str) -> Fixture {
        Fixture {
            version,
            spine: vec![
                ("h1.xhtml".into(), true),
                ("c1.xhtml".into(), true),
                ("c2.xhtml".into(), true),
                ("h2.xhtml".into(), true),
                ("c3.xhtml".into(), true),
                ("h3.xhtml".into(), true),
                ("c4.xhtml".into(), true),
            ],
            metadata_extra: "",
            toc: vec![
                e(
                    "第一卷",
                    "",
                    vec![
                        e(
                            "第一话",
                            "h1.xhtml",
                            vec![
                                e("第一章", "c1.xhtml", vec![]),
                                e("第二章", "c2.xhtml", vec![]),
                            ],
                        ),
                        e("第二话", "h2.xhtml", vec![e("第三章", "c3.xhtml", vec![])]),
                    ],
                ),
                e(
                    "第二卷",
                    "",
                    vec![e(
                        "第一话",
                        "h3.xhtml",
                        vec![e("第四章", "c4.xhtml", vec![])],
                    )],
                ),
            ],
        }
    }

    #[test]
    fn recovers_spine_when_ncx_only_lists_footnotes() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            spine: vec![
                ("title.xhtml".into(), true),
                ("main-1.xhtml".into(), true),
                ("main-2.xhtml".into(), true),
                ("note-1.xhtml".into(), true),
                ("note-2.xhtml".into(), true),
            ],
            metadata_extra: "",
            toc: vec![
                e("1", "note-1.xhtml", vec![]),
                e("2", "note-2.xhtml", vec![]),
            ],
        });
        let book = parse(&bytes).unwrap();

        assert_eq!(book.chapters.len(), 5);
        assert!(book.chapters[0].content.contains("测试作者"));
        assert!(book.chapters[1].content.contains("正文内容"));
        assert!(book.chapters[2].content.contains("正文内容"));
        assert!(book.chapters[3].content.contains("注"));
        assert_eq!(book.toc[0].idx, Some(3));
        assert_eq!(book.toc[1].idx, Some(4));
    }

    #[test]
    fn parses_three_level_nav_toc_epub3() {
        let book = parse(&build_epub(&deep_fixture("3.0"))).unwrap();
        assert_eq!(book.chapters.len(), 7);
        // 卷 → 话 → 章: every level survives, href-less groups included.
        assert_eq!(book.toc.len(), 2);
        let vol1 = &book.toc[0];
        assert_eq!(vol1.title, "第一卷");
        assert_eq!(vol1.idx, None);
        assert_eq!(vol1.children.len(), 2);
        let hua1 = &vol1.children[0];
        assert_eq!(hua1.title, "第一话");
        assert_eq!(hua1.idx, Some(0));
        assert_eq!(
            hua1.children
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>(),
            vec!["第一章", "第二章"]
        );
        assert_eq!(hua1.children[0].idx, Some(1));
        assert_eq!(hua1.children[1].idx, Some(2));
        let hua2 = &vol1.children[1];
        assert_eq!(hua2.title, "第二话");
        assert_eq!(hua2.children[0].idx, Some(4)); // 第三章
        let vol2 = &book.toc[1];
        assert_eq!(vol2.title, "第二卷");
        assert_eq!(vol2.idx, None);
        assert_eq!(vol2.children[0].title, "第一话");
        assert_eq!(vol2.children[0].children[0].idx, Some(6)); // 第四章
        // Group titles must not concatenate their children's titles.
        assert_eq!(vol1.title, "第一卷");
        assert_eq!(vol1.children[1].title, "第二话");
    }

    #[test]
    fn parses_three_level_ncx_toc_epub2() {
        let book = parse(&build_epub(&deep_fixture("2.0"))).unwrap();
        assert_eq!(book.chapters.len(), 7);
        assert_eq!(book.toc.len(), 2);
        let vol1 = &book.toc[0];
        assert_eq!(vol1.title, "第一卷");
        assert_eq!(vol1.idx, None);
        assert_eq!(vol1.children.len(), 2);
        assert_eq!(vol1.children[0].title, "第一话");
        assert_eq!(vol1.children[0].idx, Some(0));
        assert_eq!(
            vol1.children[0]
                .children
                .iter()
                .map(|c| (c.title.as_str(), c.idx))
                .collect::<Vec<_>>(),
            vec![("第一章", Some(1)), ("第二章", Some(2))]
        );
        assert_eq!(vol1.children[1].children[0].idx, Some(4));
        let vol2 = &book.toc[1];
        assert_eq!(vol2.title, "第二卷");
        assert_eq!(vol2.children[0].children[0].idx, Some(6));
        assert_eq!(vol1.title, "第一卷");
        assert_eq!(vol1.children[1].title, "第二话");
    }

    #[test]
    fn part_pages_become_group_nodes() {
        // Part divider pages are auxiliary (linear="no") or entirely
        // absent from the spine; the nav entries that point at them must
        // survive as structure groups (this is the shape of books whose
        // parts have no own chapter, e.g. the 汪晖 epub that triggered
        // the fix). Linear image-only leaves remain navigable.
        let bytes = build_epub(&Fixture {
            version: "3.0",
            spine: vec![
                ("part1.xhtml".into(), false), // 第一编 divider, auxiliary
                ("e1.xhtml".into(), true),
                ("e2.xhtml".into(), true),
                ("part2.xhtml".into(), false), // 第二编 divider, auxiliary
                ("e3.xhtml".into(), true),
                ("pic.xhtml".into(), true), // linear illustration plate
            ],
            metadata_extra: "",
            toc: vec![
                e("序言", "e1.xhtml", vec![]),
                e(
                    "第一编 去政治化的政治",
                    "part1.xhtml",
                    vec![
                        e("去政治化的政治", "e1.xhtml", vec![]),
                        e("当代中国的思想状况", "e2.xhtml", vec![]),
                    ],
                ),
                e(
                    "第二编",
                    "part2.xhtml",
                    vec![e("韦伯与中国的现代性", "e3.xhtml", vec![])],
                ),
                e("插图", "pic.xhtml", vec![]),
            ],
        });
        let book = parse(&bytes).unwrap();
        // Only auxiliary divider pages are excluded.
        assert_eq!(book.chapters.len(), 6);
        // Parts remain groups, and the illustration retains its target.
        assert_eq!(book.toc.len(), 4);
        assert_eq!(book.toc[0].title, "序言");
        assert_eq!(book.toc[0].idx, Some(0));
        let part1 = &book.toc[1];
        assert_eq!(part1.title, "第一编 去政治化的政治");
        assert_eq!(part1.idx, Some(4));
        assert_eq!(
            part1
                .children
                .iter()
                .map(|c| (c.title.as_str(), c.idx))
                .collect::<Vec<_>>(),
            vec![("去政治化的政治", Some(0)), ("当代中国的思想状况", Some(1))]
        );
        let part2 = &book.toc[2];
        assert_eq!(part2.title, "第二编");
        assert_eq!(part2.idx, Some(5));
        assert_eq!(part2.children[0].title, "韦伯与中国的现代性");
        assert_eq!(part2.children[0].idx, Some(2));
    }

    #[test]
    fn keeps_fragment_sections() {
        // Calibre-style epub: one real file per chapter plus fragment
        // navPoints for the sections inside it (汪晖 epub: 61 of its 84
        // navPoints are `file.html#section` entries, up to 4 levels deep).
        for version in ["2.0", "3.0"] {
            let bytes = build_epub(&Fixture {
                version,
                spine: vec![
                    ("e1.xhtml".into(), true),
                    ("e2.xhtml".into(), true),
                    ("e3.xhtml".into(), true),
                ],
                metadata_extra: "",
                toc: vec![
                    e("序章", "e1.xhtml", vec![]),
                    e(
                        "去政治化的政治",
                        "e2.xhtml",
                        vec![
                            e("一、中国与60年代的终结", "e2.xhtml#sec1", vec![]),
                            e(
                                "二、去政治化的政治",
                                "e2.xhtml#sec2",
                                vec![
                                    e("去政治化与政党政治的转变", "e2.xhtml#sec2a", vec![]),
                                    e("去政治化与理论辩论的终结", "e2.xhtml#sec2b", vec![]),
                                ],
                            ),
                            e("三、去政治化的政治与现代社会", "e2.xhtml#sec3", vec![]),
                        ],
                    ),
                    e("韦伯", "e3.xhtml", vec![]),
                ],
            });
            let book = parse(&bytes).unwrap();
            // Real files stay chapters; the sections resolve into them.
            assert_eq!(book.chapters.len(), 3);
            assert_eq!(book.toc.len(), 3);
            let essay = &book.toc[1];
            assert_eq!(essay.title, "去政治化的政治");
            assert_eq!(essay.idx, Some(1));
            assert_eq!(essay.frag, None);
            assert_eq!(essay.children.len(), 3);
            let s1 = &essay.children[0];
            assert_eq!(s1.title, "一、中国与60年代的终结");
            assert_eq!(s1.idx, Some(1));
            assert_eq!(s1.frag.as_deref(), Some("sec1"));
            let s2 = &essay.children[1];
            assert_eq!(s2.frag.as_deref(), Some("sec2"));
            assert_eq!(s2.children.len(), 2);
            assert_eq!(s2.children[0].title, "去政治化与政党政治的转变");
            assert_eq!(s2.children[0].idx, Some(1));
            assert_eq!(s2.children[0].frag.as_deref(), Some("sec2a"));
            assert_eq!(s2.children[1].frag.as_deref(), Some("sec2b"));
            assert_eq!(essay.children[2].frag.as_deref(), Some("sec3"));
            // Chapter title still comes from the file-level entry.
            assert_eq!(book.chapters[1].title, "去政治化的政治");
        }
    }

    #[test]
    fn falls_back_to_first_heading_without_toc() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            spine: vec![("ch1.xhtml".into(), true)],
            metadata_extra: "",
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

    #[test]
    fn extracts_epub3_cover_image() {
        let bytes = build_epub(&Fixture {
            version: "3.0",
            spine: vec![("ch1.xhtml".into(), true)],
            metadata_extra: "",
            toc: vec![e("第一章", "ch1.xhtml", vec![])],
        });
        let book = parse(&bytes).unwrap();
        let cover = book.cover.expect("epub3 cover-image property");
        assert_eq!(cover.mime, "image/png");
        assert_eq!(cover.bytes, PNG_1PX);
    }

    #[test]
    fn extracts_extended_metadata_epub2() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            spine: vec![("ch1.xhtml".into(), true)],
            metadata_extra: r#"
                <dc:identifier opf:scheme="ISBN">1-55860-832-X</dc:identifier>
                <dc:publisher>新潮社</dc:publisher>
                <dc:date>2012-5</dc:date>
                <dc:language>ja</dc:language>
                <dc:creator opf:role="trl">李译者</dc:creator>"#,
            toc: vec![e("第一章", "ch1.xhtml", vec![])],
        });
        let book = parse(&bytes).unwrap();
        assert_eq!(book.ext.isbn.as_deref(), Some("9781558608320"));
        assert_eq!(book.ext.publisher.as_deref(), Some("新潮社"));
        assert_eq!(book.ext.pub_date.as_deref(), Some("2012-5"));
        assert_eq!(book.ext.language.as_deref(), Some("ja"));
        assert_eq!(book.ext.translators, Some(vec!["李译者".to_string()]));
    }

    #[test]
    fn extracts_extended_metadata_epub3() {
        let bytes = build_epub(&Fixture {
            version: "3.0",
            spine: vec![("ch1.xhtml".into(), true)],
            metadata_extra: r##"
                <dc:identifier>urn:isbn:9787536692930</dc:identifier>
                <dc:publisher>重庆出版社</dc:publisher>
                <dc:language>zh-CN</dc:language>
                <meta property="role" refines="#creator" scheme="marc:relators">trl</meta>"##,
            toc: vec![e("第一章", "ch1.xhtml", vec![])],
        });
        let book = parse(&bytes).unwrap();
        // urn:isbn prefix stripped, checksum verified, stored canonically
        assert_eq!(book.ext.isbn.as_deref(), Some("9787536692930"));
        assert_eq!(book.ext.publisher.as_deref(), Some("重庆出版社"));
        assert_eq!(book.ext.language.as_deref(), Some("zh-CN"));
    }

    #[test]
    fn invalid_isbn_in_opf_is_skipped() {
        let bytes = build_epub(&Fixture {
            version: "2.0",
            spine: vec![("ch1.xhtml".into(), true)],
            metadata_extra: r#"
                <dc:identifier opf:scheme="ISBN">1234567890123</dc:identifier>
                <dc:publisher>出版社</dc:publisher>"#,
            toc: vec![e("第一章", "ch1.xhtml", vec![])],
        });
        let book = parse(&bytes).unwrap();
        assert_eq!(book.ext.isbn, None, "invalid ISBN never lands");
        assert_eq!(book.ext.publisher.as_deref(), Some("出版社"));
    }
}
