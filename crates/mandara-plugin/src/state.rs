//! Instance-scoped storage behind the plugin state import.

pub const MAX_STATE_ENTRIES: usize = 4096;
pub const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum StateStoreError {
    #[error("plugin state quota exceeded")]
    QuotaExceeded,
    #[error("plugin state backend failed: {0:#}")]
    Backend(#[from] anyhow::Error),
}

/// A handle already bound to one instance. The guest cannot choose a namespace.
/// Implementations make each operation atomic, with no multi-operation lock.
pub trait StateStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StateStoreError>;
    fn set(&self, key: &str, value: &[u8]) -> Result<(), StateStoreError>;
    fn delete(&self, key: &str) -> Result<bool, StateStoreError>;
}
