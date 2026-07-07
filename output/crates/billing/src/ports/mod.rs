//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: PaymentGateway
#[async_trait]
pub trait PaymentGateway {
    async fn create_customer(&self, email: String) -> Result<ExtId, DomainError>;
    async fn cancel_subscription(&self, sub_id: Uuid) -> Result<(), DomainError>;
}
