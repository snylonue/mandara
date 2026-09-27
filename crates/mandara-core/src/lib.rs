//! Core domain models shared by all mandara crates.
//!
//! This crate is intentionally dependency-light: no database or runtime
//! bindings live here. The server maps its own persistence rows into these
//! models.

pub mod error;
pub mod ext;
pub mod model;
pub mod source;

pub use error::{Error, Result};
pub use ext::{BookExt, SeriesExt, SeriesStatus, normalize_isbn};
pub use model::*;
pub use source::*;
