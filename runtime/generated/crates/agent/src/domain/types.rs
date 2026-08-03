//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: LlmProvider
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LlmProvider {
    Bedrock {
        model_id: String,
    },
    Byok {
        provider: String,
        model: String,
        api_key: String,
    },
    AcpTunnel {
        user_id: String,
    },
    VeilServerProxy,
}

/// enum: AgentProviderMode
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum AgentProviderMode {
    #[default]
    Bedrock,
    Byok,
    Acp,
    VeilServer,
}

/// enum: AcpAgentFrame
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpAgentFrame {
    ContentDelta {
        turn_id: String,
        delta: String,
    },
    ToolUse {
        turn_id: String,
        tool_call: AcpToolCall,
    },
    TurnComplete {
        turn_id: String,
    },
    Error {
        turn_id: Option<String>,
        message: String,
    },
}

/// enum: AcpTurnError
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AcpTurnError {
    NotConnected,
    SessionClosed,
    AgentError { message: String },
}

/// ValueObject: AcpMessage
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpMessage {
    pub role: String,
    pub content: String,
    pub tool_use: Option<Vec<AcpToolCall>>,
}

impl AcpMessage {
    pub fn new(role: String, content: String) -> Self {
        Self {
            role,
            content,
            tool_use: None,
        }
    }
}

/// ValueObject: AcpToolDef
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl AcpToolDef {
    pub fn new(name: String, description: String) -> Self {
        Self {
            name,
            description,
            parameters: serde_json::json!({}),
        }
    }
}

/// ValueObject: AcpToolCall
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl AcpToolCall {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            arguments: serde_json::json!({}),
        }
    }
}

/// ValueObject: AcpTurnRequest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpTurnRequest {
    pub turn_id: String,
    pub messages: Vec<AcpMessage>,
    pub tools: Vec<AcpToolDef>,
}

impl AcpTurnRequest {
    pub fn new(turn_id: String) -> Self {
        Self {
            turn_id,
            messages: Vec::new(),
            tools: Vec::new(),
        }
    }
}

/// ValueObject: AcpToolResult
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpToolResult {
    pub turn_id: String,
    pub call_id: String,
    pub output: serde_json::Value,
}

impl AcpToolResult {
    pub fn new(turn_id: String, call_id: String) -> Self {
        Self {
            turn_id,
            call_id,
            output: serde_json::json!({}),
        }
    }
}

/// ValueObject: AcpSession
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpSession {
    pub user_id: String,
    pub agent_name: String,
    pub connected_at: DateTime<Utc>,
}

impl AcpSession {
    pub fn new(user_id: String, agent_name: String, connected_at: DateTime<Utc>) -> Self {
        Self {
            user_id,
            agent_name,
            connected_at,
        }
    }
}

/// ValueObject: AcpConnectionStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpConnectionStatus {
    pub agent_name: String,
    pub connected: bool,
}

impl AcpConnectionStatus {
    pub fn new(agent_name: String) -> Self {
        Self {
            agent_name,
            connected: false,
        }
    }
}

/// ValueObject: AgentStatusResponse
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatusResponse {
    pub provider: String,
    pub provider_mode: serde_json::Value,
    pub acp_tunnel: serde_json::Value,
}

impl AgentStatusResponse {
    pub fn new(provider: String) -> Self {
        Self {
            provider,
            provider_mode: serde_json::json!({}),
            acp_tunnel: serde_json::json!({}),
        }
    }
}

/// ValueObject: ChatMsg
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

impl ChatMsg {
    pub fn new(role: String, content: String) -> Self {
        Self { role, content }
    }
}

/// ValueObject: ChatRequest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMsg>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub system_prompt: Option<String>,
    pub context: Option<AgentContext>,
}

impl ChatRequest {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            model: None,
            provider: None,
            system_prompt: None,
            context: None,
        }
    }
}

impl Default for ChatRequest {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: AgentContext
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentContext {
    pub page: Option<String>,
    pub project: Option<String>,
    pub surfaces: Option<Vec<serde_json::Value>>,
}

impl AgentContext {
    pub fn new() -> Self {
        Self {
            page: None,
            project: None,
            surfaces: None,
        }
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: ToolDef
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub category: String,
}

impl ToolDef {
    pub fn new(name: String, description: String, category: String) -> Self {
        Self {
            name,
            description,
            category,
        }
    }
}

/// ValueObject: ToolExecutionResult
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub output: serde_json::Value,
    pub is_error: bool,
    pub navigation: Option<serde_json::Value>,
}

impl ToolExecutionResult {
    pub fn new() -> Self {
        Self {
            output: serde_json::json!({}),
            is_error: false,
            navigation: None,
        }
    }
}

impl Default for ToolExecutionResult {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: AcpConnectParams
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpConnectParams {
    pub token: Option<String>,
    pub agent_name: Option<String>,
}

impl AcpConnectParams {
    pub fn new() -> Self {
        Self {
            token: None,
            agent_name: None,
        }
    }
}

impl Default for AcpConnectParams {
    fn default() -> Self {
        Self::new()
    }
}
