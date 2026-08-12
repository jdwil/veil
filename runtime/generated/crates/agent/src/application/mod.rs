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
    pub registry: std::sync::Arc<dyn AcpSessionRegistry + Send + Sync>,
    pub bus: std::sync::Arc<dyn Bus + Send + Sync>,
}

/// DomainService: ResolveProvider
#[tracing::instrument(skip_all)]
pub async fn resolve_provider() -> Result<LlmProvider, DomainError> {
    // step: resolve
    let provider_env = std::env::var("VEIL_AGENT_PROVIDER".to_string())
        .unwrap_or_else(|_| "veil-server".to_string());
    match provider_env.as_str() {
        "bedrock" => {
            let model_id = std::env::var("VEIL_BEDROCK_MODEL".to_string())
                .unwrap_or_else(|_| "us.anthropic.claude-sonnet-4-20250514-v1:0".to_string());
            return Ok(LlmProvider::Bedrock {
                model_id: model_id.clone(),
            });
        }
        "openai" => {
            let mut api_key =
                std::env::var("VEIL_OPENAI_KEY".to_string()).unwrap_or_else(|_| "".to_string());
            let mut model = std::env::var("VEIL_OPENAI_MODEL".to_string())
                .unwrap_or_else(|_| "gpt-4o".to_string());
            return Ok(LlmProvider::Byok {
                provider: "openai".to_string(),
                model: model.clone(),
                api_key: api_key.clone(),
            });
        }
        "anthropic" => {
            let mut api_key =
                std::env::var("VEIL_ANTHROPIC_KEY".to_string()).unwrap_or_else(|_| "".to_string());
            let mut model = std::env::var("VEIL_ANTHROPIC_MODEL".to_string())
                .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
            return Ok(LlmProvider::Byok {
                provider: "anthropic".to_string(),
                model: model.clone(),
                api_key: api_key.clone(),
            });
        }
        "acp" => {
            return Ok(LlmProvider::AcpTunnel {
                user_id: "default_user".to_string(),
            });
        }
        _ => return Ok(LlmProvider::VeilServerProxy),
    };
}

/// DomainService: GetToolRegistry
#[tracing::instrument(skip_all)]
pub async fn get_tool_registry() -> Result<Vec<ToolDef>, DomainError> {
    // step: build
    let mut tools = vec![];
    tools.push(ToolDef {
        name: "navigate_to".to_string(),
        description: "Navigate the runtime UI to a specific page".to_string(),
        category: "navigation".to_string(),
    });
    tools.push(ToolDef {
        name: "open_ide".to_string(),
        description: "Open the IDE for a specific project".to_string(),
        category: "navigation".to_string(),
    });
    tools.push(ToolDef {
        name: "switch_project".to_string(),
        description: "Switch to a different project".to_string(),
        category: "navigation".to_string(),
    });
    tools.push(ToolDef {
        name: "list_changes".to_string(),
        description: "List open change requests".to_string(),
        category: "sdlc".to_string(),
    });
    tools.push(ToolDef {
        name: "create_change".to_string(),
        description: "Create a new change request".to_string(),
        category: "sdlc".to_string(),
    });
    tools.push(ToolDef {
        name: "approve_pr".to_string(),
        description: "Approve a change request".to_string(),
        category: "sdlc".to_string(),
    });
    tools.push(ToolDef {
        name: "merge_pr".to_string(),
        description: "Merge an approved change request".to_string(),
        category: "sdlc".to_string(),
    });
    tools.push(ToolDef {
        name: "deploy_project".to_string(),
        description: "Deploy a project to an environment".to_string(),
        category: "deploy".to_string(),
    });
    tools.push(ToolDef {
        name: "deploy_status".to_string(),
        description: "Get deployment status".to_string(),
        category: "deploy".to_string(),
    });
    tools.push(ToolDef {
        name: "rollback".to_string(),
        description: "Rollback a deployment".to_string(),
        category: "deploy".to_string(),
    });
    tools.push(ToolDef {
        name: "edit_file".to_string(),
        description: "Edit a file in a project via the IDE".to_string(),
        category: "ide".to_string(),
    });
    tools.push(ToolDef {
        name: "read_file".to_string(),
        description: "Read a file from a project".to_string(),
        category: "ide".to_string(),
    });
    tools.push(ToolDef {
        name: "gen_package".to_string(),
        description: "Generate code from a veil package".to_string(),
        category: "ide".to_string(),
    });
    tools.push(ToolDef {
        name: "list_projects".to_string(),
        description: "List all projects in the runtime".to_string(),
        category: "meta".to_string(),
    });
    tools.push(ToolDef {
        name: "get_current_context".to_string(),
        description: "Get current page and project context".to_string(),
        category: "meta".to_string(),
    });
    tools.push(ToolDef {
        name: "search_registry".to_string(),
        description: "Search the layer/extension registry".to_string(),
        category: "registry".to_string(),
    });
    tools.push(ToolDef {
        name: "wiki_search".to_string(),
        description: "Search the mind-palace wiki".to_string(),
        category: "wiki".to_string(),
    });
    tools.push(ToolDef {
        name: "wiki_read".to_string(),
        description: "Read a wiki page".to_string(),
        category: "wiki".to_string(),
    });
    tools.push(ToolDef {
        name: "wiki_create".to_string(),
        description: "Create a wiki page".to_string(),
        category: "wiki".to_string(),
    });
    tools.push(ToolDef {
        name: "wiki_update".to_string(),
        description: "Update a wiki page".to_string(),
        category: "wiki".to_string(),
    });
    return Ok(tools);
}

/// DomainService: ExecuteTool
#[tracing::instrument(skip_all)]
pub async fn execute_tool(
    name: String,
    args: serde_json::Value,
    ctx: Option<AgentContext>,
) -> Result<ToolExecutionResult, DomainError> {
    // step: execute
    match name.as_str() {
        "navigate_to" => {
            let mut path = args
                .get("path")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("/dashboard".to_string());
            return Ok(ToolExecutionResult {
                output: serde_json::json!(serde_json::json!({ "navigated_to": path.clone() })),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": path.clone() })
                )),
            });
        }
        "open_ide" => {
            let mut project = args
                .get("project")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("unknown".to_string());
            return Ok(ToolExecutionResult {
                output: serde_json::json!(serde_json::json!({ "opened_ide": project.clone() })),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "open-ide".to_string(), "project": project.clone() })
                )),
            });
        }
        "switch_project" => {
            let mut project = args
                .get("project")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("unknown".to_string());
            return Ok(ToolExecutionResult {
                output: serde_json::json!(serde_json::json!({ "switched_to": project.clone() })),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "switch-project".to_string(), "project": project.clone() })
                )),
            });
        }
        "list_changes" => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": "Opening change requests".to_string(), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": "/changes".to_string() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": "/changes".to_string() })
                )),
            });
        }
        "create_change" => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": "Opening new change request form".to_string(), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": "/changes/new".to_string() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": "/changes/new".to_string() })
                )),
            });
        }
        "list_projects" => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": "Opening projects".to_string(), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": "/projects".to_string() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": "/projects".to_string() })
                )),
            });
        }
        "navigate_to" => {
            let mut path = args
                .get("path")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("/dashboard".to_string());
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": format!("Navigate to {}", path), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": path.clone() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": path.clone() })
                )),
            });
        }
        "open_project" => {
            let mut project = args
                .get("project")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or(
                    args.get("slug")
                        .cloned()
                        .ok_or(DomainError::NotFound)?
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or("".to_string()),
                );
            let mut path = if project == "".to_string() {
                "/projects".to_string()
            } else {
                format!("/projects/{}", project)
            };
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": format!("Open project {}", project), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": path.clone(), "project": project.clone() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": path.clone(), "project": project.clone() })
                )),
            });
        }
        "open_ide" => {
            let mut project = args
                .get("project")
                .cloned()
                .ok_or(DomainError::NotFound)?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or("".to_string());
            let mut path = if project == "".to_string() {
                "/projects".to_string()
            } else {
                format!("/projects/{}", project)
            };
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": format!("Open IDE for {}", project), "navigation": serde_json::json!({ "action": "open-ide".to_string(), "path": path.clone(), "project": project.clone() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "open-ide".to_string(), "path": path.clone(), "project": project.clone() })
                )),
            });
        }
        "open_deploy" => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": "Open deploy".to_string(), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": "/deploy".to_string() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": "/deploy".to_string() })
                )),
            });
        }
        "open_registry" => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": "Open registry".to_string(), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": "/registry".to_string() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": "/registry".to_string() })
                )),
            });
        }
        "open_dashboard" => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "summary": "Open dashboard".to_string(), "navigation": serde_json::json!({ "action": "goto".to_string(), "path": "/dashboard".to_string() }) })
                ),
                is_error: false,
                navigation: Some(serde_json::json!(
                    serde_json::json!({ "action": "goto".to_string(), "path": "/dashboard".to_string() })
                )),
            });
        }
        "get_current_context" => {
            let page = ctx.clone().and_then(|c| c.page).unwrap_or("/".to_string());
            let mut project = ctx.clone().and_then(|c| c.project);
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "page": page.clone(), "project": project.clone() })
                ),
                is_error: false,
                navigation: None,
            });
        }
        _ => {
            return Ok(ToolExecutionResult {
                output: serde_json::json!(
                    serde_json::json!({ "error": format!("unknown tool: {}", name) })
                ),
                is_error: true,
                navigation: None,
            });
        }
    };
}

/// DomainService: ExecuteAcpTurn
/// @dep
#[tracing::instrument(skip_all)]
pub async fn execute_acp_turn(
    deps: &Deps,
    user_id: String,
    turn_id: String,
    messages: Vec<AcpMessage>,
    tools: Vec<AcpToolDef>,
) -> Result<String, DomainError> {
    // step: execute
    let connected = deps.registry.is_connected(user_id.clone()).await;
    if !(connected) {
        return Err(DomainError::Validation(
            "No ACP agent connected".to_string(),
        ));
    };
    let request = AcpTurnRequest {
        turn_id: turn_id.clone(),
        messages: messages.clone(),
        tools: tools.clone(),
    };
    deps.registry
        .send_turn_request(user_id.clone(), request.clone())
        .await?;
    return Ok("turn_sent".to_string());
}

/// DomainService: SendToolResult
/// @dep
#[tracing::instrument(skip_all)]
pub async fn send_tool_result(
    deps: &Deps,
    user_id: String,
    turn_id: String,
    call_id: String,
    output: serde_json::Value,
) -> Result<String, DomainError> {
    // step: send
    let result = AcpToolResult {
        turn_id: turn_id.clone(),
        call_id: call_id.clone(),
        output: output.clone(),
    };
    deps.registry
        .send_tool_result(user_id.clone(), result.clone())
        .await?;
    return Ok("sent".to_string());
}

/// WsHandler: WsAgentChat
/// @path
#[tracing::instrument(skip_all)]
pub async fn ws_agent_chat(deps: &Deps, msg: String) -> Result<(), DomainError> {
    // step: handle
    let provider = serde_json::from_value::<LlmProvider>(
        deps.bus
            .invoke(serde_json::json!({ "type": "ResolveProvider" }))
            .await?,
    )
    .map_err(|e| DomainError::External(e.to_string()))?;
    let tools = serde_json::from_value::<Vec<ToolDef>>(
        deps.bus
            .invoke(serde_json::json!({ "type": "GetToolRegistry" }))
            .await?,
    )
    .map_err(|e| DomainError::External(e.to_string()))?;

    Ok(())
}

/// WsHandler: WsAgentAcp
/// @path
#[tracing::instrument(skip_all)]
pub async fn ws_agent_acp(msg: String) -> Result<(), DomainError> {
    // step: handle

    Ok(())
}

/// HttpRoute: AgentStatus
/// @method
/// @path
#[tracing::instrument(skip_all)]
pub async fn agent_status(deps: &Deps) -> Result<serde_json::Value, DomainError> {
    // step: handle
    let provider = serde_json::from_value::<LlmProvider>(
        deps.bus
            .invoke(serde_json::json!({ "type": "ResolveProvider" }))
            .await?,
    )
    .map_err(|e| DomainError::External(e.to_string()))?;
    let provider_name = match provider.clone() {
        LlmProvider::Bedrock { model_id } => "Bedrock".to_string(),
        LlmProvider::Byok { provider, .. } => provider,
        LlmProvider::AcpTunnel { .. } => "ACP Tunnel".to_string(),
        LlmProvider::VeilServerProxy => "veil-server (proxy)".to_string(),
        _ => unreachable!(),
    };
    let provider_mode = match provider.clone() {
        LlmProvider::Bedrock { model_id } => {
            serde_json::json!({ "mode": "bedrock".to_string(), "model": model_id.clone() })
        }
        LlmProvider::Byok {
            provider, model, ..
        } => {
            serde_json::json!({ "mode": "byok".to_string(), "provider": provider.clone(), "model": model.clone() })
        }
        LlmProvider::AcpTunnel { .. } => serde_json::json!({ "mode": "acp".to_string() }),
        LlmProvider::VeilServerProxy => serde_json::json!({ "mode": "veil-server".to_string() }),
        _ => unreachable!(),
    };
    let acp_tunnel =
        serde_json::json!({ "connected": false, "agents": serde_json::Value::Array(vec![]) });
    return Ok(
        serde_json::json!({ "provider": provider_name.clone(), "provider_mode": provider_mode.clone(), "acp_tunnel": acp_tunnel.clone() }),
    );
}
