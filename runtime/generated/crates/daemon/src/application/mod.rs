//! Application services and flow functions.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::messages::*;
use crate::domain::types::*;
use crate::ports::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Injected dependencies (ports).
pub struct Deps {
    pub bus: std::sync::Arc<dyn Bus + Send + Sync>,
}

/// DomainService: LoadConfig
#[tracing::instrument(skip_all)]
pub async fn load_config() -> Result<DaemonConfig, DomainError> {
    // step: execute
    let work_dir = std::env::var("VEIL_WORK_DIR".to_string())
        .unwrap_or_else(|_| "/tmp/veil-runtime".to_string());
    let s3_bucket = std::env::var("VEIL_S3_BUCKET".to_string()).ok();
    let ddb_table = std::env::var("VEIL_DDB_TABLE".to_string()).ok();
    let aws_region =
        std::env::var("AWS_REGION".to_string()).unwrap_or_else(|_| "us-east-1".to_string());
    let ecr_prefix = std::env::var("VEIL_ECR_PREFIX".to_string()).ok();
    let llm_api_key = std::env::var("OPENAI_API_KEY".to_string()).ok();
    let llm_model =
        std::env::var("VEIL_LLM_MODEL".to_string()).unwrap_or_else(|_| "gpt-4o".to_string());
    return Ok(DaemonConfig {
        port: 8080,
        work_dir: work_dir.clone(),
        s3_bucket: s3_bucket.clone(),
        ddb_table: ddb_table.clone(),
        aws_region: aws_region.clone(),
        ecr_repository_prefix: ecr_prefix.clone(),
        llm_api_key: llm_api_key.clone(),
        llm_model: llm_model.clone(),
    });
}

/// WsHandler: HandleConnection
/// @path
#[tracing::instrument(skip_all)]
pub async fn handle_connection(deps: &Deps, msg: String) -> Result<(), DomainError> {
    // step: handle
    let response = serde_json::from_value::<OutgoingMessage>(deps.bus.invoke(serde_json::json!({ "type": "HandleToolCall", "tool": "echo".to_string(), "args": serde_json::json!({}) })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    /* send_ws response */

    Ok(())
}

/// DomainService: HandleAgentMessage
#[tracing::instrument(skip_all)]
pub async fn handle_agent_message(message: String) -> Result<OutgoingMessage, DomainError> {
    // step: execute
    let response = OutgoingMessage::AgentResponse {
        message: format!(
            "Agent mode received: '{}'. Configure OPENAI_API_KEY to enable.",
            message
        ),
    };
    return Ok(response);
}

/// DomainService: HandleToolCall
#[tracing::instrument(skip_all)]
pub async fn handle_tool_call(
    deps: &Deps,
    tool: String,
    args: serde_json::Value,
) -> Result<OutgoingMessage, DomainError> {
    // step: execute
    let result = deps
        .bus
        .request(
            serde_json::json!({ "type": "HandleTool", "tool": tool.clone(), "args": args.clone() }),
        )
        .await?;
    return Ok(OutgoingMessage::Result {
        data: result.clone(),
    });
}

/// HttpRoute: HealthCheck
/// @method
/// @path
#[tracing::instrument(skip_all)]
pub async fn health_check() -> Result<serde_json::Value, DomainError> {
    // step: handle
    return Ok(
        serde_json::json!({ "status": "healthy".to_string(), "service": "veil-runtime".to_string() }),
    );
}
