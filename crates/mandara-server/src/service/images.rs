//! Server-side image store backing the plugin `store-image` import.
//!
//! Plugin calls run on blocking threads (wasmtime is synchronous), while
//! the actual storage lives behind the async sqlx pool — so the
//! implementation captures the runtime handle at construction and
//! `block_on`s the async store helper from the import call.

use std::path::PathBuf;
use std::sync::Arc;

use crate::db::DieselDb;

use super::library::store_image;

/// Content-addressed image store for plugin imports: bytes under
/// `{files_dir}/images/{sha256}`, row in the `images` table. Dedup by
/// content — identical bytes return the same id.
#[derive(Clone)]
pub struct DbImageStore {
    db: DieselDb,
    files_dir: PathBuf,
    rt: tokio::runtime::Handle,
}

impl DbImageStore {
    pub fn new(db: DieselDb, files_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            db,
            files_dir,
            rt: tokio::runtime::Handle::current(),
        })
    }
}

impl mandara_plugin::host::ImageStore for DbImageStore {
    fn store_image(&self, bytes: &[u8], mime: &str) -> std::result::Result<String, String> {
        self.rt
            .block_on(store_image(&self.db, &self.files_dir, bytes, mime))
            .map(|id| id.to_string())
            .map_err(|e| e.to_string())
    }
}
