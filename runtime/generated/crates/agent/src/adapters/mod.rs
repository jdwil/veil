//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

// External-effect runtime hooks (stubs). Replace with real
// integrations; generated so adapter bodies compile.
fn sessions_contains_key(_arg0: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn sessions_get(_arg0: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn sessions_insert(_arg0: impl std::fmt::Debug, _arg1: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn sessions_remove(_arg0: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn sessions_values() { /* stub — replace with real integration */
}

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

/// Adapter: InMemoryAcpRegistry (implements AcpSessionRegistry)
pub struct InMemoryAcpRegistry {
    pub sessions: tokio::sync::RwLock<std::collections::HashMap<String, AcpSession>>,
}

#[async_trait]
impl AcpSessionRegistry for InMemoryAcpRegistry {
    async fn get_status(&self, user_id: String) -> Option<AcpConnectionStatus> {
        let session = self.sessions.read().await.get(&user_id).cloned();
        if session.is_none() {
            return None;
        };
        let s = session.clone()?;
        return Some(AcpConnectionStatus {
            agent_name: s.agent_name.clone(),
            connected: true,
        });
    }

    async fn is_connected(&self, user_id: String) -> bool {
        return self.sessions.read().await.contains_key(&user_id);
    }

    async fn list_sessions(&self) -> Vec<AcpConnectionStatus> {
        return self
            .sessions
            .read()
            .await
            .values()
            .map(|s| AcpConnectionStatus {
                agent_name: s.agent_name.clone(),
                connected: true,
            })
            .collect();
    }

    async fn register(&self, user_id: String, session: AcpSession) -> Result<(), DomainError> {
        self.sessions
            .write()
            .await
            .insert(user_id.clone(), session.clone());
        Ok(())
    }

    async fn send_tool_result(
        &self,
        user_id: String,
        result: AcpToolResult,
    ) -> Result<(), DomainError> {
        todo!("empty adapter body: InMemoryAcpRegistry::send_tool_result")
    }

    async fn send_turn_request(
        &self,
        user_id: String,
        request: AcpTurnRequest,
    ) -> Result<(), DomainError> {
        todo!("empty adapter body: InMemoryAcpRegistry::send_turn_request")
    }

    async fn unregister(&self, user_id: String) -> Result<(), DomainError> {
        self.sessions.write().await.remove(&user_id);
        Ok(())
    }
}
