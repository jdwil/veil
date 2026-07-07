//! Nested message types (grouped by parent construct).

#![allow(unused_imports)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::types::*;

/// Event messages for Customer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CustomerEvent {
    CustomerCreated(CustomerCreated),
    CustomerVerified(CustomerVerified),
    CustomerRejected(CustomerRejected),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerCreated {
    pub id: Uuid,
    pub email: String,
    pub created: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerVerified {
    pub id: Uuid,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerRejected {
    pub id: Uuid,
    pub reason: String,
}

/// Command messages for Customer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CustomerCommand {
    CreateCustomer(CreateCustomer),
    VerifyCustomer(VerifyCustomer),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCustomer {
    pub email: Email,
    pub phone: Phone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCustomer {
    pub id: Uuid,
    pub code: String,
}
