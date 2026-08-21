//! EPUB parsing (epub-rs + HTML to plain text).

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bookshelf_core::error::{Error, Result};
use scraper::{Html, Selector};

use crate::{ParsedBook, ParsedChapter};

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

fn parse_path(path: &PathBuf) -> Result<ParsedBook> {
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

    let n = doc.get_num_chapters();
    let mut chapters = Vec::new();
    for i in 0..n {
        if !doc.set_current_chapter(i) {
            continue;
        }
        if let Some((html, _mime)) = doc.get_current_str() {
            let title = extract_heading(&html).unwrap_or_else(|| format!("第 {} 章", i + 1));
            let content = html2text::config::plain()
                .string_from_read(&mut Cursor::new(html), 78)
                .map_err(|e| Error::InvalidArgument(format!("epub content: {e}")))?;
            chapters.push(ParsedChapter { title, content });
        }
    }

    if chapters.is_empty() {
        return Err(Error::InvalidArgument(
            "epub has no readable chapters".into(),
        ));
    }

    Ok(ParsedBook {
        title,
        authors,
        description,
        cover_url: None, // cover extraction (bytes) is not stored yet
        chapters,
    })
}

/// Best-effort chapter heading extraction from epub XHTML.
fn extract_heading(html: &str) -> Option<String> {
    let frag = Html::parse_fragment(html);
    for sel in ["h1", "h2", "h3", "title"] {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        if let Some(el) = frag.select(&selector).next() {
            let text: String = el.text().collect();
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}