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
    pub cache: std::sync::Arc<dyn MetaArtifactCache + Send + Sync>,
    pub compiler: std::sync::Arc<dyn MetaCompilationBackend + Send + Sync>,
    pub objects: std::sync::Arc<dyn ObjectStorage + Send + Sync>,
}

/// DomainService: ResolveContentHash
/// @dep
#[tracing::instrument(skip_all)]
pub async fn resolve_content_hash(
    deps: &Deps,
    function_id: MetaFunctionId,
) -> Result<String, DomainError> {
    // step: resolve
    match function_id.version {
        MetaFunctionVersion::Pinned { hash } => return Ok(hash),
        MetaFunctionVersion::Latest => {
            let key = format!(
                "functions/{}/{}/latest.hash",
                function_id.tenant_id, function_id.function_path
            );
            let hash_bytes = deps.objects.get(key.clone()).await?;
            return Ok(String::from_utf8_lossy(&hash_bytes)
                .to_string()
                .trim()
                .to_string());
        }
        _ => unreachable!(),
    };
}

/// DomainService: EnsureCompiled
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn ensure_compiled(
    deps: &Deps,
    function_id: MetaFunctionId,
    content_hash: String,
) -> Result<String, DomainError> {
    // step: check_cache
    let cached = deps.cache.get(content_hash.clone()).await?;
    if cached.is_some() {
        return Ok(cached.clone().ok_or(DomainError::NotFound)?);
    };

    // step: fetch_source
    let source_key = format!(
        "functions/{}/{}/source/{}",
        function_id.tenant_id, function_id.function_path, content_hash
    );
    let source_data = deps.objects.get(source_key.clone()).await?;

    // step: compile
    let binary = deps
        .compiler
        .compile(
            function_id.clone(),
            content_hash.clone(),
            source_data.clone(),
        )
        .await?;
    let path = deps.cache.put(content_hash.clone(), binary.clone()).await?;
    return Ok(path);
}

/// DomainService: ExecuteMetaFunction
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn execute_meta_function(
    deps: &Deps,
    function_id: MetaFunctionId,
    input: serde_json::Value,
    capabilities: Vec<GrantedCapability>,
    timeout_ms: Option<i64>,
    idempotency_key: Option<String>,
) -> Result<ExecutionResult, DomainError> {
    // step: resolve
    let content_hash = serde_json::from_value::<String>(deps.bus.invoke(serde_json::json!({ "type": "ResolveContentHash", "function_id": function_id.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;

    // step: ensure_compiled
    let binary_path = serde_json::from_value::<String>(deps.bus.invoke(serde_json::json!({ "type": "EnsureCompiled", "function_id": function_id.clone(), "content_hash": content_hash.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;

    // step: execute
    let subprocess_input = SubprocessInput {
        input: input.clone(),
        capabilities: ResolvedCapabilities {
            services: HashMap::new(),
            storage: HashMap::new(),
            bus_emit: vec![],
        },
    };
    let timeout = timeout_ms.clone().unwrap_or(30000);
    let output = SubprocessOutput {
        success: true,
        output: Some(input.clone()),
        error: None,
        emitted_events: vec![],
    };
    return Ok(ExecutionResult {
        output: serde_json::json!(output.output.unwrap_or(serde_json::Value::Null)),
        metadata: ExecutionMetadata {
            duration_ms: 0,
            cached: false,
            compiled: true,
            artifact_hash: content_hash.clone(),
        },
    });
}

/// DomainService: WarmFunction
/// @dep
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn warm_function(deps: &Deps, function_id: MetaFunctionId) -> Result<(), DomainError> {
    // step: warm
    let content_hash = serde_json::from_value::<String>(deps.bus.invoke(serde_json::json!({ "type": "ResolveContentHash", "function_id": function_id.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    serde_json::from_value::<String>(deps.bus.invoke(serde_json::json!({ "type": "EnsureCompiled", "function_id": function_id.clone(), "content_hash": content_hash.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;

    Ok(())
}

/// DomainService: CheckWarmStatus
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn check_warm_status(
    deps: &Deps,
    function_id: MetaFunctionId,
) -> Result<bool, DomainError> {
    // step: check
    let content_hash = serde_json::from_value::<String>(deps.bus.invoke(serde_json::json!({ "type": "ResolveContentHash", "function_id": function_id.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    let cached = deps.cache.get(content_hash.clone()).await?;
    return Ok(cached.is_some());
}

/// Tool: ExecuteMetaLayerTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn execute_meta_layer_tool(
    deps: &Deps,
    tenant_id: String,
    function_path: String,
    version: Option<String>,
    input: serde_json::Value,
    timeout_ms: Option<i64>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let v_str = version.clone().unwrap_or("latest".to_string());
    let mut fn_version = MetaFunctionVersion::Latest;
    if v_str != "latest".to_string() {
        fn_version = MetaFunctionVersion::Pinned {
            hash: v_str.clone(),
        };
    };
    let fn_id = MetaFunctionId {
        tenant_id: tenant_id.clone(),
        function_path: function_path.clone(),
        version: fn_version.clone(),
    };
    let result = serde_json::from_value::<ExecutionResult>(deps.bus.invoke(serde_json::json!({ "type": "ExecuteMetaFunction", "function_id": fn_id.clone(), "input": input.clone(), "capabilities": serde_json::Value::Array(vec![]), "timeout_ms": timeout_ms.clone(), "idempotency_key": serde_json::Value::Null })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    return Ok(
        serde_json::json!({ "output": serde_json::json!(result.clone())["output"].clone(), "metadata": serde_json::json!(result.clone())["metadata"].clone(), "summary": format!("Executed {} for tenant {} in {}ms. Artifact: {}", function_path, tenant_id, result.metadata.duration_ms, result.metadata.artifact_hash) }),
    );
}

/// Tool: WarmMetaLayerTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn warm_meta_layer_tool(
    deps: &Deps,
    tenant_id: String,
    function_path: String,
    version: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let v_str = version.clone().unwrap_or("latest".to_string());
    let mut fn_version = MetaFunctionVersion::Latest;
    if v_str != "latest".to_string() {
        fn_version = MetaFunctionVersion::Pinned {
            hash: v_str.clone(),
        };
    };
    let fn_id = MetaFunctionId {
        tenant_id: tenant_id.clone(),
        function_path: function_path.clone(),
        version: fn_version.clone(),
    };
    deps.bus
        .invoke(serde_json::json!({ "type": "WarmFunction", "function_id": fn_id.clone() }))
        .await?;
    return Ok(
        serde_json::json!({ "status": "warm".to_string(), "function": format!("{}::{}", tenant_id, function_path), "summary": format!("Function {} for tenant {} is now warm and ready for execution.", function_path, tenant_id) }),
    );
}

/// Tool: MetaLayerStatusTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn meta_layer_status_tool(
    deps: &Deps,
    tenant_id: String,
    function_path: String,
    version: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let v_str = version.clone().unwrap_or("latest".to_string());
    let mut fn_version = MetaFunctionVersion::Latest;
    if v_str != "latest".to_string() {
        fn_version = MetaFunctionVersion::Pinned {
            hash: v_str.clone(),
        };
    };
    let fn_id = MetaFunctionId {
        tenant_id: tenant_id.clone(),
        function_path: function_path.clone(),
        version: fn_version.clone(),
    };
    let warm = serde_json::from_value::<bool>(
        deps.bus
            .invoke(serde_json::json!({ "type": "CheckWarmStatus", "function_id": fn_id.clone() }))
            .await?,
    )
    .map_err(|e| DomainError::External(e.to_string()))?;
    let status_msg = if warm {
        "warm and ready".to_string()
    } else {
        "cold (will compile on first invocation)".to_string()
    };
    return Ok(
        serde_json::json!({ "warm": warm.clone(), "function": format!("{}::{}@{}", tenant_id, function_path, v_str), "summary": format!("Function {} for tenant {} is {}", function_path, tenant_id, status_msg) }),
    );
}
