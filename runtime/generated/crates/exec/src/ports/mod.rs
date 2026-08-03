//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: Handler
#[async_trait]
pub trait Handler: Send + Sync {
    async fn handle(&self, payload: String) -> Result<String, DomainError>;
}

/// Port: HandlerFactory
#[async_trait]
pub trait HandlerFactory: Send + Sync {
    async fn create_handler(
        &self,
        message_name: String,
        env: EnvConfig,
    ) -> Result<Option<serde_json::Value>, DomainError>;
    async fn context_name(&self) -> Result<String, DomainError>;
}
