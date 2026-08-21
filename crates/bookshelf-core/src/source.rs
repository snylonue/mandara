//! The `BookSource` trait: the seam used by the wasm plugin system.
//!
//! Books provided by a source are *catalogued* into the central library
//! (unified metadata storage in SQLite) and their chapters are materialized
//! into the same store on first access.

use async_trait::async_trait;

use crate::error::Result;

/// A book as advertised by a source.
#[derive(Debug, Clone)]
pub struct SourceBook {
    /// Stable id of the book *within the plugin*.
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

/// A chapter as returned by a source.
#[derive(Debug, Clone)]
pub struct SourceChapter {
    pub title: String,
    pub content: String,
}

/// A pluggable provider of books and their content.
///
/// The server keeps a list of `BookSource`s: the built-in `"local"` source
/// (uploaded epub/txt files, stored fully in the database) plus one source
/// per loaded wasm plugin.
#[async_trait]
pub trait BookSource: Send + Sync {
    /// Stable identifier of this source (used in the `books.source` column).
    fn id(&self) -> &str;

    /// List all books offered by this source.
    async fn list_books(&self) -> Result<Vec<SourceBook>>;

    /// Chapter titles of a book, in reading order.
    /// Indices returned here are the same indices passed to `get_chapter`.
    async fn chapter_titles(&self, book_id: &str) -> Result<Vec<String>>;

    /// Fetch one chapter. `None` means the book/id does not exist.
    async fn get_chapter(&self, book_id: &str, index: u32) -> Result<Option<SourceChapter>>;
}