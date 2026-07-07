//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Stub types — replace with actual definitions
pub type KYCResult = String;

/// ValueObject: Email
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Email {
    pub addr: String,
}

impl Email {
    pub fn new(addr: String) -> Result<Self, ValidationError> {
        let value = Self { addr };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// ValueObject: Phone
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phone {
    pub number: String,
    pub country: String,
}

impl Phone {
    pub fn new(number: String, country: String) -> Self {
        Self { number, country }
    }
}

/// Aggregate: Customer
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub email: Email,
    pub phone: Phone,
    pub status: CustomerStatus,
    pub created: DateTime<Utc>,
}

/// States for Customer (state block)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CustomerStatus {
    Pending,
    Verified,
    Active,
    Rejected,
}

impl Customer {
    pub fn new(email: Email, phone: Phone) -> Self {
        Self {
            id: Uuid::new_v4(),
            email,
            phone,
            status: CustomerStatus::Pending,
            created: Utc::now(),
        }
    }
}

impl Customer {
    pub fn verify(&mut self, code: String) -> Result<Vec<CustomerEvent>, DomainError> {
        if !(self.status == CustomerStatus::Pending) {
            return Err(DomainError::Validation("invariant violated".into()));
        }
        let mut events: Vec<CustomerEvent> = Vec::new();
        self.status = CustomerStatus::Verified;
        events.push(CustomerEvent::CustomerVerified(CustomerVerified {
            id: self.id,
            verified_at: Utc::now(),
        }));
        Ok(events)
    }

    pub fn reject(&mut self, reason: String) -> Result<Vec<CustomerEvent>, DomainError> {
        if !(self.status == CustomerStatus::Pending) {
            return Err(DomainError::Validation("invariant violated".into()));
        }
        let mut events: Vec<CustomerEvent> = Vec::new();
        self.status = CustomerStatus::Rejected;
        events.push(CustomerEvent::CustomerRejected(CustomerRejected {
            id: self.id,
            reason,
        }));
        Ok(events)
    }
}
