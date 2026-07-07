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
}

/// Saga: Onboard
/// Step `create_customer` of `Onboard` (impl SagaStep).
struct OnboardStep0 {
    email: Email,
    phone: Phone,
    name: String,
}

#[async_trait::async_trait]
impl SagaStep for OnboardStep0 {
    async fn action(
        &self,
        bus: &(dyn Bus + Send + Sync),
        mut state: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError> {
        state["c"] = serde_json::json!(bus.invoke(serde_json::json!({ "target": "Customer", "method": "new", "args": [self.email.clone(), self.phone.clone()] })).await?);
        bus.invoke(serde_json::json!({ "target": "CustomerRepo", "method": "save", "args": [state["c"].clone()] })).await?;
        bus.dispatch(serde_json::json!({ "type": "CustomerCreated", "id": state["c"]["id"].clone(), "email": self.email.clone(), "created": state["c"]["created"].clone() })).await?;
        Ok(state)
    }
    async fn compensate(
        &self,
        bus: &(dyn Bus + Send + Sync),
        mut state: serde_json::Value,
    ) -> Result<(), DomainError> {
        bus.invoke(serde_json::json!({ "target": "CustomerRepo", "method": "delete", "args": [state["c"]["id"].clone()] })).await?;
        Ok(())
    }
}

/// Step `verify_identity` of `Onboard` (impl SagaStep).
struct OnboardStep1 {
    email: Email,
    phone: Phone,
    name: String,
}

#[async_trait::async_trait]
impl SagaStep for OnboardStep1 {
    async fn action(
        &self,
        bus: &(dyn Bus + Send + Sync),
        mut state: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError> {
        state["result"] = serde_json::json!(bus.invoke(serde_json::json!({ "target": "KYCProvider", "method": "check", "args": [self.name.clone(), self.email.clone()] })).await?);
        bus.invoke(serde_json::json!({ "target": "c", "method": "verify", "args": [state["result"]["code"].clone()] })).await?;
        Ok(state)
    }
    async fn compensate(
        &self,
        bus: &(dyn Bus + Send + Sync),
        mut state: serde_json::Value,
    ) -> Result<(), DomainError> {
        bus.invoke(
            serde_json::json!({ "target": "c", "method": "reject", "args": ["saga_rollback"] }),
        )
        .await?;
        Ok(())
    }
}

/// Step `setup_billing` of `Onboard` (impl SagaStep).
struct OnboardStep2 {
    email: Email,
    phone: Phone,
    name: String,
}

#[async_trait::async_trait]
impl SagaStep for OnboardStep2 {
    async fn action(
        &self,
        bus: &(dyn Bus + Send + Sync),
        mut state: serde_json::Value,
    ) -> Result<serde_json::Value, DomainError> {
        bus.invoke(serde_json::json!({ "target": "PaymentGateway", "method": "create_customer", "args": [serde_json::json!(self.email.clone())["addr"].clone()] })).await?;
        bus.invoke(serde_json::json!({ "type": "Billing", "customer_id": state["c"]["id"].clone(), "plan": "FreeTier" })).await?;
        Ok(state)
    }
    async fn compensate(
        &self,
        bus: &(dyn Bus + Send + Sync),
        mut state: serde_json::Value,
    ) -> Result<(), DomainError> {
        bus.invoke(
            serde_json::json!({ "type": "Billing", "customer_id": state["c"]["id"].clone() }),
        )
        .await?;
        Ok(())
    }
}

#[tracing::instrument(skip_all)]
pub async fn onboard(
    deps: &Deps,
    email: Email,
    phone: Phone,
    name: String,
) -> Result<(), DomainError> {
    let steps: Vec<Box<dyn SagaStep + Send + Sync>> = vec![
        Box::new(OnboardStep0 {
            email: email.clone(),
            phone: phone.clone(),
            name: name.clone(),
        }),
        Box::new(OnboardStep1 {
            email: email.clone(),
            phone: phone.clone(),
            name: name.clone(),
        }),
        Box::new(OnboardStep2 {
            email: email.clone(),
            phone: phone.clone(),
            name: name.clone(),
        }),
    ];
    run_saga(deps.bus.as_ref(), &steps).await
}
