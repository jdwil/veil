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

/// Adapter: SmsTwilio (implements Notifier)
pub struct SmsTwilio {
    pub twilio_sid: String,
    pub twilio_token: String,
}

#[async_trait]
impl Notifier for SmsTwilio {
    async fn send_sms(&self, phone: Phone, msg: String) -> Result<(), DomainError> {
        http_post(
            "api.twilio.com/Messages",
            serde_json::json!({ "To": serde_json::json!(phone.clone())["number"].clone(), "Body": msg.clone() }),
        );
        Ok(())
    }

    async fn send_email(
        &self,
        email: Email,
        subj: String,
        body: String,
    ) -> Result<(), DomainError> {
        Ok(()) // TODO: implement
    }
}
