//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Stub types — replace with actual definitions
pub type ExtId = String;
pub type Plan = String;

/// Aggregate: Subscription
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub plan: Plan,
    pub status: SubStatus,
}

/// States for Subscription (state block)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SubStatus {
    Trial,
    Active,
    Cancelled,
}

impl Subscription {
    pub fn new(customer_id: Uuid, plan: Plan) -> Self {
        Self {
            id: Uuid::new_v4(),
            customer_id,
            plan,
            status: SubStatus::Trial,
        }
    }
}

impl Subscription {
    pub fn activate(&mut self) -> Result<Vec<SubscriptionCommand>, DomainError> {
        if !(self.status == SubStatus::Trial) {
            return Err(DomainError::Validation("invariant violated".into()));
        }
        let mut events: Vec<SubscriptionCommand> = Vec::new();
        self.status = SubStatus::Active;
        Ok(events)
    }

    pub fn cancel(&mut self) -> Result<Vec<SubscriptionCommand>, DomainError> {
        if !(self.status == SubStatus::Active) {
            return Err(DomainError::Validation("invariant violated".into()));
        }
        let mut events: Vec<SubscriptionCommand> = Vec::new();
        self.status = SubStatus::Cancelled;
        Ok(events)
    }
}
