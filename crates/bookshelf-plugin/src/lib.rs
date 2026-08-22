//! Bookshelf plugin host: loads wasm components implementing the
//! `bookshelf:plugin/bookshelf-plugin` world and exposes per-instance
//! calls with configuration injection and epoch timeouts.
//!
//! See `wit/bookshelf.wit` for the plugin interface definition,
//! `docs/plugin-v2-design.md` for the design, and `docs/plugins.md` for
//! how to write and build a plugin.

pub mod host;

pub use host::{
    load_dir, validate_config, values_from_config, BookEntry, ConfigErrors, ConfigField,
    ConfigFieldError, ConfigKind, ConfigValue, DeclaredBook, SearchResult, WasmPlugin,
    MAX_CHAPTER_CONTENT_BYTES, MAX_DECLARE_BOOKS, MAX_DECLARE_CHAPTERS, MAX_SEARCH_LIMIT,
    MAX_SEARCH_OFFSET,
};
