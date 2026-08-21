//! Domain models: users, book metadata, files, chapters, reading sessions,
//! shares.

use serde::{Deserialize, Serialize};

/// User role. Administrators can manage every book and every file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            _ => None,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Private => "private",
            Visibility::Public => "public",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "private" => Some(Visibility::Private),
            "public" => Some(Visibility::Public),
            _ => None,
        }
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub created_at: String,
}

/// Pure book metadata. One entry can be linked to many
/// [`FileMeta`]s (formats/editions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookMeta {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
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
    /// `"epub"` | `"txt"` for uploads, `"plugin"` for plugin books.
    pub format: String,
    /// Short human label for this edition.
    pub label: String,
    pub visibility: Visibility,
    pub owner_id: Option<String>,
    pub chapter_count: u32,
    pub created_at: String,
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
    pub children: Vec<TocNode>,
}

/// Content format of a chapter. `epub` chapters carry sanitized HTML
/// (EPUB XHTML, structure preserved); `txt` and plugin chapters are plain
/// text. The web reader renders accordingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChapterFormat {
    Html,
    Text,
}

impl ChapterFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ChapterFormat::Html => "html",
            ChapterFormat::Text => "text",
        }
    }

    /// Parse a stored format string, falling back to plain text for
    /// unknown/legacy values.
    pub fn parse(s: &str) -> ChapterFormat {
        match s {
            "html" => ChapterFormat::Html,
            _ => ChapterFormat::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub idx: u32,
    pub title: String,
    /// `"html"` (sanitized EPUB XHTML) or `"text"` (plain text).
    pub format: ChapterFormat,
    pub content: String,
}

/// A reading position inside a file.
///
/// `chapter_idx` selects the chapter, `offset` is a character offset inside
/// the chapter (used by non-web readers), and `fraction` is a 0..1 scroll
/// fraction inside the chapter (used by the web reader).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    pub chapter_idx: u32,
    pub offset: u32,
    pub fraction: f64,
}

impl Default for Position {
    fn default() -> Self {
        Position {
            chapter_idx: 0,
            offset: 0,
            fraction: 0.0,
        }
    }
}

impl Position {
    /// Progress within the current chapter, as a percentage.
    pub fn percent(&self) -> u8 {
        (self.fraction.clamp(0.0, 1.0) * 100.0).round() as u8
    }

    /// Clamp the position so it is valid for a file with `chapter_count`
    /// chapters.
    pub fn clamped(mut self, chapter_count: u32) -> Self {
        self.chapter_idx = if chapter_count > 0 {
            self.chapter_idx.min(chapter_count - 1)
        } else {
            0
        };
        self.offset = 0;
        self.fraction = self.fraction.clamp(0.0, 1.0);
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
    pub updated_at: String,
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
    /// `"read"` (read-only) or `"progress"` (session progress view).
    pub mode: String,
    pub file_id: String,
    pub session_id: Option<String>,
    pub created_by: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}
