//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

// External-effect runtime hooks (stubs). Replace with real
// integrations; generated so adapter bodies compile.
fn http_post(_arg0: impl std::fmt::Debug, _arg1: impl std::fmt::Debug) -> String {
    String::new()
}

/// Adapter: StripeAdapter (implements PaymentGateway)
pub struct StripeAdapter {
    pub stripe_key: String,
}

#[async_trait]
impl PaymentGateway for StripeAdapter {
    async fn create_customer(&self, email: String) -> Result<ExtId, DomainError> {
        Ok(http_post(
            "api.stripe.com/v1/customers",
            serde_json::json!({ "email": email.clone() }),
        ))
    }

    async fn cancel_subscription(&self, sub_id: Uuid) -> Result<(), DomainError> {
        Ok(()) // TODO: implement
    }
}
