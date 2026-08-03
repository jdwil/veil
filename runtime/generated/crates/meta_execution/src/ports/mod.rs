//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: ObjectStorage
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    async fn put(&self, key: String, data: Vec<u8>) -> Result<(), DomainError>;
    async fn get(&self, key: String) -> Result<Vec<u8>, DomainError>;
    async fn delete(&self, key: String) -> Result<(), DomainError>;
    async fn exists(&self, key: String) -> Result<bool, DomainError>;
    async fn list(&self, prefix: String) -> Result<Vec<String>, DomainError>;
    async fn size(&self, key: String) -> Result<i64, DomainError>;
}

/// Port: MetaLayerExecutor
#[async_trait]
pub trait MetaLayerExecutor: Send + Sync {
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult, DomainError>;
    async fn warm_check(&self, function_id: MetaFunctionId) -> Result<bool, DomainError>;
    async fn warm(&self, function_id: MetaFunctionId) -> Result<(), DomainError>;
}

/// Port: MetaCompilationBackend
#[async_trait]
pub trait MetaCompilationBackend: Send + Sync {
    async fn compile(
        &self,
        function_id: MetaFunctionId,
        content_hash: String,
        source_data: Vec<u8>,
    ) -> Result<Vec<u8>, DomainError>;
}

/// Port: MetaArtifactCache
#[async_trait]
pub trait MetaArtifactCache: Send + Sync {
    async fn get(&self, content_hash: String) -> Result<Option<String>, DomainError>;
    async fn put(&self, content_hash: String, binary_data: Vec<u8>) -> Result<String, DomainError>;
    async fn evict(&self, content_hash: String) -> Result<(), DomainError>;
    async fn stats(&self) -> Result<CacheStats, DomainError>;
}

/// Port: SubprocessRunner
#[async_trait]
pub trait SubprocessRunner: Send + Sync {
    async fn run(
        &self,
        binary_path: String,
        input_json: String,
        timeout_ms: i64,
        memory_limit_mb: i64,
    ) -> Result<SubprocessOutput, DomainError>;
}
