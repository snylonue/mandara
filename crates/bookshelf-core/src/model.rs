//! Domain models: users, books, chapters, reading sessions, shares.

use serde::{Deserialize, Serialize};

/// User role. Administrators can manage every book and every user.
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

/// Visibility of a book. `Private` books are only visible to their owner
/// (and admins); `Public` books are visible to every logged-in user.
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

/// Metadata of a book. Chapters are stored separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookMeta {
    pub id: String,
    /// Source of the book: `"local"` for uploaded files, or a plugin id.
    pub source: String,
    /// Id of the book inside its source (for local books, same as `id`).
    pub external_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub idx: u32,
    pub title: String,
    pub content: String,
}

/// A reading position inside a book.
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
    /// Overall progress within the book, as a percentage of the current
    /// chapter fraction. (A full-book percentage would additionally need the
    /// total chapter count — see `percent_of`.)
    pub fn percent(&self) -> u8 {
        (self.fraction.clamp(0.0, 1.0) * 100.0).round() as u8
    }

    /// Clamp the position so it is valid for a book with `chapter_count`
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

/// A reading session: one device context reading one book.
///
/// A user can have several sessions per book (e.g. phone + laptop), each with
/// its own progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingSession {
    pub id: String,
    pub user_id: String,
    pub book_id: String,
    pub label: String,
    pub position: Position,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareKind {
    /// Share of a book: anyone with the link can read it.
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
    pub book_id: String,
    pub session_id: Option<String>,
    pub created_by: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}