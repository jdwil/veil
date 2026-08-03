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

/// Tool: CreateRepoTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn create_repo_tool(
    deps: &Deps,
    name: String,
    description: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let result = deps.bus.request(serde_json::json!({ "type": "CreateRepo", "name": name.clone(), "description": description.clone() })).await?;
    return Ok(result);
}

/// Tool: WriteFileTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn write_file_tool(
    deps: &Deps,
    repo_id: String,
    branch: String,
    path: String,
    content: String,
    message: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let result = deps.bus.invoke(serde_json::json!({ "type": "WriteFile", "repo_id": id.clone(), "branch": branch.clone(), "path": path.clone(), "content": content.clone(), "message": message.clone() })).await?;
    return Ok(result);
}

/// Tool: ReadFileTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn read_file_tool(
    deps: &Deps,
    repo_id: String,
    branch: String,
    path: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let result = deps.bus.invoke(serde_json::json!({ "type": "ReadFile", "repo_id": id.clone(), "branch": branch.clone(), "path": path.clone() })).await?;
    return Ok(serde_json::json!({ "content": result.clone(), "path": path.clone() }));
}

/// Tool: ListFilesTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn list_files_tool(
    deps: &Deps,
    repo_id: String,
    branch: String,
    prefix: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let p = prefix.clone().unwrap_or("".to_string());
    let files = serde_json::from_value::<Vec<String>>(deps.bus.invoke(serde_json::json!({ "type": "ListFiles", "repo_id": id.clone(), "branch": branch.clone(), "prefix": p.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    return Ok(serde_json::json!({ "files": files.clone() }));
}

/// Tool: CreateBranchTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn create_branch_tool(
    deps: &Deps,
    repo_id: String,
    name: String,
    from_ref: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let branch = deps.bus.invoke(serde_json::json!({ "type": "CreateBranch", "repo_id": id.clone(), "name": name.clone(), "from_ref": from_ref.clone() })).await?;
    return Ok(branch);
}

/// Tool: ListBranchesTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn list_branches_tool(
    deps: &Deps,
    repo_id: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let branches = deps
        .bus
        .invoke(serde_json::json!({ "type": "ListBranches", "repo_id": id.clone() }))
        .await?;
    return Ok(branches);
}

/// Tool: DiffTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn diff_tool(
    deps: &Deps,
    repo_id: String,
    from_ref: String,
    to_ref: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let diff = deps.bus.invoke(serde_json::json!({ "type": "GetDiff", "repo_id": id.clone(), "from_ref": from_ref.clone(), "to_ref": to_ref.clone() })).await?;
    return Ok(diff);
}

/// Tool: CompileTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn compile_tool(
    deps: &Deps,
    repo_id: String,
    branch: String,
    target: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let result = deps.bus.invoke(serde_json::json!({ "type": "Compile", "repo_id": id.clone(), "branch": branch.clone(), "target": target.clone() })).await?;
    return Ok(result);
}

/// Tool: DeployTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn deploy_tool(
    deps: &Deps,
    artifact_id: String,
    target: String,
    tag: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let aid = artifact_id;
    let result = deps.bus.invoke(serde_json::json!({ "type": "Deploy", "artifact_id": aid.clone(), "target": target.clone(), "tag": tag.clone() })).await?;
    return Ok(result);
}

/// Tool: ListReposTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn list_repos_tool(deps: &Deps) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let repos = deps
        .bus
        .invoke(serde_json::json!({ "type": "ListRepos" }))
        .await?;
    return Ok(repos);
}

/// Tool: LogTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn log_tool(
    deps: &Deps,
    repo_id: String,
    branch: Option<String>,
    limit: Option<i64>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let id = repo_id;
    let l = limit.clone().unwrap_or(20);
    let commits = deps.bus.invoke(serde_json::json!({ "type": "GetCommitLog", "repo_id": id.clone(), "branch": branch.clone(), "limit": l.clone(), "offset": 0 })).await?;
    return Ok(commits);
}

/// Tool: ValidateReactionPaletteTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn validate_reaction_palette_tool(
    deps: &Deps,
    node_kinds: Vec<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let ok = serde_json::from_value::<bool>(deps.bus.invoke(serde_json::json!({ "type": "ValidateReactionPalette", "node_kinds": node_kinds.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    if ok {
        return Ok(serde_json::json!({ "ok": true, "message": "palette ok".to_string() }));
    };
    return Ok(
        serde_json::json!({ "ok": false, "message": "rejected: one or more node kinds are outside the reaction palette (allowed: Guard, Activate, Map, EmitEvent, End)".to_string() }),
    );
}

/// Tool: ProposeReactionGraphTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn propose_reaction_graph_tool(
    deps: &Deps,
    node_kinds: Vec<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let ok = serde_json::from_value::<bool>(deps.bus.invoke(serde_json::json!({ "type": "ValidateReactionPalette", "node_kinds": node_kinds.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    if ok == false {
        return Ok(
            serde_json::json!({ "ok": false, "accepted": false, "message": "rejected: graph not presentable with configured palette".to_string(), "node_kinds": node_kinds.clone() }),
        );
    };
    return Ok(
        serde_json::json!({ "ok": true, "accepted": true, "message": "accepted".to_string(), "node_kinds": node_kinds.clone() }),
    );
}
