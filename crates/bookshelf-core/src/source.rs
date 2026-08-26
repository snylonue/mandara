//! Book entries as advertised by plugin sources.
//!
//! These are pure data structures: the metadata a plugin source offers and
//! the chapters it returns. The server materializes them into the central
//! library (unified metadata storage in SQLite) on sync / on demand.

/// One volume of a multi-volume source book (see WIT `volume-info`).
#[derive(Debug, Clone)]
pub struct SourceVolume {
    /// Volume title (e.g. `第一卷`); the host falls back to `第N卷` when
    /// empty.
    pub title: String,
    /// Chapters in this volume, in the flat reading order.
    pub chapter_count: u32,
}

/// A book as advertised by a plugin source.
#[derive(Debug, Clone)]
pub struct SourceBook {
    /// Stable id of the book *within the plugin instance*.
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Extended metadata as a raw JSON object (`BookExt` shape plus
    /// free-form extras; see `docs/metadata-ext-design.md`). Validated at
    /// the storage boundary; `None` = no extended metadata.
    pub ext: Option<serde_json::Value>,
    /// When set, the book's chapters come from another plugin instance
    /// (metadata/content source separation). `None` = this instance also
    /// provides the content.
    pub content_source: Option<String>,
    /// Book id inside the content source. `None` = same as `id`.
    pub content_id: Option<String>,
    /// Volume structure when the source book spans several volumes;
    /// empty = single volume. The host auto-splits acquisition into one
    /// series + one library book per volume when >1 volumes are declared.
    pub volumes: Vec<SourceVolume>,
}

/// A chapter as returned by a plugin source.
#[derive(Debug, Clone)]
pub struct SourceChapter {
    pub title: String,
    pub content: String,
}

/// A whole book file as a plugin source stores it (epub/txt download,
/// `get-book-file`). The server parses it with the upload pipeline and
/// stores it like a local file — afterwards the plugin is not consulted
/// for content.
#[derive(Debug, Clone)]
pub struct SourceBookFile {
    /// Original download name; the extension selects the parser.
    pub filename: String,
    /// MIME type as reported by the source.
    pub mime: String,
    /// File bytes (epub or txt).
    pub bytes: Vec<u8>,
}
