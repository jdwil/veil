//! Shared types across all context crates — common errors and
//! layer-provided infrastructure traits (routing ports, etc.).

#![allow(unused_imports)]

pub mod register_handlers;
pub use register_handlers::{HANDLER_NAMES, handler_count, register_all};

use async_trait::async_trait;
use uuid::Uuid;

/// Domain error type.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("Not found")]
    NotFound,
    #[error("Validation failed: {0}")]
    Validation(String),
    #[error("External service error: {0}")]
    External(String),
}

/// Validation error type.
#[derive(Debug, thiserror::Error)]
#[error("Validation error: {0}")]
pub struct ValidationError(pub String);

impl From<ValidationError> for DomainError {
    fn from(e: ValidationError) -> Self {
        DomainError::Validation(e.0)
    }
}

impl From<serde_json::Error> for DomainError {
    fn from(e: serde_json::Error) -> Self {
        DomainError::External(e.to_string())
    }
}

impl From<String> for DomainError {
    fn from(e: String) -> Self {
        DomainError::External(e)
    }
}

/// Trait: ApiClient
#[async_trait]
pub trait ApiClient: Send + Sync {
    async fn fetch(
        &self,
        endpoint: String,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError>;
    async fn mutate(
        &self,
        endpoint: String,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError>;
    async fn put(
        &self,
        endpoint: String,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError>;
    async fn delete(&self, endpoint: String) -> Result<serde_json::Value, DomainError>;
}

/// Layer-provided struct: LocalStorage
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LocalStorage {}

/// Layer-provided struct: FormState
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FormState {
    pub data: serde_json::Value,
    pub errors: std::collections::HashMap<String, String>,
    pub submitting: bool,
    pub valid: bool,
}

/// Layer-declared coordinator.
pub async fn goto(url: String) -> Result<(), DomainError> {
    url;
    return Ok(());
}

/// Layer-declared coordinator.
pub async fn location_pathname() -> String {
    return "".to_string();
}

/// Layer-declared coordinator.
pub async fn location_last_segment() -> String {
    return "".to_string();
}

/// Layer-declared coordinator.
pub async fn invalidate_all() -> Result<(), DomainError> {
    "all".to_string();
    return Ok(());
}
