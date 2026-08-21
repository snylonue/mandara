//! Bookshelf plugin host: loads wasm components implementing the
//! `bookshelf:plugin/bookshelf-plugin` world and exposes them as
//! [`BookSource`]s.
//!
//! See `wit/bookshelf.wit` for the plugin interface definition and
//! `docs/plugins.md` for how to write and build a plugin.

pub mod host;

pub use host::{load_dir, PluginManager, WasmPlugin};