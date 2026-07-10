//! DynamoDB metadata adapter (RT-015) — honest `not_implemented` until AWS SDK.
//!
//! Same conceptual port as `FileMetaStore` (put/get/delete/list by kind+id).
//! Select with `VEIL_META=ddb`. Requires table name env; currently always fails
//! closed so LocalStack/AWS paths are explicit and never silently no-op.

use crate::StorageError;

/// DynamoDB-backed meta store placeholder.
#[derive(Debug, Clone)]
pub struct DdbMetaStore {
    table: String,
    region: String,
    endpoint: Option<String>,
}

impl DdbMetaStore {
    pub fn new(table: impl Into<String>, region: impl Into<String>, endpoint: Option<String>) -> Self {
        Self {
            table: table.into(),
            region: region.into(),
            endpoint,
        }
    }

    pub fn from_env() -> Result<Self, StorageError> {
        let table = std::env::var("VEIL_DDB_TABLE").map_err(|_| {
            StorageError::Config(
                "VEIL_DDB_TABLE required for VEIL_META=ddb (e.g. veil-meta)".into(),
            )
        })?;
        let region = std::env::var("VEIL_DDB_REGION")
            .or_else(|_| std::env::var("AWS_REGION"))
            .unwrap_or_else(|_| "us-east-1".into());
        let endpoint = std::env::var("VEIL_DDB_ENDPOINT").ok();
        Ok(Self::new(table, region, endpoint))
    }

    fn not_impl(&self, op: &str) -> StorageError {
        StorageError::NotImplemented(format!(
            "DynamoDB meta {op} not yet wired (table={}, region={}, endpoint={:?}). \
             Use VEIL_META=fs (FileMetaStore) for local. Track RT-015 follow-up for AWS SDK.",
            self.table, self.region, self.endpoint
        ))
    }

    pub fn put_bytes(&self, _kind: &str, _id: &str, _bytes: &[u8]) -> Result<(), StorageError> {
        Err(self.not_impl("put"))
    }

    pub fn get_bytes(&self, _kind: &str, _id: &str) -> Result<Vec<u8>, StorageError> {
        Err(self.not_impl("get"))
    }

    pub fn delete(&self, _kind: &str, _id: &str) -> Result<(), StorageError> {
        Err(self.not_impl("delete"))
    }

    pub fn list_ids(&self, _kind: &str) -> Result<Vec<String>, StorageError> {
        Err(self.not_impl("list"))
    }
}
