//! EPUB URL resolution. ZIP paths, queries and decoded fragments stay separate.
use percent_encoding::percent_decode_str;
use std::path::Path;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reference {
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

/// Resolve against a document, never a directory. Only container-local URLs
/// are accepted; network and active schemes must not become ZIP lookups.
pub(crate) fn resolve(document: &Path, href: &str) -> Option<Reference> {
    // The synthetic origin is an implementation detail, never a publication
    // origin. Reject every absolute URL, including one matching that host.
    if Url::parse(href.trim()).is_ok() {
        return None;
    }
    let mut base = Url::parse("https://epub.invalid/").ok()?;
    {
        let mut segments = base.path_segments_mut().ok()?;
        segments.clear();
        for segment in document.to_str()?.split('/') {
            segments.push(segment);
        }
    }
    let reference = base.join(href.trim()).ok()?;
    if reference.origin() != base.origin() || href.trim().starts_with("//") {
        return None;
    }
    let path = percent_decode_str(reference.path().strip_prefix('/')?)
        .decode_utf8()
        .ok()?
        .into_owned();
    if path.contains(['\0', '\\']) {
        return None;
    }
    Some(Reference {
        path,
        query: reference.query().map(str::to_owned),
        fragment: reference
            .fragment()
            .filter(|f| !f.is_empty())
            .and_then(|f| {
                percent_decode_str(f)
                    .decode_utf8()
                    .ok()
                    .map(|s| s.into_owned())
            }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_url_components_without_mixing_zip_paths_and_fragments() {
        let r = resolve(
            Path::new("OPS/nav/toc.xhtml"),
            "../Text/chapter%20one.xhtml?reader=1#节%20二",
        )
        .unwrap();
        assert_eq!(r.path, "OPS/Text/chapter one.xhtml");
        assert_eq!(r.query.as_deref(), Some("reader=1"));
        assert_eq!(r.fragment.as_deref(), Some("节 二"));
        assert_eq!(
            resolve(Path::new("OPS/ch.xhtml"), "#local").unwrap().path,
            "OPS/ch.xhtml"
        );
        assert_eq!(
            resolve(Path::new("OPS/ch.xhtml"), "./../Images/p.png#xywh=1")
                .unwrap()
                .path,
            "Images/p.png"
        );
        assert_eq!(
            resolve(Path::new("OPS/100%.xhtml"), "#a%23b")
                .unwrap()
                .fragment
                .as_deref(),
            Some("a#b")
        );
    }
    #[test]
    fn rejects_non_container_references() {
        for href in [
            "https://tracker.test/p.png",
            "https://epub.invalid/OPS/p.png",
            "https://user:password@epub.invalid/OPS/p.png",
            "//tracker.test/p.png",
            "javascript:alert(1)",
            "data:image/png;base64,x",
            "file:///etc/passwd",
            "a%00.png",
            "a%5cb.png",
        ] {
            assert!(resolve(Path::new("OPS/ch.xhtml"), href).is_none(), "{href}");
        }
    }
}
