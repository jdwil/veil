//! Nested message types (grouped by parent construct).

#![allow(unused_imports)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::types::*;

/// Command messages for Subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscriptionCommand {
    CreateTrial(CreateTrial),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTrial {
    pub customer_id: Uuid,
    pub plan: Plan,
}
