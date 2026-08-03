//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: IncomingMessage
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IncomingMessage {
    Agent {
        message: String,
    },
    Tool {
        tool: String,
        args: serde_json::Value,
    },
}

/// enum: OutgoingMessage
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutgoingMessage {
    Result { data: serde_json::Value },
    AgentResponse { message: String },
    Error { message: String, code: String },
    Ack { id: Option<String> },
}

/// ValueObject: DaemonConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub port: i64,
    pub work_dir: String,
    pub s3_bucket: Option<String>,
    pub ddb_table: Option<String>,
    pub aws_region: String,
    pub ecr_repository_prefix: Option<String>,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
}

impl DaemonConfig {
    pub fn new(work_dir: String, aws_region: String, llm_model: String) -> Self {
        Self {
            port: 0,
            work_dir,
            s3_bucket: None,
            ddb_table: None,
            aws_region,
            ecr_repository_prefix: None,
            llm_api_key: None,
            llm_model,
        }
    }
}
