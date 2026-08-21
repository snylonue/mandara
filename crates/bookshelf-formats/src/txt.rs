//! Plain-text book parsing: encoding detection + chapter splitting.
//!
//! Encodings: UTF-8 (BOM or valid), UTF-16 (BOM), otherwise GB18030 (common
//! for Chinese light novels downloaded as txt).

use bookshelf_core::error::{Error, Result};
use bookshelf_core::model::{ChapterFormat, TocNode};

use crate::{ParsedBook, ParsedChapter};

const HEADING_SUFFIXES: [char; 7] = ['章', '节', '回', '卷', '部', '篇', '集'];
const EXACT_HEADINGS: [&str; 12] = [
    "序章",
    "序言",
    "楔子",
    "前言",
    "尾声",
    "终章",
    "后记",
    "番外",
    "间章",
    "幕间",
    "人物介绍",
    "出场人物",
];

/// Parse txt bytes into a normalized book.
pub fn parse(bytes: &[u8], filename: &str) -> Result<ParsedBook> {
    let text = decode(bytes)?;
    let (title, chapters) = split_chapters(&text, filename);
    let toc = chapters
        .iter()
        .enumerate()
        .map(|(i, c)| TocNode {
            title: c.title.clone(),
            idx: Some(i as u32),
            frag: None,
            children: Vec::new(),
        })
        .collect();
    Ok(ParsedBook {
        title,
        authors: Vec::new(),
        description: None,
        cover_url: None,
        chapters,
        toc,
    })
}

/// Decode bytes into text, detecting the encoding.
fn decode(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(bytes[3..].to_vec())
            .map_err(|_| Error::InvalidArgument("invalid utf-8 content".into()));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let (cow, _enc, _had_errors) = encoding_rs::UTF_16LE.decode(bytes);
        return Ok(cow.into_owned());
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(s.to_string());
    }
    // Not valid utf-8: assume GB18030 (superset of GBK). This covers most
    // Chinese-shifted-JIS-era txt files floating around.
    let (cow, _enc, _had_errors) = encoding_rs::GB18030.decode(bytes);
    let mut s = cow.into_owned();
    // strip a possible replacement char if decoding was hopeless
    if s.starts_with('\u{FFFD}') {
        s = s.trim_start_matches('\u{FFFD}').to_string();
    }
    Ok(s)
}

/// Split text into chapters. Lines that look like chapter headings start a
/// new chapter; everything else is appended to the current one.
fn split_chapters(text: &str, filename: &str) -> (String, Vec<ParsedChapter>) {
    let mut chapters: Vec<ParsedChapter> = Vec::new();
    let mut cur_title = String::new();
    let mut cur = String::new();
    let mut heading_seen = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if is_heading(trimmed) && char_count(trimmed) <= 60 {
            if heading_seen {
                chapters.push(ParsedChapter {
                    title: std::mem::take(&mut cur_title),
                    format: ChapterFormat::Text,
                    content: std::mem::take(&mut cur),
                });
            }
            heading_seen = true;
            cur_title = trimmed.to_string();
        } else if heading_seen {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    if heading_seen {
        chapters.push(ParsedChapter {
            title: std::mem::take(&mut cur_title),
            format: ChapterFormat::Text,
            content: std::mem::take(&mut cur),
        });
    }

    let title = filename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(filename)
        .to_string();

    if chapters.is_empty() {
        chapters.push(ParsedChapter {
            title: title.clone(),
            format: ChapterFormat::Text,
            content: text.to_string(),
        });
    }
    (title, chapters)
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Heuristic for "does this line look like a chapter heading?"
fn is_heading(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    // Chapter-heading patterns for CJK light novels: an ordinal marker
    // followed by digits/CJK numerals and a unit suffix (chapter/section/
    // volume), as well as western "Chapter N" / "VOL.N" forms.
    // NOTE: the ordinal marker is 3 UTF-8 bytes, so slicing is char-safe.
    if line.starts_with('第') && line.len() > 3 {
        let rest = &line[3..];
        let han = [
            '零', '一', '二', '三', '四', '五', '六', '七', '八', '九', '十', '百', '千', '万',
            '两',
        ];
        let mut idx = 0;
        for ch in rest.chars() {
            if ch.is_ascii_digit() || ch.is_ascii_whitespace() || han.contains(&ch) {
                idx += ch.len_utf8();
            } else {
                break;
            }
        }
        if idx == 0 {
            return false;
        }
        let tail = &rest[idx..];
        return tail
            .chars()
            .next()
            .is_some_and(|c| HEADING_SUFFIXES.contains(&c));
    }
    // Chapter N / VOL.1 / Vol 01 (char-safe comparison: lines may start
    // with multi-byte characters)
    if starts_with_ascii_case_insensitive(line, "chapter") {
        return line
            .chars()
            .nth(7)
            .is_some_and(|c| c.is_ascii_digit() || c.is_whitespace());
    }
    if starts_with_ascii_case_insensitive(line, "vol") {
        return line
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_digit() || c.is_whitespace() || c == '.');
    }
    EXACT_HEADINGS.contains(&line)
}

/// Case-insensitive ASCII prefix check that never splits a UTF-8 char.
fn starts_with_ascii_case_insensitive(line: &str, prefix: &str) -> bool {
    let mut chars = line.chars();
    prefix
        .chars()
        .all(|p| chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_chinese_chapters() {
        let text = "第1章 开始\n正文一\n正文二\n\n第十二章 结束\n正文三\n";
        let (_t, chapters) = split_chapters(text, "book.txt");
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "第1章 开始");
        assert_eq!(chapters[0].content, "正文一\n正文二\n\n");
        assert_eq!(chapters[1].title, "第十二章 结束");
        assert_eq!(chapters[1].content, "正文三\n");
    }

    #[test]
    fn no_headings_means_single_chapter() {
        let text = "没有标题的\n一篇文字\n";
        let (title, chapters) = split_chapters(text, "novel.txt");
        assert_eq!(title, "novel");
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].content, text);
    }

    #[test]
    fn heading_heuristics() {
        assert!(is_heading("第1章 出发"));
        assert!(is_heading("第一百二十三章"));
        assert!(is_heading("Chapter 12"));
        assert!(is_heading("VOL.02"));
        assert!(is_heading("楔子"));
        assert!(!is_heading("这是一段正文"));
        assert!(!is_heading("第一个故事讲完了")); // no chapter suffix
        assert!(!is_heading("第 一 次 遇见")); // suffix check absent
    }

    #[test]
    fn decodes_gb18030() {
        // GB18030 bytes for "你好，世界"
        let bytes = [0xc4, 0xe3, 0xba, 0xc3, 0xa3, 0xac, 0xca, 0xc0, 0xbd, 0xe7];
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, "你好，世界");
    }
}
