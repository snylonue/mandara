//! Test helpers shared across modules.

/// A scratch directory under the temp area, removed when dropped.
///
/// The prefix keeps the directory recognisable next to the real data
/// directory while a test is running.
pub(crate) fn tempdir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("create scratch directory")
}
