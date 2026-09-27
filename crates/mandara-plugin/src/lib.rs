//! Mandara plugin host: loads wasm components implementing the
//! `mandara:plugin/mandara-plugin` world and exposes per-instance
//! calls with configuration injection and epoch timeouts.
//!
//! See `wit/mandara.wit` for the plugin interface definition,
//! `docs/plugin-v2-design.md` for the design, and `docs/plugins.md` for
//! how to write and build a plugin.

pub mod host;
pub mod http_fetch;

pub use host::{
    BookEntry, ConfigErrors, ConfigField, ConfigFieldError, ConfigKind, ConfigValue, DeclaredBook,
    ImageStore, MAX_BOOK_FILE_BYTES, MAX_CALL_IMAGE_BYTES, MAX_CHAPTER_CONTENT_BYTES,
    MAX_DECLARE_BOOKS, MAX_DECLARE_CHAPTERS, MAX_IMAGE_BYTES, MAX_SEARCH_LIMIT, MAX_SEARCH_OFFSET,
    SearchResult, SourceInfo, WasmPlugin, load_dir, validate_config, values_from_config,
};
pub use http_fetch::{FetchError, FetchPolicy, FetchRequest, FetchResponse, MAX_REDIRECTS};
