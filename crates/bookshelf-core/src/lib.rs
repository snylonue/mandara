//! Core domain models shared by all bookshelf crates.
//!
//! This crate is intentionally dependency-light: no database or runtime
//! bindings live here. The server maps its own persistence rows into these
//! models.

pub mod error;
pub mod model;
pub mod source;

pub use error::{Error, Result};
pub use model::*;
pub use source::*;