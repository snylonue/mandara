//! Book entries as advertised by plugin sources.
//!
//! These are pure data structures: the metadata a plugin source offers and
//! the chapters it returns. The server materializes them into the central
//! library (unified metadata storage in SQLite) on sync / on demand.

/// Series metadata advertised by one plugin instance.
#[derive(Debug, Clone)]
pub struct SourceSeries {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub ext: Option<serde_json::Value>,
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
    pub series: Option<SourceSeries>,
    pub volume_no: u32,
}

/// A chapter as returned by a plugin source.
#[derive(Debug, Clone)]
pub struct SourceChapter {
    pub title: String,
    /// How `content` is interpreted (the WIT `chapter.format`): `"text"`
    /// (default) = plain text, normalized by the host; `"html"` = the
    /// plugin HTML, sanitized before image-reference annotation.
    pub format: String,
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
