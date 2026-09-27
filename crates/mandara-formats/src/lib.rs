//! Parsers for the supported book formats: epub and txt.
//!
//! Every parser produces the same normalized representation
//! ([`ParsedBook`]), which the server then stores in the central library.

pub mod epub;
pub mod htmlize;
pub mod imgdim;
pub mod txt;

use std::path::Path;

use mandara_core::error::{Error, Result};
use mandara_core::ext::BookExt;
use mandara_core::model::TocNode;

/// A parsed book in normalized form.
#[derive(Debug, Clone)]
pub struct ParsedBook {
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Cover image extracted from the source file (epub only), stored by
    /// the server and served via `GET /api/books/{id}/cover`.
    pub cover: Option<CoverImage>,
    /// Embedded raster images extracted from the source (epub only), in
    /// first-reference order. Chapter HTML references them by placeholder
    /// (`src="image:{n}"`, `n` = index here); the server stores each image
    /// in the content-addressed image store at ingest and rewrites the
    /// placeholders to `/api/images/{id}`.
    pub images: Vec<ParsedImage>,
    pub chapters: Vec<ParsedChapter>,
    /// Hierarchical table of contents per the ebook's nav/NCX. `idx`
    /// references entries of `chapters`; pure group nodes have `None`.
    pub toc: Vec<TocNode>,
    /// Extended metadata extracted from the source (epub OPF: ISBN,
    /// publisher, dates, translators, language). Empty for txt.
    pub ext: BookExt,
}

/// Cover image bytes with their content type (e.g. `image/jpeg`).
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// An embedded chapter image extracted from the source file.
#[derive(Debug, Clone)]
pub struct ParsedImage {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// One chapter of a parsed book. `content` is canonical sanitized HTML
/// for every source (epub sanitizer output, txt/plugin text converted by
/// [`htmlize`]).
#[derive(Debug, Clone)]
pub struct ParsedChapter {
    pub title: String,
    pub content: String,
}

/// Parse `bytes` according to the extension of `filename`.
pub fn parse(bytes: &[u8], filename: &str) -> Result<ParsedBook> {
    let lower = filename.to_ascii_lowercase();
    let name = Path::new(&lower)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&lower);
    if name.ends_with(".epub") {
        epub::parse(bytes)
    } else if name.ends_with(".txt") || name.ends_with(".text") {
        txt::parse(bytes, name)
    } else {
        Err(Error::UnsupportedFormat(filename.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_extension() {
        let err = parse(b"hello", "book.pdf").unwrap_err();
        assert!(matches!(err, Error::UnsupportedFormat(_)));
    }

    #[test]
    fn parses_plain_txt() {
        let book = parse(b"hello world", "book.txt").unwrap();
        assert_eq!(book.title, "book");
        assert_eq!(book.chapters.len(), 1);
        assert_eq!(book.chapters[0].content, "<p>hello world</p>\n");
    }
}
