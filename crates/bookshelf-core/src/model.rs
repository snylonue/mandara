//! Domain models: users, book metadata, files, chapters, reading sessions,
//! shares.

use crate::ext::{BookExt, SeriesExt};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// User role. Administrators can manage every book and every file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl AsRef<str> for Role {
    fn as_ref(&self) -> &str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            _ => Err(format!("unknown role: {s}")),
        }
    }
}

/// Visibility of a file. `Private` files are only visible to their owner
/// (and admins); `Public` files are visible to every logged-in user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Public,
}

impl AsRef<str> for Visibility {
    fn as_ref(&self) -> &str {
        match self {
            Visibility::Private => "private",
            Visibility::Public => "public",
        }
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for Visibility {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "private" => Ok(Visibility::Private),
            "public" => Ok(Visibility::Public),
            _ => Err(format!("unknown visibility: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

/// A series: metadata grouping for the volumes of one publication family
/// (e.g. a light-novel series split from a multi-volume source). Member
/// books carry `series_id` + `volume_no` (1-based order).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesMeta {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Extended metadata (publisher/status/tags/…; see [`SeriesExt`]).
    #[serde(default = "SeriesExt::default")]
    pub ext: SeriesExt,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Pure book metadata. One entry can be linked to many
/// [`FileMeta`]s (formats/editions). When the entry is one volume of a
/// series, `series_id` points to it and `volume_no` is its 1-based
/// ordinal (`0` = standalone).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookMeta {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Extended metadata (ISBN/publisher/…; see [`BookExt`]).
    #[serde(default = "BookExt::default")]
    pub ext: BookExt,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Series this book is a volume of (`None` = standalone).
    pub series_id: Option<String>,
    /// 1-based volume ordinal inside the series (`0` = standalone).
    pub volume_no: u32,
}

/// One actual book file (a local upload or a virtual plugin book).
/// Chapters, sessions and shares attach to files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub id: String,
    pub book_id: String,
    /// `"local"` for uploads, otherwise the plugin id that provides it.
    pub source: String,
    /// Id of the book inside its source (for local files, same as `id`).
    pub external_id: String,
    /// When set, chapters come from another plugin instance instead of
    /// `source` (metadata/content source separation). NULL = same as
    /// `source` / `external_id`.
    pub content_source: Option<String>,
    pub content_external_id: Option<String>,
    /// Real format for uploads (`epub`/`txt`), `plugin` for virtual
    /// plugin books.
    pub format: FileFormat,
    /// Short human label for this edition.
    pub label: String,
    pub visibility: Visibility,
    pub owner_id: Option<String>,
    pub chapter_count: u32,
    pub created_at: DateTime<Utc>,
    /// Volume slice of the source book this file covers (0 = the whole
    /// book, today's semantics).
    pub volume_no: u32,
    /// Flat chapter index of this volume's first chapter inside the
    /// source (persisted because the plugin is stateless).
    pub volume_offset: u32,
    /// Retained original file bytes (`data/files/{id}.{ext}`), when the
    /// content came with one (uploads, plugin file-mode pulls). Enables
    /// `GET /api/files/{id}/download` and `--reparse-originals`.
    /// `None` = plugin chapter-mode files and pre-retention uploads.
    pub original: Option<OriginalInfo>,
}

/// Metadata of a file's retained original bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginalInfo {
    /// Original file extension without the dot (`epub` / `txt`).
    pub ext: String,
    pub size: u64,
    pub sha256: String,
}

/// Id of an image in the content-addressed image store: the lowercase hex
/// sha-256 digest of its bytes (64 chars). The id doubles as a disk
/// filename and a `/api/images/{id}` URL path segment, so it is validated
/// at construction — any [`ImageId`] in circulation is guaranteed to be a
/// plain hex string and safe to embed in paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageId(String);

impl ImageId {
    /// Validate raw sha-256 hex output (exactly 64 lowercase hex chars).
    /// Returns the rejected input on failure.
    pub fn from_sha256_hex(id: impl Into<String>) -> Result<Self, String> {
        let id = id.into();
        let is_lower_hex = id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'));
        if is_lower_hex { Ok(Self(id)) } else { Err(id) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ImageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for ImageId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_sha256_hex(s.to_owned()).map_err(|bad| format!("invalid image id: {bad}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_id_accepts_sha256_hex_only() {
        let ok = "0123456789abcdef".repeat(4);
        assert!(ImageId::from_sha256_hex(ok.clone()).is_ok());
        assert_eq!(ImageId::from_sha256_hex(ok).unwrap().as_str().len(), 64);

        // uppercase hex, wrong length, path traversal, empty
        assert!(ImageId::from_sha256_hex("A".repeat(64)).is_err());
        assert!(ImageId::from_sha256_hex("a".repeat(63)).is_err());
        assert!(ImageId::from_sha256_hex("../../etc/passwd").is_err());
        assert!(ImageId::from_sha256_hex("").is_err());
    }

    /// The wire format is the documented one (docs/api/openapi.yaml):
    /// `in_chapter` is `{"fraction": 0.5}` or `{"offset": 120}`, never a
    /// bare number (which cannot tell the two channels apart).
    #[test]
    fn in_chapter_wire_format() {
        let fraction: InChapter =
            serde_json::from_str(r#"{"fraction":0.5}"#).expect("fraction accepted");
        assert_eq!(fraction, InChapter::Fraction { fraction: 0.5 });
        assert_eq!(
            serde_json::to_string(&fraction).unwrap(),
            r#"{"fraction":0.5}"#
        );

        let offset: InChapter = serde_json::from_str(r#"{"offset":120}"#).expect("offset accepted");
        assert_eq!(offset, InChapter::CharOffset { offset: 120 });
        assert_eq!(serde_json::to_string(&offset).unwrap(), r#"{"offset":120}"#);

        // both channels, a bare number and an unknown key are all rejected
        assert!(serde_json::from_str::<InChapter>(r#"{"fraction":0.5,"offset":120}"#).is_err());
        assert!(serde_json::from_str::<InChapter>("0.5").is_err());
        assert!(serde_json::from_str::<InChapter>(r#"{"nope":1}"#).is_err());

        let pos: Position =
            serde_json::from_str(r#"{"chapter_idx":3,"in_chapter":{"fraction":0.25}}"#)
                .expect("position accepted");
        assert_eq!(pos.chapter_idx, 3);
        assert_eq!(pos.fraction(), 0.25);
        assert_eq!(
            serde_json::to_string(&pos).unwrap(),
            r#"{"chapter_idx":3,"in_chapter":{"fraction":0.25}}"#
        );
    }
}

/// Content format of a book file. Uploads keep their real format
/// (`Epub`/`Txt`); plugin chapter-mode books are `Plugin`, until a file-mode
/// pull materializes them into their actual format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Epub,
    Txt,
    Plugin,
}

impl AsRef<str> for FileFormat {
    fn as_ref(&self) -> &str {
        match self {
            FileFormat::Epub => "epub",
            FileFormat::Txt => "txt",
            FileFormat::Plugin => "plugin",
        }
    }
}

impl std::fmt::Display for FileFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for FileFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "epub" => Ok(FileFormat::Epub),
            "txt" => Ok(FileFormat::Txt),
            "plugin" => Ok(FileFormat::Plugin),
            _ => Err(format!("unknown file format: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterMeta {
    pub idx: u32,
    pub title: String,
}

/// One node of a hierarchical table of contents.
///
/// `idx` is the chapter index this entry points to (`None` for pure group
/// entries); the book's spine order stays the flat reading order in
/// `ChapterMeta` lists, this tree adds the author's structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TocNode {
    pub title: String,
    pub idx: Option<u32>,
    /// Leading fragment of the source href (`file.html#section`), when
    /// this entry points into the middle of its chapter. The web reader
    /// scrolls to the element with that id after loading the chapter.
    #[serde(default)]
    pub frag: Option<String>,
    pub children: Vec<TocNode>,
}

/// One chapter of a stored book: canonical sanitized HTML (epub
/// chapters, txt paragraphs, plugin text with the illustration
/// conventions expanded at the ingest boundary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub idx: u32,
    pub title: String,
    pub content: String,
}

impl ShareKind {
    /// The share view mode implied by the kind (`"read"` for file shares,
    /// `"progress"` for session shares).
    pub fn mode(self) -> &'static str {
        match self {
            ShareKind::Book => "read",
            ShareKind::Session => "progress",
        }
    }
}

impl AsRef<str> for ShareKind {
    fn as_ref(&self) -> &str {
        match self {
            ShareKind::Book => "book",
            ShareKind::Session => "session",
        }
    }
}

impl std::fmt::Display for ShareKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl std::str::FromStr for ShareKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "book" => Ok(ShareKind::Book),
            "session" => Ok(ShareKind::Session),
            _ => Err(format!("unknown share kind: {s}")),
        }
    }
}

/// How far inside a chapter a reading position is. The web reader tracks
/// a 0..1 scroll `Fraction`; non-web readers track a character `CharOffset`.
/// Exactly one of the two is set on the wire (`{"fraction": 0.5}` or
/// `{"offset": 120}`, see `docs/api/openapi.yaml`); a payload carrying
/// both is rejected at deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(untagged)]
pub enum InChapter {
    /// Scroll fraction within the chapter, 0..1 (web reader).
    Fraction {
        /// 0..1 scroll position inside the chapter.
        fraction: f64,
    },
    /// Character offset within the chapter (non-web readers).
    CharOffset {
        /// Character offset inside the chapter.
        offset: u32,
    },
}

impl<'de> Deserialize<'de> for InChapter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Fraction(FractionWire),
            CharOffset(OffsetWire),
        }

        // `deny_unknown_fields` makes each channel reject the other's key,
        // so sending both (or a stray key) fails instead of silently
        // dropping one of them.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FractionWire {
            fraction: f64,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OffsetWire {
            offset: u32,
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Fraction(f) => InChapter::Fraction {
                fraction: f.fraction,
            },
            Wire::CharOffset(o) => InChapter::CharOffset { offset: o.offset },
        })
    }
}

impl Default for InChapter {
    fn default() -> Self {
        InChapter::Fraction { fraction: 0.0 }
    }
}

/// A reading position inside a file.
///
/// `chapter_idx` selects the chapter; `in_chapter` is the progress inside
/// it (scroll fraction or character offset — see [`InChapter`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    pub chapter_idx: u32,
    pub in_chapter: InChapter,
}

impl Position {
    /// The web-reader progress fraction within the chapter (0.0 when the
    /// position is a character offset).
    pub fn fraction(&self) -> f64 {
        match self.in_chapter {
            InChapter::Fraction { fraction } => fraction,
            InChapter::CharOffset { .. } => 0.0,
        }
    }

    /// The non-web-reader character offset within the chapter (0 when the
    /// position is a scroll fraction).
    pub fn offset(&self) -> u32 {
        match self.in_chapter {
            InChapter::Fraction { .. } => 0,
            InChapter::CharOffset { offset } => offset,
        }
    }

    pub fn percent(&self) -> u8 {
        (self.fraction().clamp(0.0, 1.0) * 100.0).round() as u8
    }

    /// Clamp the position so it is valid for a file with `chapter_count`
    /// chapters.
    pub fn clamped(mut self, chapter_count: u32) -> Self {
        self.chapter_idx = if chapter_count > 0 {
            self.chapter_idx.min(chapter_count - 1)
        } else {
            0
        };
        self.in_chapter = match self.in_chapter {
            InChapter::Fraction { fraction } => InChapter::Fraction {
                fraction: fraction.clamp(0.0, 1.0),
            },
            c => c,
        };
        self
    }
}

/// A reading session: one device context reading one file.
///
/// A user can have several sessions per file (e.g. phone + laptop), each
/// with its own progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingSession {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub label: String,
    pub position: Position,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareKind {
    /// Share of a file: anyone with the link can read it.
    Book,
    /// Share of a session: anyone with the link can follow that session's
    /// progress (e.g. to keep in sync with a friend or another device).
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    pub token: String,
    pub kind: ShareKind,
    pub file_id: String,
    pub session_id: Option<String>,
    pub created_by: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
