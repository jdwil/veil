//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: AcpSessionRegistry
#[async_trait]
pub trait AcpSessionRegistry: Send + Sync {
    async fn is_connected(&self, user_id: String) -> bool;
    async fn register(&self, user_id: String, session: AcpSession) -> Result<(), DomainError>;
    async fn unregister(&self, user_id: String) -> Result<(), DomainError>;
    async fn get_status(&self, user_id: String) -> Option<AcpConnectionStatus>;
    async fn list_sessions(&self) -> Vec<AcpConnectionStatus>;
    async fn send_turn_request(
        &self,
        user_id: String,
        request: AcpTurnRequest,
    ) -> Result<(), DomainError>;
    async fn send_tool_result(
        &self,
        user_id: String,
        result: AcpToolResult,
    ) -> Result<(), DomainError>;
}

/// Port: AgentMetricsStore
#[async_trait]
pub trait AgentMetricsStore: Send + Sync {
    async fn record_change(&self, metrics: AgentLoopMetrics) -> Result<(), DomainError>;
    async fn get_recent(&self, limit: i64) -> Result<Vec<AgentLoopMetrics>, DomainError>;
    async fn get_summary(&self) -> Result<serde_json::Value, DomainError>;
}
