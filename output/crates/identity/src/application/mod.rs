//! Application services and flow orchestrators.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::messages::*;
use crate::domain::types::*;
use crate::ports::*;
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

/// Injected dependencies (ports).
pub struct Deps {
    pub bus: std::sync::Arc<dyn Bus + Send + Sync>,
    pub customer_repo: std::sync::Arc<dyn CustomerRepo + Send + Sync>,
}

/// DomainService: CreateCustomerService
#[tracing::instrument(skip_all)]
pub async fn create_customer_service(
    deps: &Deps,
    email: Email,
    phone: Phone,
) -> Result<Uuid, DomainError> {
    // step: validate
    email
        .validate()
        .map_err(|_| DomainError::Validation("invalid email".to_string()))?;

    // step: persist
    let c = Customer::new(email.clone(), phone.clone());
    deps.customer_repo.save(c.clone()).await?;
    deps.bus.dispatch(serde_json::json!({ "type": "CustomerCreated", "id": serde_json::json!(c.clone())["id"].clone(), "email": email.clone(), "created": serde_json::json!(c.clone())["created"].clone() })).await?;

    Ok(c.id)
}
