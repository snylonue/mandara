//! Explicit, bounded recovery for producer-relative references after standard resolution.
use crate::{
    epub_container::Container,
    epub_render::Diagnostic,
    epub_uri::{Reference, resolve},
};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek},
    path::{Path, PathBuf},
};

pub(crate) struct ReferenceResolver {
    package: PathBuf,
    declared: HashSet<String>,
    conflicts: HashSet<String>,
    compatible: bool,
}
impl ReferenceResolver {
    pub fn new(
        package: PathBuf,
        resources: impl Iterator<Item = (String, String)>,
        compatible: bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Self {
        let mut media = HashMap::new();
        let mut conflicts = HashSet::new();
        let mut duplicates = HashSet::new();
        for (path, mime) in resources {
            if let Some(previous) = media.insert(path.clone(), mime.clone()) {
                if previous != mime {
                    conflicts.insert(path.clone());
                }
                duplicates.insert(path);
            }
        }
        let mut duplicates: Vec<_> = duplicates.into_iter().collect();
        duplicates.sort();
        for path in duplicates {
            note(diagnostics, &package, &path, "duplicate-manifest-resource");
        }
        Self {
            package,
            declared: media.into_keys().collect(),
            conflicts,
            compatible,
        }
    }
    pub fn resolve<R: Read + Seek>(
        &self,
        container: &Container<R>,
        base: &Path,
        href: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Reference> {
        let standard = resolve(base, href)?;
        if self.conflicts.contains(&standard.path) {
            note(diagnostics, base, href, "conflicting-manifest-media-types");
            return None;
        }
        let exists = |r: &Reference| {
            self.declared.contains(&r.path)
                && !self.conflicts.contains(&r.path)
                && container.contains(Path::new(&r.path))
        };
        if exists(&standard) || !self.compatible {
            return Some(standard);
        }
        // Do not reinterpret absolute URLs, fragment-only references or active
        // schemes. There is no basename search, remote fetch or ZIP-wide guess.
        let href = href.trim();
        if href.starts_with(['/', '#', '?']) || href.is_empty() {
            return Some(standard);
        }
        let mut candidates = Vec::new();
        if let Some(r) = resolve(&self.package, href).filter(&exists) {
            candidates.push(r);
        }
        let mut stripped = href;
        for _ in 0..8 {
            if let Some(s) = stripped
                .strip_prefix("../")
                .or_else(|| stripped.strip_prefix("./"))
            {
                stripped = s;
            } else {
                break;
            }
        }
        if stripped != href
            && let Some(r) = resolve(&self.package, stripped).filter(&exists)
        {
            candidates.push(r);
        }
        candidates.sort_by(|a, b| a.path.cmp(&b.path));
        candidates.dedup_by(|a, b| a.path == b.path);
        match candidates.len() {
            1 => {
                note(
                    diagnostics,
                    base,
                    href,
                    "compatibility-opf-relative-reference",
                );
                candidates.pop()
            }
            0 => Some(standard),
            _ => {
                note(diagnostics, base, href, "ambiguous-compatibility-reference");
                Some(standard)
            }
        }
    }
}
fn note(diagnostics: &mut Vec<Diagnostic>, base: &Path, href: &str, reason: &'static str) {
    let document = base.to_string_lossy();
    if !diagnostics.iter().any(|d| {
        d.document == document && d.reference.as_deref() == Some(href) && d.reason == reason
    }) {
        diagnostics.push(Diagnostic {
            document: document.into_owned(),
            reference: Some(href.to_owned()),
            reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    #[test]
    fn prefers_standard_references_and_refuses_ambiguous_or_active_recovery() {
        let paths = [
            "OPS/text/image.png",
            "OPS/image.png",
            "image.png",
            "OPS/text/ch.xhtml",
            "photo.png",
            "OPS/photo.png",
        ];
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for path in paths {
            zip.start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"resource").unwrap();
        }
        let container = Container::new(zip.finish().unwrap()).unwrap();
        let mut diagnostics = Vec::new();
        let resolver = ReferenceResolver::new(
            PathBuf::from("OPS/book.opf"),
            paths.into_iter().map(|p| (p.into(), "image/png".into())),
            true,
            &mut diagnostics,
        );
        assert_eq!(
            resolver
                .resolve(
                    &container,
                    Path::new("OPS/text/ch.xhtml"),
                    "image.png",
                    &mut diagnostics
                )
                .unwrap()
                .path,
            "OPS/text/image.png"
        );
        assert!(diagnostics.is_empty());
        let ambiguous = resolver
            .resolve(
                &container,
                Path::new("OPS/text/nav.xhtml"),
                "../../image.png",
                &mut diagnostics,
            )
            .unwrap();
        // A legal exact root target wins even when alternative package targets exist.
        assert_eq!(ambiguous.path, "image.png");
        let ambiguous = resolver
            .resolve(
                &container,
                Path::new("OPS/text/nav.xhtml"),
                "../image.png",
                &mut diagnostics,
            )
            .unwrap();
        assert_eq!(ambiguous.path, "OPS/image.png");
        let ambiguous = resolver
            .resolve(
                &container,
                Path::new("OPS/text/deep/nav.xhtml"),
                "../photo.png",
                &mut diagnostics,
            )
            .unwrap();
        assert_eq!(ambiguous.path, "OPS/text/photo.png");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.reason == "ambiguous-compatibility-reference")
        );
        assert!(
            resolver
                .resolve(
                    &container,
                    Path::new("OPS/text/nav.xhtml"),
                    "javascript:alert(1)",
                    &mut diagnostics
                )
                .is_none()
        );
        assert!(
            resolver
                .resolve(
                    &container,
                    Path::new("OPS/text/nav.xhtml"),
                    "//example.com/image.png",
                    &mut diagnostics
                )
                .is_none()
        );
        assert_eq!(
            resolver
                .resolve(
                    &container,
                    Path::new("OPS/text/nav.xhtml"),
                    "text/ch.xhtml?mode=1#%E6%B3%A8%E9%87%8A",
                    &mut diagnostics
                )
                .unwrap()
                .fragment
                .as_deref(),
            Some("注释")
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.reason == "compatibility-opf-relative-reference")
        );
    }
}
