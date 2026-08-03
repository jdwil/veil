//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

/// Adapter: BusAuthAdapter (implements AuthService)
pub struct BusAuthAdapter {}

#[async_trait]
impl AuthService for BusAuthAdapter {
    async fn check_permission(
        &self,
        principal: Principal,
        permission: String,
    ) -> Result<bool, DomainError> {
        return Ok(true);
    }

    async fn validate_token(&self, token: String) -> Result<Principal, DomainError> {
        return Ok(Principal {
            id: token.clone(),
            roles: vec![],
            claims: HashMap::new(),
        });
    }
}
