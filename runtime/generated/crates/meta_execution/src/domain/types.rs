//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: MetaFunctionVersion
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetaFunctionVersion {
    Latest,
    Pinned { hash: String },
}

/// enum: GrantedCapability
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GrantedCapability {
    ServiceRegistry {
        service_name: String,
        operations: Vec<String>,
    },
    StorageRead {
        namespace: String,
    },
    StorageWrite {
        namespace: String,
    },
    BusEmit {
        event_types: Vec<String>,
    },
}

/// enum: ExecutionError
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutionError {
    NotFound {
        function_path: String,
        tenant_id: String,
    },
    CompilationFailed {
        details: String,
    },
    Timeout {
        timeout_ms: i64,
    },
    RuntimePanic {
        message: String,
        exit_code: Option<i64>,
    },
    CapabilityDenied {
        capability: String,
        reason: String,
    },
    ResourceExceeded {
        resource: String,
        limit: String,
    },
    Internal {
        message: String,
    },
    Unavailable {
        message: String,
    },
}

/// enum: StorageAccessMode
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum StorageAccessMode {
    #[default]
    Read,
    Write,
    ReadWrite,
}

/// ValueObject: MetaFunctionId
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetaFunctionId {
    pub tenant_id: String,
    pub function_path: String,
    pub version: MetaFunctionVersion,
}

impl MetaFunctionId {
    pub fn new(tenant_id: String, function_path: String, version: MetaFunctionVersion) -> Self {
        Self {
            tenant_id,
            function_path,
            version,
        }
    }
}

/// ValueObject: ExecutionRequest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub function_id: MetaFunctionId,
    pub input: serde_json::Value,
    pub capabilities: Vec<GrantedCapability>,
    pub timeout_ms: Option<i64>,
    pub idempotency_key: Option<String>,
}

impl ExecutionRequest {
    pub fn new(function_id: MetaFunctionId) -> Self {
        Self {
            function_id,
            input: serde_json::json!({}),
            capabilities: Vec::new(),
            timeout_ms: None,
            idempotency_key: None,
        }
    }
}

/// ValueObject: ExecutionResult
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub output: serde_json::Value,
    pub metadata: ExecutionMetadata,
}

impl ExecutionResult {
    pub fn new(metadata: ExecutionMetadata) -> Self {
        Self {
            output: serde_json::json!({}),
            metadata,
        }
    }
}

/// ValueObject: ExecutionMetadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionMetadata {
    pub duration_ms: i64,
    pub cached: bool,
    pub compiled: bool,
    pub artifact_hash: String,
}

impl ExecutionMetadata {
    pub fn new(artifact_hash: String) -> Self {
        Self {
            duration_ms: 0,
            cached: false,
            compiled: false,
            artifact_hash,
        }
    }
}

/// ValueObject: SubprocessInput
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubprocessInput {
    pub input: serde_json::Value,
    pub capabilities: ResolvedCapabilities,
}

impl SubprocessInput {
    pub fn new(capabilities: ResolvedCapabilities) -> Self {
        Self {
            input: serde_json::json!({}),
            capabilities,
        }
    }
}

/// ValueObject: SubprocessOutput
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubprocessOutput {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub emitted_events: Vec<serde_json::Value>,
}

impl SubprocessOutput {
    pub fn new() -> Self {
        Self {
            success: false,
            output: None,
            error: None,
            emitted_events: Vec::new(),
        }
    }
}

impl Default for SubprocessOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: ResolvedCapabilities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedCapabilities {
    pub services: std::collections::HashMap<String, ResolvedService>,
    pub storage: std::collections::HashMap<String, ResolvedStorage>,
    pub bus_emit: Vec<String>,
}

impl ResolvedCapabilities {
    pub fn new() -> Self {
        Self {
            services: std::collections::HashMap::new(),
            storage: std::collections::HashMap::new(),
            bus_emit: Vec::new(),
        }
    }
}

impl Default for ResolvedCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: ResolvedService
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedService {
    pub endpoint: String,
    pub token: String,
    pub operations: Vec<String>,
}

impl ResolvedService {
    pub fn new(endpoint: String, token: String) -> Self {
        Self {
            endpoint,
            token,
            operations: Vec::new(),
        }
    }
}

/// ValueObject: ResolvedStorage
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedStorage {
    pub endpoint: String,
    pub mode: StorageAccessMode,
    pub token: String,
}

impl ResolvedStorage {
    pub fn new(endpoint: String, mode: StorageAccessMode, token: String) -> Self {
        Self {
            endpoint,
            mode,
            token,
        }
    }
}

/// ValueObject: CacheStats
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheStats {
    pub entries: i64,
    pub total_size_bytes: i64,
}

impl CacheStats {
    pub fn new() -> Self {
        Self {
            entries: 0,
            total_size_bytes: 0,
        }
    }
}

impl Default for CacheStats {
    fn default() -> Self {
        Self::new()
    }
}
