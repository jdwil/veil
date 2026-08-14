//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: ExtensionRegistry
#[async_trait]
pub trait ExtensionRegistry: Send + Sync {
    async fn create(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError>;
    async fn get(&self, id: Uuid) -> Result<Option<ExtensionRecord>, DomainError>;
    async fn list(
        &self,
        scope: Option<String>,
        kind: Option<String>,
        product_id: Option<String>,
        tenant_id: Option<Uuid>,
    ) -> Result<Vec<ExtensionRecord>, DomainError>;
    async fn update(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError>;
    async fn save_version(&self, ver: ExtensionVersion) -> Result<ExtensionVersion, DomainError>;
    async fn get_version(
        &self,
        id: Uuid,
        version: i64,
    ) -> Result<Option<ExtensionVersion>, DomainError>;
    async fn list_versions(&self, id: Uuid) -> Result<Vec<ExtensionVersion>, DomainError>;
}

/// Port: ExtensionSourceStore
#[async_trait]
pub trait ExtensionSourceStore: Send + Sync {
    async fn ensure_package(&self, id: Uuid) -> Result<String, DomainError>;
    async fn write_file(
        &self,
        id: Uuid,
        rel_path: String,
        content: String,
    ) -> Result<(), DomainError>;
    async fn read_file(&self, id: Uuid, rel_path: String) -> Result<Option<String>, DomainError>;
    async fn list_files(&self, id: Uuid, prefix: String) -> Result<Vec<String>, DomainError>;
    async fn package_root(&self, id: Uuid) -> Result<String, DomainError>;
}

/// Port: ExtensionArtifactStore
#[async_trait]
pub trait ExtensionArtifactStore: Send + Sync {
    async fn put(
        &self,
        id: Uuid,
        version: i64,
        target: String,
        data: Vec<u8>,
    ) -> Result<String, DomainError>;
    async fn get_uri(
        &self,
        id: Uuid,
        version: i64,
        target: String,
    ) -> Result<Option<String>, DomainError>;
}

/// Port: ExtensionExecutor
#[async_trait]
pub trait ExtensionExecutor: Send + Sync {
    async fn publish(&self, id: Uuid) -> Result<ExtensionVersion, DomainError>;
    async fn invoke(
        &self,
        req: ExtensionInvokeRequest,
    ) -> Result<ExtensionInvokeResult, DomainError>;
}
