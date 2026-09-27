//! Extended metadata for books and series (bangumi/douban-style fields).
//!
//! One JSON object column per table (`books.ext_meta`, `series.ext_meta`)
//! holds a [`BookExt`] / [`SeriesExt`] object: every field is optional,
//! unknown keys round-trip verbatim through the `extra` passthrough map,
//! so adding future fields is a Rust-only change (no migration).
//! See `docs/metadata-ext-design.md`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Normalize an ISBN for storage: strip separators, verify the checksum,
/// convert valid ISBN-10 to its canonical ISBN-13 form. Returns the
/// hyphenless ISBN-13 or a descriptive error — invalid input never lands.
pub fn normalize_isbn(raw: &str) -> Result<String, String> {
    let cleaned: String = raw.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
    match cleaned.len() {
        13 => {
            if !cleaned.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("invalid ISBN-13 `{raw}`"));
            }
            if isbn13_check(&cleaned) != cleaned.as_bytes()[12] - b'0' {
                return Err(format!("ISBN-13 checksum mismatch: `{raw}`"));
            }
            Ok(cleaned)
        }
        10 => {
            let bytes = cleaned.as_bytes();
            let digits: Result<Vec<u8>, String> = bytes[..9]
                .iter()
                .map(|b| {
                    b.checked_sub(b'0')
                        .filter(|d| *d <= 9)
                        .ok_or_else(|| format!("invalid ISBN-10 `{raw}`"))
                })
                .collect();
            let digits = digits?;
            let check = match bytes[9] {
                b'X' => 10,
                b if b.is_ascii_digit() => b - b'0',
                _ => return Err(format!("invalid ISBN-10 `{raw}`")),
            };
            let sum: u32 = digits
                .iter()
                .enumerate()
                .map(|(i, d)| ((10 - i) as u32) * (*d as u32))
                .sum::<u32>()
                + check as u32;
            if !sum.is_multiple_of(11) {
                return Err(format!("ISBN-10 checksum mismatch: `{raw}`"));
            }
            // Canonicalize to ISBN-13: 978 prefix + first 9 digits + new check.
            let mut base = format!("978{}", &cleaned[..9]);
            base.push_str(&(isbn13_check(&base)).to_string());
            Ok(base)
        }
        _ => Err(format!("not an ISBN-10/13: `{raw}`")),
    }
}

/// The ISBN-13 check digit of the first 12 digits.
fn isbn13_check(first12: &str) -> u8 {
    let sum: u32 = first12
        .bytes()
        .take(12)
        .enumerate()
        .map(|(i, b)| if i % 2 == 0 { 1 } else { 3 } * (b - b'0') as u32)
        .sum();
    ((10 - sum % 10) % 10) as u8
}

fn trimmed(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn opt_trim(v: &mut Option<String>) {
    *v = v.as_deref().and_then(trimmed);
}

/// Drop `null` entries and non-object junk from the passthrough map.
fn clean_extra(extra: &mut Map<String, Value>) {
    extra.retain(|_, v| !v.is_null());
}

/// Extended book metadata. Every known key is optional; unknown keys are
/// preserved verbatim (`extra`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BookExt {
    /// 副标题
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// 原作名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    /// Canonical hyphenless ISBN-13 (inputs are normalized on storage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
    /// 出版社
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// 出版时间, stored exactly as supplied ("2012-5" style included)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// 译者
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translators: Option<Vec<String>>,
    /// 插画师
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub illustrators: Option<Vec<String>>,
    /// 页数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    /// 定价, currency as printed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// 装帧 (文库/单行本/…)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    /// BCP-47 language hint ("zh-CN", "ja")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Anything not modeled yet — preserved round-trip.
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

impl BookExt {
    /// Parse a raw JSON object (plugin `extra`, stored column) into a
    /// validated, normalized `BookExt`. Fails on non-objects and invalid
    /// ISBNs — bad data never lands.
    pub fn from_value(value: Value) -> Result<Self, String> {
        let this: Self = serde_json::from_value(value)
            .map_err(|e| format!("invalid extended metadata object: {e}"))?;
        let mut this = this;
        this.sanitize()?;
        Ok(this)
    }

    /// Trim strings (empty → `None`), normalize the ISBN, drop null
    /// passthrough entries.
    pub fn sanitize(&mut self) -> Result<(), String> {
        opt_trim(&mut self.subtitle);
        opt_trim(&mut self.original_title);
        self.isbn = match self.isbn.as_deref().map(str::trim) {
            None => None,
            Some("") => None,
            Some(raw) => Some(normalize_isbn(raw)?),
        };
        opt_trim(&mut self.publisher);
        opt_trim(&mut self.pub_date);
        opt_trim(&mut self.price);
        opt_trim(&mut self.binding);
        opt_trim(&mut self.language);
        if let Some(list) = &mut self.translators {
            list.retain(|t| !t.trim().is_empty());
            for t in list.iter_mut() {
                *t = t.trim().to_string();
            }
            if list.is_empty() {
                self.translators = None;
            }
        }
        if let Some(list) = &mut self.illustrators {
            list.retain(|t| !t.trim().is_empty());
            for t in list.iter_mut() {
                *t = t.trim().to_string();
            }
            if list.is_empty() {
                self.illustrators = None;
            }
        }
        clean_extra(&mut self.extra);
        Ok(())
    }

    /// Apply create-time manual overrides: every `Some` key replaces the
    /// target's value; passthrough keys present in `o` win too.
    pub fn apply_override(&mut self, o: &Self) {
        if o.subtitle.is_some() {
            self.subtitle = o.subtitle.clone();
        }
        if o.original_title.is_some() {
            self.original_title = o.original_title.clone();
        }
        if o.isbn.is_some() {
            self.isbn = o.isbn.clone();
        }
        if o.publisher.is_some() {
            self.publisher = o.publisher.clone();
        }
        if o.pub_date.is_some() {
            self.pub_date = o.pub_date.clone();
        }
        if o.translators.is_some() {
            self.translators = o.translators.clone();
        }
        if o.illustrators.is_some() {
            self.illustrators = o.illustrators.clone();
        }
        if o.pages.is_some() {
            self.pages = o.pages;
        }
        if o.price.is_some() {
            self.price = o.price.clone();
        }
        if o.binding.is_some() {
            self.binding = o.binding.clone();
        }
        if o.language.is_some() {
            self.language = o.language.clone();
        }
        for (k, v) in &o.extra {
            self.extra.insert(k.clone(), v.clone());
        }
    }

    /// Serialized column form (`"{}"` when empty).
    pub fn to_column(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// Parse a stored column (tolerant: unreadable content reads as
    /// empty, matching the `authors` column behavior).
    pub fn from_column(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }
}

/// Publication status of a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeriesStatus {
    Ongoing,
    Completed,
    Hiatus,
}

impl AsRef<str> for SeriesStatus {
    fn as_ref(&self) -> &str {
        match self {
            SeriesStatus::Ongoing => "ongoing",
            SeriesStatus::Completed => "completed",
            SeriesStatus::Hiatus => "hiatus",
        }
    }
}

impl std::fmt::Display for SeriesStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for SeriesStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ongoing" => Ok(SeriesStatus::Ongoing),
            "completed" => Ok(SeriesStatus::Completed),
            "hiatus" => Ok(SeriesStatus::Hiatus),
            _ => Err(format!(
                "unknown series status: `{s}` (ongoing | completed | hiatus)"
            )),
        }
    }
}

/// Extended series metadata (edition-independent facts that belong to the
/// publication family, not to one volume).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SeriesExt {
    /// 原作名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    /// 出版社 (usually constant across a series)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// 首卷发售时间, verbatim
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// BCP-47 language hint
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Publication status
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SeriesStatus>,
    /// Planned volume count
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_volumes: Option<u32>,
    /// 类型标签
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Anything not modeled yet — preserved round-trip.
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

impl SeriesExt {
    /// Parse a raw JSON object into a validated `SeriesExt`.
    pub fn from_value(value: Value) -> Result<Self, String> {
        let mut this: Self = serde_json::from_value(value)
            .map_err(|e| format!("invalid extended metadata object: {e}"))?;
        this.sanitize()?;
        Ok(this)
    }

    pub fn sanitize(&mut self) -> Result<(), String> {
        opt_trim(&mut self.original_title);
        opt_trim(&mut self.publisher);
        opt_trim(&mut self.pub_date);
        opt_trim(&mut self.language);
        if let Some(list) = &mut self.tags {
            list.retain(|t| !t.trim().is_empty());
            for t in list.iter_mut() {
                *t = t.trim().to_string();
            }
            if list.is_empty() {
                self.tags = None;
            }
        }
        clean_extra(&mut self.extra);
        Ok(())
    }

    /// Apply create-time manual overrides (every `Some` key wins).
    pub fn apply_override(&mut self, o: &Self) {
        if o.original_title.is_some() {
            self.original_title = o.original_title.clone();
        }
        if o.publisher.is_some() {
            self.publisher = o.publisher.clone();
        }
        if o.pub_date.is_some() {
            self.pub_date = o.pub_date.clone();
        }
        if o.language.is_some() {
            self.language = o.language.clone();
        }
        if o.status.is_some() {
            self.status = o.status;
        }
        if o.total_volumes.is_some() {
            self.total_volumes = o.total_volumes;
        }
        if o.tags.is_some() {
            self.tags = o.tags.clone();
        }
        for (k, v) in &o.extra {
            self.extra.insert(k.clone(), v.clone());
        }
    }

    /// Extract the series-relevant subset from a plugin source's ext
    /// object (the same object a book entry carries; keys resolve through
    /// both shapes' shared passthrough).
    pub fn from_source(value: Option<&Value>) -> Result<Self, String> {
        match value {
            None => Ok(Self::default()),
            Some(v) => Self::from_value(v.clone()),
        }
    }

    pub fn to_column(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_column(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn isbn13_passes_through_normalized() {
        assert_eq!(normalize_isbn("9787536692930").unwrap(), "9787536692930");
        // separators stripped
        assert_eq!(
            normalize_isbn("978-7-5366-9293-0").unwrap(),
            "9787536692930"
        );
    }

    #[test]
    fn isbn10_converts_to_13() {
        // 1-55860-832-X is a valid ISBN-10 (check sum 242 ≡ 0 mod 11)
        assert_eq!(normalize_isbn("1-55860-832-X").unwrap(), "9781558608320");
        assert_eq!(normalize_isbn("0306406152").unwrap(), "9780306406157");
    }

    #[test]
    fn isbn_rejects_bad_input() {
        for bad in [
            "",
            "abc",
            "12345678901234", // wrong length
            "9787536692931",  // bad check digit
            "0306406153",     // bad ISBN-10 checksum
            "97875366929X",   // X outside ISBN-10
        ] {
            assert!(normalize_isbn(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn book_ext_sanitizes_and_normalizes() {
        let mut ext = BookExt {
            subtitle: Some("  副标题 ".into()),
            isbn: Some("978-7-5366-9293-0".into()),
            pages: Some(256),
            ..Default::default()
        };
        ext.sanitize().unwrap();
        assert_eq!(ext.subtitle.as_deref(), Some("副标题"));
        assert_eq!(ext.isbn.as_deref(), Some("9787536692930"));
    }

    #[test]
    fn book_ext_round_trips_unknown_keys() {
        let raw = json!({
            "publisher": "电击文库",
            "future_field": {"nested": true},
            "nulled": null
        });
        let ext = BookExt::from_value(raw).unwrap();
        assert_eq!(ext.publisher.as_deref(), Some("电击文库"));
        assert_eq!(
            ext.extra.get("future_field"),
            Some(&json!({"nested": true}))
        );
        assert!(!ext.extra.contains_key("nulled"), "nulls are dropped");
        // round-trip preserves them
        let back = BookExt::from_column(&ext.to_column());
        assert_eq!(back, ext);
    }

    #[test]
    fn book_ext_rejects_invalid_isbn_and_non_objects() {
        assert!(BookExt::from_value(json!({"isbn": "123"})).is_err());
        assert!(BookExt::from_value(json!([1, 2])).is_err());
    }

    #[test]
    fn book_ext_apply_override_wins_per_key() {
        let mut target = BookExt {
            publisher: Some("原出版社".into()),
            pages: Some(100),
            ..Default::default()
        };
        let o = BookExt {
            isbn: Some("9787536692930".into()),
            pages: Some(200),
            ..Default::default()
        };
        target.apply_override(&o);
        assert_eq!(target.publisher.as_deref(), Some("原出版社")); // kept
        assert_eq!(target.pages, Some(200)); // overridden
        assert_eq!(target.isbn.as_deref(), Some("9787536692930")); // added
    }

    #[test]
    fn series_ext_parses_status_and_from_source() {
        let ext =
            SeriesExt::from_value(json!({"status": "completed", "total_volumes": 12})).unwrap();
        assert_eq!(ext.status, Some(SeriesStatus::Completed));
        assert_eq!(ext.total_volumes, Some(12));

        assert!(SeriesExt::from_value(json!({"status": "dropped"})).is_err());

        let from_book = SeriesExt::from_source(Some(
            &json!({"publisher": "小学馆", "tags": ["ラノベ", " "] }),
        ))
        .unwrap();
        assert_eq!(from_book.publisher.as_deref(), Some("小学馆"));
        assert_eq!(from_book.tags, Some(vec!["ラノベ".to_string()]));
        assert_eq!(SeriesExt::from_source(None).unwrap(), SeriesExt::default());
    }

    #[test]
    fn empty_strings_become_none() {
        let mut ext = BookExt {
            publisher: Some("  ".into()),
            price: Some(String::new()),
            ..Default::default()
        };
        ext.sanitize().unwrap();
        assert_eq!(ext, BookExt::default());
        assert_eq!(ext.to_column(), "{}");
    }
}
