//! Shared types across all context crates — common errors and
//! layer-provided infrastructure traits (the message Bus).

#![allow(unused_imports)]

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

/// Trait: Bus
#[async_trait]
pub trait Bus: Send + Sync {
    async fn dispatch(&self, evt: serde_json::Value) -> Result<(), DomainError>;
    async fn invoke(&self, cmd: serde_json::Value) -> Result<serde_json::Value, DomainError>;
    async fn request(&self, qry: serde_json::Value) -> Result<serde_json::Value, DomainError>;
}

/// Trait: AuthService
#[async_trait]
pub trait AuthService: Send + Sync {
    async fn validate_token(&self, token: String) -> Result<Principal, DomainError>;
    async fn check_permission(
        &self,
        principal: Principal,
        permission: String,
    ) -> Result<bool, DomainError>;
}

/// Trait: SagaStep
#[async_trait]
pub trait SagaStep: Send + Sync {
    async fn action(
        &self,
        bus: &(dyn Bus + Send + Sync),
        state: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError>;
    async fn compensate(
        &self,
        bus: &(dyn Bus + Send + Sync),
        state: serde_json::Value,
    ) -> Result<(), DomainError>;
}

/// Layer-declared coordinator.
pub async fn unwind(
    bus: &(dyn Bus + Send + Sync),
    steps: &[Box<dyn SagaStep + Send + Sync>],
    upto: i64,
    state: serde_json::Value,
) -> Result<(), DomainError> {
    let mut i = upto;
    while i > 0 {
        i = i - 1;
        steps[(i) as usize]
            .compensate(bus.clone(), state.clone())
            .await?;
    }
    return Ok(());
}

/// Layer-declared coordinator.
pub async fn run_saga(
    bus: &(dyn Bus + Send + Sync),
    steps: &[Box<dyn SagaStep + Send + Sync>],
) -> Result<(), DomainError> {
    let mut state = serde_json::json!({});
    let mut i = 0;
    while i < (steps.len() as i64) {
        match steps[(i) as usize].action(bus.clone(), state.clone()).await {
            Ok(next) => {
                state = next;
                i = i + 1;
            }
            Err(e) => {
                unwind(bus.clone(), steps.clone(), i.clone(), state.clone());
                return Err(e);
            }
        };
    }
    return Ok(());
}
