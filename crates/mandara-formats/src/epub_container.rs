//! Exact OCF entry access, separate from URL resolution and dependency recovery.
use std::{
    collections::HashSet,
    io::{Read, Seek},
    path::Path,
};

pub(crate) struct Container<R: Read + Seek> {
    archive: zip::ZipArchive<R>,
    entries: HashSet<String>,
}

impl<R: Read + Seek> Container<R> {
    pub fn new(reader: R) -> std::result::Result<Self, zip::result::ZipError> {
        let archive = zip::ZipArchive::new(reader)?;
        let entries = archive.file_names().map(str::to_owned).collect();
        Ok(Self { archive, entries })
    }
    pub fn contains(&self, path: &Path) -> bool {
        path.to_str().is_some_and(|p| self.entries.contains(p))
    }
    pub fn bytes(&mut self, path: &Path) -> Option<Vec<u8>> {
        let mut file = self.archive.by_name(path.to_str()?).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }
    pub fn text(&mut self, path: &Path) -> Option<String> {
        decode_text(&self.bytes(path)?)
    }
}

/// EPUB XML/CSS resources support UTF-8 and BOM-marked UTF-16. Reject malformed
/// input rather than inserting replacement characters into resource URLs.
pub(crate) fn decode_text(bytes: &[u8]) -> Option<String> {
    let utf16 = bytes
        .strip_prefix(&[0xff, 0xfe])
        .map(|b| (encoding_rs::UTF_16LE, b))
        .or_else(|| {
            bytes
                .strip_prefix(&[0xfe, 0xff])
                .map(|b| (encoding_rs::UTF_16BE, b))
        });
    if let Some((encoding, data)) = utf16 {
        let (text, errors) = encoding.decode_without_bom_handling(data);
        return (!errors).then(|| text.into_owned());
    }
    String::from_utf8(
        bytes
            .strip_prefix(&[0xef, 0xbb, 0xbf])
            .unwrap_or(bytes)
            .to_vec(),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    #[test]
    fn canonical_paths_are_not_percent_decoded_again() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "OEBPS/name space.xhtml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"space").unwrap();
        zip.start_file(
            "OEBPS/name%20literal.xhtml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"literal percent").unwrap();
        let mut container = Container::new(zip.finish().unwrap()).unwrap();
        assert_eq!(
            container
                .text(Path::new("OEBPS/name space.xhtml"))
                .as_deref(),
            Some("space")
        );
        assert_eq!(
            container
                .text(Path::new("OEBPS/name%20literal.xhtml"))
                .as_deref(),
            Some("literal percent")
        );
        assert!(
            container
                .text(Path::new("OEBPS/name%20space.xhtml"))
                .is_none()
        );
    }
    #[test]
    fn decodes_utf8_and_utf16_without_lossy_replacement() {
        let text = "<p>中文正文</p>";
        for (bom, little) in [(vec![0xff, 0xfe], true), (vec![0xfe, 0xff], false)] {
            let mut bytes = bom;
            bytes.extend(text.encode_utf16().flat_map(|n| {
                if little {
                    n.to_le_bytes()
                } else {
                    n.to_be_bytes()
                }
            }));
            assert_eq!(decode_text(&bytes).as_deref(), Some(text));
        }
        assert_eq!(decode_text(text.as_bytes()).as_deref(), Some(text));
        assert_eq!(
            decode_text(b"\xef\xbb\xbf<p>text</p>").as_deref(),
            Some("<p>text</p>")
        );
        assert!(decode_text(&[0xff, 0xfe, 0x01]).is_none());
        assert!(decode_text(&[0xff]).is_none());
    }
}
