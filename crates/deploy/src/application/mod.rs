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
    pub executor: std::sync::Arc<dyn ActionExecutor + Send + Sync>,
    pub bus: std::sync::Arc<dyn Bus + Send + Sync>,
    pub exec: std::sync::Arc<dyn DeployExec + Send + Sync>,
    pub store: std::sync::Arc<dyn DeploymentStateStore + Send + Sync>,
}

/// DomainService: Reconcile
/// @dep
#[tracing::instrument(skip_all)]
pub async fn reconcile(
    deps: &Deps,
    desired: DeployUnitConfig,
    environment: String,
    new_artifact_hash: Option<String>,
) -> Result<ReconcileResult, DomainError> {
    // step: load_current
    let current = deps
        .store
        .get_current(environment.clone(), desired.name.clone())
        .await?;

    // step: reconcile
    if current.is_none() {
        let mut actions = vec![];
        match desired.unit_type {
            DeployUnitType::LambdaApi => {
                actions.push(Action {
                    action_type: ActionType::CreateLambda.clone(),
                    description: format!("Create Lambda function {}", desired.name),
                    risk: ActionRisk::Low.clone(),
                    details: serde_json::json!(serde_json::Value::Null),
                });
                actions.push(Action {
                    action_type: ActionType::CreateApiRoute.clone(),
                    description: format!(
                        "Attach API routes for {} on existing gateway (api_gateway.gateway)",
                        desired.name
                    ),
                    risk: ActionRisk::Low.clone(),
                    details: serde_json::json!(serde_json::Value::Null),
                });
            }
            DeployUnitType::LambdaConsumer => {
                actions.push(Action {
                    action_type: ActionType::CreateLambda.clone(),
                    description: format!("Create Lambda function {}", desired.name),
                    risk: ActionRisk::Low.clone(),
                    details: serde_json::json!(serde_json::Value::Null),
                });
                actions.push(Action {
                    action_type: ActionType::CreateQueue.clone(),
                    description: format!("Create SQS queue for {}", desired.name),
                    risk: ActionRisk::Low.clone(),
                    details: serde_json::json!(serde_json::Value::Null),
                });
            }
            DeployUnitType::EcsService => actions.push(Action {
                action_type: ActionType::CreateEcsService.clone(),
                description: format!("Create ECS service {}", desired.name),
                risk: ActionRisk::Medium.clone(),
                details: serde_json::json!(serde_json::Value::Null),
            }),
            DeployUnitType::EcsTask => actions.push(Action {
                action_type: ActionType::CreateEcsService.clone(),
                description: format!("Create ECS task {}", desired.name),
                risk: ActionRisk::Low.clone(),
                details: serde_json::json!(serde_json::Value::Null),
            }),
            _ => unreachable!(),
        };
        return Ok(ReconcileResult::Create {
            actions: actions.clone(),
        });
    };
    let state = current.clone().ok_or(DomainError::NotFound)?;
    let mut actions = vec![];
    if new_artifact_hash.is_some() {
        if new_artifact_hash.clone().ok_or(DomainError::NotFound)? != state.artifact.hash {
            actions.push(Action {
                action_type: ActionType::UpdateLambdaCode.clone(),
                description: format!("Update code for {}", desired.name),
                risk: ActionRisk::Low.clone(),
                details: serde_json::json!(serde_json::Value::Null),
            });
        };
    };
    if desired.lambda.is_some() {
        let lc = desired.lambda.unwrap();
        if state.config.memory_mb.is_some() && state.config.memory_mb.unwrap() != lc.memory_mb {
            actions.push(Action {
                action_type: ActionType::UpdateLambdaConfig.clone(),
                description: format!(
                    "Update {} memory: {}MB → {}MB",
                    desired.name,
                    state.config.memory_mb.unwrap(),
                    lc.memory_mb
                ),
                risk: ActionRisk::Low.clone(),
                details: serde_json::json!(serde_json::Value::Null),
            });
        };
        if state.config.timeout_seconds.is_some()
            && state.config.timeout_seconds.unwrap() != lc.timeout_seconds
        {
            actions.push(Action {
                action_type: ActionType::UpdateLambdaConfig.clone(),
                description: format!(
                    "Update {} timeout: {}s → {}s",
                    desired.name,
                    state.config.timeout_seconds.unwrap(),
                    lc.timeout_seconds
                ),
                risk: ActionRisk::Low.clone(),
                details: serde_json::json!(serde_json::Value::Null),
            });
        };
    };
    if actions.is_empty() {
        return Ok(ReconcileResult::InSync);
    };
    return Ok(ReconcileResult::Update {
        actions: actions.clone(),
    });
}

/// DomainService: DeployUnit
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn deploy_unit(
    deps: &Deps,
    desired: DeployUnitConfig,
    environment: String,
    artifact_hash: String,
    actor: String,
    message: Option<String>,
) -> Result<DeploymentState, DomainError> {
    // step: reconcile
    let plan = serde_json::from_value::<ReconcileResult>(deps.bus.invoke(serde_json::json!({ "type": "Reconcile", "desired": desired.clone(), "environment": environment.clone(), "new_artifact_hash": artifact_hash.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;

    // step: execute_actions
    let state_opt = deps
        .store
        .get_current(environment.clone(), desired.name.clone())
        .await?;
    match plan.clone() {
        ReconcileResult::InSync => {}
        ReconcileResult::Create { actions } => {
            if state_opt.is_some() {
                for action in actions {
                    deps.executor
                        .execute_action(
                            action.clone(),
                            state_opt.clone().ok_or(DomainError::NotFound)?,
                        )
                        .await?;
                }
            }
        }
        ReconcileResult::Update { actions } => {
            if state_opt.is_some() {
                for action in actions {
                    deps.executor
                        .execute_action(
                            action.clone(),
                            state_opt.clone().ok_or(DomainError::NotFound)?,
                        )
                        .await?;
                }
            }
        }
        _ => unreachable!(),
    };

    // step: save_state
    let now = Utc::now();
    let current = deps
        .store
        .get_current(environment.clone(), desired.name.clone())
        .await?;
    let version = if current.is_some() {
        current.clone().ok_or(DomainError::NotFound)?.version + 1
    } else {
        1
    };
    let new_state = DeploymentState {
        project: "".to_string(),
        context: desired.context.clone(),
        unit_name: desired.name.clone(),
        unit_type: desired.unit_type.clone(),
        environment: environment.clone(),
        region: "".to_string(),
        status: DeploymentStatus::Active.clone(),
        version: version.clone(),
        artifact: ArtifactInfo {
            hash: artifact_hash.clone(),
            image_uri: None,
            compiled_at: now.clone(),
            source_commit: None,
        },
        config: DeployedConfig {
            memory_mb: None,
            timeout_seconds: None,
            architecture: None,
            reserved_concurrency: None,
            cpu: None,
            desired_count: None,
            env_vars: HashMap::new(),
        },
        aws_resources: AwsResources {
            lambda_arn: None,
            lambda_version: None,
            api_gw_id: None,
            api_gw_route_id: None,
            sqs_queue_url: None,
            sqs_queue_arn: None,
            sqs_dlq_url: None,
            ecs_service_arn: None,
            ecs_cluster_arn: None,
            role_arn: None,
            log_group: None,
            ecr_repository: None,
        },
        health: HealthStatus {
            last_check: None,
            status: HealthState::Unknown.clone(),
            error_rate_1h: None,
            p99_latency_ms: None,
            invocations_1h: None,
        },
        deployed_at: now.clone(),
        deployed_by: actor.clone(),
        description: desired.description.clone(),
    };
    deps.store.save_current(new_state.clone()).await?;
    deps.store.save_version(new_state.clone()).await?;
    let event = DeployEvent {
        event_type: EventType::Deployed.clone(),
        version: version.clone(),
        actor: actor.clone(),
        changes: vec![],
        message: message.clone(),
        timestamp: now.clone(),
        duration_ms: None,
    };
    deps.store
        .append_event(environment.clone(), desired.name.clone(), event.clone())
        .await?;
    return Ok(new_state);
}

/// DomainService: RollbackDeployment
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn rollback_deployment(
    deps: &Deps,
    environment: String,
    unit_name: String,
    target_version: Option<i64>,
    actor: String,
    reason: Option<String>,
) -> Result<DeploymentState, DomainError> {
    // step: load
    let current = deps
        .store
        .get_current(environment.clone(), unit_name.clone())
        .await?;
    let v = target_version
        .clone()
        .unwrap_or(current.clone().ok_or(DomainError::NotFound)?.version - 1);
    let target = deps
        .store
        .get_version(environment.clone(), unit_name.clone(), v.clone())
        .await?;

    // step: rollback
    let state = target.clone().ok_or(DomainError::NotFound)?;
    let now = Utc::now();
    let new_version = current.clone().ok_or(DomainError::NotFound)?.version + 1;
    let rolled_back = DeploymentState {
        project: state.project.clone(),
        context: state.context.clone(),
        unit_name: unit_name.clone(),
        unit_type: state.unit_type.clone(),
        environment: environment.clone(),
        region: state.region.clone(),
        status: DeploymentStatus::Active.clone(),
        version: new_version,
        artifact: state.artifact.clone(),
        config: state.config.clone(),
        aws_resources: state.aws_resources.clone(),
        health: state.health.clone(),
        deployed_at: now.clone(),
        deployed_by: actor.clone(),
        description: state.description.clone(),
    };
    deps.store.save_current(rolled_back.clone()).await?;
    deps.store.save_version(rolled_back.clone()).await?;
    let event = DeployEvent {
        event_type: EventType::RolledBack.clone(),
        version: new_version,
        actor: actor.clone(),
        changes: vec![],
        message: reason.clone(),
        timestamp: now.clone(),
        duration_ms: None,
    };
    deps.store
        .append_event(environment.clone(), unit_name.clone(), event.clone())
        .await?;
    return Ok(rolled_back);
}

/// DomainService: GetDeploymentStatus
/// @dep
#[tracing::instrument(skip_all)]
pub async fn get_deployment_status(
    deps: &Deps,
    environment: String,
    unit_name: String,
) -> Result<serde_json::Value, DomainError> {
    // step: load
    let current = deps
        .store
        .get_current(environment.clone(), unit_name.clone())
        .await?;
    let events = deps
        .store
        .get_events(environment.clone(), unit_name.clone(), 10)
        .await?;
    return Ok(serde_json::json!({ "state": current.clone(), "recent_events": events.clone() }));
}

/// DomainService: ListAllDeployments
/// @dep
#[tracing::instrument(skip_all)]
pub async fn list_all_deployments(
    deps: &Deps,
    environment: Option<String>,
    project: Option<String>,
) -> Result<Vec<DeploymentState>, DomainError> {
    // step: query
    let results = deps.store.list_deployments().await?;
    return Ok(results);
}

/// DomainService: ListDeployEnvironments
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn list_deploy_environments(deps: &Deps) -> Result<serde_json::Value, DomainError> {
    // step: query
    let _probe = deps
        .store
        .get_current("_".to_string(), "_".to_string())
        .await?;
    let raw = deps.exec.list_environments().await?;
    let mut catalog: serde_json::Value = serde_json::from_str::<_>(&raw)?;
    return Ok(catalog);
}

/// DomainService: PlanProvision
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn plan_provision(
    deps: &Deps,
    project_slug: String,
    environment: String,
    repo_id: String,
    branch: String,
) -> Result<serde_json::Value, DomainError> {
    // step: plan
    let _probe = deps
        .store
        .get_current(environment.clone(), project_slug.clone())
        .await?;
    let raw = deps
        .exec
        .plan_provision_repo(
            repo_id.clone(),
            branch.clone(),
            project_slug.clone(),
            environment.clone(),
        )
        .await?;
    let mut plan: serde_json::Value = serde_json::from_str::<_>(&raw)?;
    return Ok(plan);
}

/// DomainService: ProvisionProject
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn provision_project(
    deps: &Deps,
    project_slug: String,
    environment: String,
    repo_id: String,
    branch: String,
) -> Result<serde_json::Value, DomainError> {
    // step: provision
    let _probe = deps
        .store
        .get_current(environment.clone(), project_slug.clone())
        .await?;
    let raw = deps
        .exec
        .start_provision_repo(
            repo_id.clone(),
            branch.clone(),
            project_slug.clone(),
            environment.clone(),
        )
        .await?;
    let mut job: serde_json::Value = serde_json::from_str::<_>(&raw)?;
    return Ok(job);
}

/// DomainService: GetProvisionJob
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn get_provision_job(
    deps: &Deps,
    job_id: String,
) -> Result<serde_json::Value, DomainError> {
    // step: query
    let _probe = deps
        .store
        .get_current("_".to_string(), "_".to_string())
        .await?;
    let raw = deps.exec.get_provision_job(job_id.clone()).await?;
    let mut job: serde_json::Value = serde_json::from_str::<_>(&raw)?;
    return Ok(job);
}

/// DomainService: ScaleService
/// @dep
/// @dep
#[tracing::instrument(skip_all)]
pub async fn scale_service(
    deps: &Deps,
    environment: String,
    unit_name: String,
    desired_count: i64,
    actor: String,
    reason: Option<String>,
) -> Result<DeploymentState, DomainError> {
    // step: scale
    let current = deps
        .store
        .get_current(environment.clone(), unit_name.clone())
        .await?;
    let state = current.clone().ok_or(DomainError::NotFound)?;
    let action = Action {
        action_type: ActionType::ScaleEcsService.clone(),
        description: format!("Scale {} to {} tasks", unit_name, desired_count),
        risk: ActionRisk::Low.clone(),
        details: serde_json::json!(serde_json::Value::Null),
    };
    deps.executor
        .execute_action(action.clone(), state.clone())
        .await?;
    let now = Utc::now();
    let event = DeployEvent {
        event_type: EventType::Scaled.clone(),
        version: state.version.clone(),
        actor: actor.clone(),
        changes: vec![ConfigChange {
            field: "desired_count".to_string(),
            from: Some(serde_json::json!(state.config.desired_count.clone())),
            to: Some(serde_json::json!(desired_count)),
        }],
        message: reason.clone(),
        timestamp: now.clone(),
        duration_ms: None,
    };
    deps.store
        .append_event(environment.clone(), unit_name.clone(), event.clone())
        .await?;
    return Ok(state);
}

/// Tool: DeployTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn deploy_tool(
    deps: &Deps,
    unit_name: String,
    environment: String,
    artifact_hash: Option<String>,
    message: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let hash = artifact_hash.clone().unwrap_or("latest".to_string());
    let result = serde_json::from_value::<DeploymentState>(deps.bus.invoke(serde_json::json!({ "type": "DeployUnit", "desired": DeployUnitConfig { name: unit_name.clone(), unit_type: DeployUnitType::LambdaApi.clone(), context: unit_name.clone(), description: None, lambda: None, api_gateway: None, queue: None, ecs: None, scaling: None, env: HashMap::new() }, "environment": environment.clone(), "artifact_hash": hash.clone(), "actor": "agent".to_string(), "message": message.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    return Ok(
        serde_json::json!({ "version": serde_json::json!(result.clone())["version"].clone(), "unit_name": serde_json::json!(result.clone())["unit_name"].clone(), "environment": serde_json::json!(result.clone())["environment"].clone(), "status": "active".to_string(), "summary": format!("Deployed {} v{} to {}. Status: active.", unit_name, result.version, environment) }),
    );
}

/// Tool: RollbackTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn rollback_tool(
    deps: &Deps,
    unit_name: String,
    environment: String,
    target_version: Option<i64>,
    reason: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let result = serde_json::from_value::<DeploymentState>(deps.bus.invoke(serde_json::json!({ "type": "RollbackDeployment", "environment": environment.clone(), "unit_name": unit_name.clone(), "target_version": target_version.clone(), "actor": "agent".to_string(), "reason": reason.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    return Ok(
        serde_json::json!({ "version": serde_json::json!(result.clone())["version"].clone(), "unit_name": unit_name.clone(), "environment": environment.clone(), "status": "active".to_string(), "summary": format!("Rolled back {} in {} to v{}. Reason: {}", unit_name, environment, result.version, reason.unwrap_or("no reason given".to_string())) }),
    );
}

/// Tool: DeploymentStatusTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn deployment_status_tool(
    deps: &Deps,
    unit_name: String,
    environment: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let result = deps.bus.invoke(serde_json::json!({ "type": "GetDeploymentStatus", "environment": environment.clone(), "unit_name": unit_name.clone() })).await?;
    return Ok(
        serde_json::json!({ "state": result["state"].clone(), "recent_events": result["recent_events"].clone(), "summary": format!("Deployment {} in {}: status={}, version={}, health={}", unit_name, environment, result["state"]["status"], result["state"]["version"], result["state"]["health"]["status"]) }),
    );
}

/// Tool: ListDeploymentsTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn list_deployments_tool(
    deps: &Deps,
    environment: Option<String>,
    project: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let results = serde_json::from_value::<Vec<DeploymentState>>(deps.bus.invoke(serde_json::json!({ "type": "ListAllDeployments", "environment": environment.clone(), "project": project.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    let count = (results.len() as i64);
    return Ok(
        serde_json::json!({ "deployments": results.clone(), "count": count.clone(), "summary": format!("Found {} deployments{}{}.", count, if environment.is_some() { format!(" in {}", environment.unwrap()) } else { "".to_string() }, if project.is_some() { format!(" for project {}", project.unwrap()) } else { "".to_string() }) }),
    );
}

/// Tool: DeploymentDiffTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn deployment_diff_tool(
    deps: &Deps,
    unit_name: String,
    environment: String,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let desired = DeployUnitConfig {
        name: unit_name.clone(),
        unit_type: DeployUnitType::LambdaApi.clone(),
        context: unit_name.clone(),
        description: None,
        lambda: None,
        api_gateway: None,
        queue: None,
        ecs: None,
        scaling: None,
        env: HashMap::new(),
    };
    let plan = serde_json::from_value::<ReconcileResult>(deps.bus.invoke(serde_json::json!({ "type": "Reconcile", "desired": desired.clone(), "environment": environment.clone(), "new_artifact_hash": serde_json::Value::Null })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    match plan.clone() {
        ReconcileResult::InSync => {
            return Ok(
                serde_json::json!({ "in_sync": true, "changes": serde_json::Value::Array(vec![]), "summary": format!("{} in {} is in sync. No changes needed.", unit_name, environment) }),
            );
        }
        ReconcileResult::Create { actions } => {
            return Ok(
                serde_json::json!({ "in_sync": false, "changes": actions.clone(), "summary": format!("{} does not exist in {}. Would create with {} actions.", unit_name, environment, actions.len()) }),
            );
        }
        ReconcileResult::Update { actions } => {
            return Ok(
                serde_json::json!({ "in_sync": false, "changes": actions.clone(), "summary": format!("{} in {} has {} pending changes.", unit_name, environment, actions.len()) }),
            );
        }
        ReconcileResult::Remove { actions } => {
            return Ok(
                serde_json::json!({ "in_sync": false, "changes": actions.clone(), "summary": format!("{} in {} would be removed ({} actions).", unit_name, environment, actions.len()) }),
            );
        }
        _ => unreachable!(),
    };
}

/// Tool: ScaleTool
/// @desc
#[tracing::instrument(skip_all)]
pub async fn scale_tool(
    deps: &Deps,
    unit_name: String,
    environment: String,
    desired_count: i64,
    reason: Option<String>,
) -> Result<serde_json::Value, DomainError> {
    // step: execute
    let result = serde_json::from_value::<DeploymentState>(deps.bus.invoke(serde_json::json!({ "type": "ScaleService", "environment": environment.clone(), "unit_name": unit_name.clone(), "desired_count": desired_count.clone(), "actor": "agent".to_string(), "reason": reason.clone() })).await?).map_err(|e| DomainError::External(e.to_string()))?;
    return Ok(
        serde_json::json!({ "unit_name": unit_name.clone(), "environment": environment.clone(), "desired_count": desired_count.clone(), "summary": format!("Scaled {} in {} to {} tasks. {}", unit_name, environment, desired_count, reason.unwrap_or("".to_string())) }),
    );
}
