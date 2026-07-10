//! Object storage port (provider-agnostic).

use crate::StorageError;

/// Put/get/list/delete for content and artifacts.
pub trait ObjectStorage: Send + Sync {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError>;
    fn get(&self, key: &str) -> Result<Vec<u8>, StorageError>;
    fn delete(&self, key: &str) -> Result<(), StorageError>;
    fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError>;
    fn backend_name(&self) -> &str;
}
