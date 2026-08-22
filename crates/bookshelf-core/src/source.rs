//! Book entries as advertised by plugin sources.
//!
//! These are pure data structures: the metadata a plugin source offers and
//! the chapters it returns. The server materializes them into the central
//! library (unified metadata storage in SQLite) on sync / on demand.

/// A book as advertised by a plugin source.
#[derive(Debug, Clone)]
pub struct SourceBook {
    /// Stable id of the book *within the plugin instance*.
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// When set, the book's chapters come from another plugin instance
    /// (metadata/content source separation). `None` = this instance also
    /// provides the content.
    pub content_source: Option<String>,
    /// Book id inside the content source. `None` = same as `id`.
    pub content_id: Option<String>,
}

/// A chapter as returned by a plugin source.
#[derive(Debug, Clone)]
pub struct SourceChapter {
    pub title: String,
    pub content: String,
}
