//! Parsers for the supported book formats: epub and txt.
//!
//! Every parser produces the same normalized representation
//! ([`ParsedBook`]), which the server then stores in the central library.

pub mod epub;
pub mod txt;

use std::path::Path;

use bookshelf_core::error::{Error, Result};
use bookshelf_core::model::{ChapterFormat, TocNode};

/// A parsed book in normalized form.
#[derive(Debug, Clone)]
pub struct ParsedBook {
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub chapters: Vec<ParsedChapter>,
    /// Hierarchical table of contents per the ebook's nav/NCX. `idx`
    /// references entries of `chapters`; pure group nodes have `None`.
    pub toc: Vec<TocNode>,
}

/// One chapter of a parsed book.
///
/// `content` is either sanitized HTML (`format == Html`, epub source) or
/// plain text (`format == Text`, txt source).
#[derive(Debug, Clone)]
pub struct ParsedChapter {
    pub title: String,
    pub format: ChapterFormat,
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
        assert_eq!(book.chapters[0].content, "hello world");
        assert_eq!(book.chapters[0].format, ChapterFormat::Text);
    }
}