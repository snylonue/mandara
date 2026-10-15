//! Core domain models shared by all mandara crates.
//!
//! Domain values include their SQLite mappings so persistence queries can
//! read and write the same types used by the application.

mod db;
pub mod error;
pub mod ext;
pub mod model;
pub mod source;

pub use error::{Error, Result};
pub use ext::{BookExt, SeriesExt, SeriesStatus, normalize_isbn};
pub use model::*;
pub use source::*;
