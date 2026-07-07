//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: Notifier
#[async_trait]
pub trait Notifier {
    async fn send_sms(&self, phone: Phone, msg: String) -> Result<(), DomainError>;
    async fn send_email(&self, email: Email, subj: String, body: String)
    -> Result<(), DomainError>;
}

/// Port: KYCProvider
#[async_trait]
pub trait KYCProvider {
    async fn check(&self, name: String, email: Email) -> Result<KYCResult, DomainError>;
}

/// Port: CustomerRepo
#[async_trait]
pub trait CustomerRepo {
    async fn save(&self, c: Customer) -> Result<(), DomainError>;
    async fn find(&self, id: Uuid) -> Result<Option<Customer>, DomainError>;
    async fn delete(&self, id: Uuid) -> Result<(), DomainError>;
}
