//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: DeployUnitType
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum DeployUnitType {
    #[default]
    LambdaApi,
    LambdaConsumer,
    EcsService,
    EcsTask,
}

/// enum: EnvVarSource
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EnvVarSource {
    Literal { value: String },
    Ssm { path: String },
    Secret { arn: String },
    FromOutput { reference: String },
}

/// enum: DeploymentStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum DeploymentStatus {
    #[default]
    Active,
    Deploying,
    Failed,
    RollingBack,
    Inactive,
}

/// enum: HealthState
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum HealthState {
    #[default]
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// enum: EventType
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum EventType {
    #[default]
    Deployed,
    RolledBack,
    Scaled,
    ConfigUpdated,
    DriftDetected,
    DriftResolved,
    Failed,
}

/// enum: ActionRisk
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ActionRisk {
    #[default]
    Low,
    Medium,
    High,
}

/// enum: ActionType
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ActionType {
    #[default]
    CreateLambda,
    UpdateLambdaConfig,
    UpdateLambdaCode,
    CreateQueue,
    UpdateQueue,
    CreateEcsService,
    UpdateEcsService,
    ScaleEcsService,
    CreateApiRoute,
    UpdateApiRoute,
    DeleteLambda,
    DeleteQueue,
    DeleteEcsService,
}

/// enum: ReconcileResult
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReconcileResult {
    InSync,
    Create { actions: Vec<Action> },
    Update { actions: Vec<Action> },
    Remove { actions: Vec<Action> },
}

/// ValueObject: LambdaConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LambdaConfig {
    pub memory_mb: i64,
    pub timeout_seconds: i64,
    pub architecture: String,
    pub reserved_concurrency: Option<i64>,
    pub provisioned_concurrency: Option<i64>,
    pub layers: Vec<String>,
    pub ephemeral_storage_mb: i64,
}

impl LambdaConfig {
    pub fn new(architecture: String) -> Self {
        Self {
            memory_mb: 0,
            timeout_seconds: 0,
            architecture,
            reserved_concurrency: None,
            provisioned_concurrency: None,
            layers: Vec::new(),
            ephemeral_storage_mb: 0,
        }
    }
}

/// ValueObject: ApiGatewayConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiGatewayConfig {
    pub gateway: String,
    pub path_prefix: String,
    pub cors: bool,
    pub auth: String,
    pub throttle_rate: Option<i64>,
    pub throttle_burst: Option<i64>,
}

impl ApiGatewayConfig {
    pub fn new(gateway: String, path_prefix: String, auth: String) -> Self {
        Self {
            gateway,
            path_prefix,
            cors: false,
            auth,
            throttle_rate: None,
            throttle_burst: None,
        }
    }
}

/// ValueObject: QueueConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueConfig {
    pub visibility_timeout_seconds: i64,
    pub max_receive_count: i64,
    pub dlq: bool,
    pub batch_size: i64,
    pub batch_window_seconds: i64,
    pub fifo: bool,
    pub content_deduplication: bool,
}

impl QueueConfig {
    pub fn new() -> Self {
        Self {
            visibility_timeout_seconds: 0,
            max_receive_count: 0,
            dlq: false,
            batch_size: 0,
            batch_window_seconds: 0,
            fifo: false,
            content_deduplication: false,
        }
    }
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: EcsConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EcsConfig {
    pub cpu: i64,
    pub memory_mb: i64,
    pub desired_count: i64,
    pub min_count: i64,
    pub max_count: i64,
    pub health_check_path: String,
    pub health_check_interval_seconds: i64,
    pub port: i64,
}

impl EcsConfig {
    pub fn new(health_check_path: String) -> Self {
        Self {
            cpu: 0,
            memory_mb: 0,
            desired_count: 0,
            min_count: 0,
            max_count: 0,
            health_check_path,
            health_check_interval_seconds: 0,
            port: 0,
        }
    }
}

/// ValueObject: ScalingConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScalingConfig {
    pub metric: String,
    pub target_value: i64,
    pub scale_in_cooldown: i64,
    pub scale_out_cooldown: i64,
}

impl ScalingConfig {
    pub fn new(metric: String) -> Self {
        Self {
            metric,
            target_value: 0,
            scale_in_cooldown: 0,
            scale_out_cooldown: 0,
        }
    }
}

/// ValueObject: NetworkConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub vpc: String,
    pub security_groups: Vec<String>,
    pub subnets: String,
}

impl NetworkConfig {
    pub fn new(vpc: String, subnets: String) -> Self {
        Self {
            vpc,
            security_groups: Vec::new(),
            subnets,
        }
    }
}

/// ValueObject: DeployUnitConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployUnitConfig {
    pub name: String,
    pub unit_type: DeployUnitType,
    pub context: String,
    pub description: Option<String>,
    pub lambda: Option<LambdaConfig>,
    pub api_gateway: Option<ApiGatewayConfig>,
    pub queue: Option<QueueConfig>,
    pub ecs: Option<EcsConfig>,
    pub scaling: Option<ScalingConfig>,
    pub env: std::collections::HashMap<String, EnvVarSource>,
}

impl DeployUnitConfig {
    pub fn new(name: String, unit_type: DeployUnitType, context: String) -> Self {
        Self {
            name,
            unit_type,
            context,
            description: None,
            lambda: None,
            api_gateway: None,
            queue: None,
            ecs: None,
            scaling: None,
            env: std::collections::HashMap::new(),
        }
    }
}

/// ValueObject: DeployConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployConfig {
    pub region: String,
    pub project_prefix: String,
    pub account_id: Option<String>,
    pub env: std::collections::HashMap<String, EnvVarSource>,
    pub network: Option<NetworkConfig>,
    pub units: Vec<DeployUnitConfig>,
}

impl DeployConfig {
    pub fn new(region: String, project_prefix: String) -> Self {
        Self {
            region,
            project_prefix,
            account_id: None,
            env: std::collections::HashMap::new(),
            network: None,
            units: Vec::new(),
        }
    }
}

/// ValueObject: ArtifactInfo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactInfo {
    pub hash: String,
    pub image_uri: Option<String>,
    pub compiled_at: DateTime<Utc>,
    pub source_commit: Option<String>,
}

impl ArtifactInfo {
    pub fn new(hash: String, compiled_at: DateTime<Utc>) -> Self {
        Self {
            hash,
            image_uri: None,
            compiled_at,
            source_commit: None,
        }
    }
}

/// ValueObject: DeployedConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployedConfig {
    pub memory_mb: Option<i64>,
    pub timeout_seconds: Option<i64>,
    pub architecture: Option<String>,
    pub reserved_concurrency: Option<i64>,
    pub cpu: Option<i64>,
    pub desired_count: Option<i64>,
    pub env_vars: std::collections::HashMap<String, EnvVarSource>,
}

impl DeployedConfig {
    pub fn new() -> Self {
        Self {
            memory_mb: None,
            timeout_seconds: None,
            architecture: None,
            reserved_concurrency: None,
            cpu: None,
            desired_count: None,
            env_vars: std::collections::HashMap::new(),
        }
    }
}

impl Default for DeployedConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: AwsResources
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AwsResources {
    pub lambda_arn: Option<String>,
    pub lambda_version: Option<String>,
    pub api_gw_id: Option<String>,
    pub api_gw_route_id: Option<String>,
    pub sqs_queue_url: Option<String>,
    pub sqs_queue_arn: Option<String>,
    pub sqs_dlq_url: Option<String>,
    pub ecs_service_arn: Option<String>,
    pub ecs_cluster_arn: Option<String>,
    pub role_arn: Option<String>,
    pub log_group: Option<String>,
    pub ecr_repository: Option<String>,
}

impl AwsResources {
    pub fn new() -> Self {
        Self {
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
        }
    }
}

impl Default for AwsResources {
    fn default() -> Self {
        Self::new()
    }
}

/// ValueObject: HealthStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthStatus {
    pub last_check: Option<DateTime<Utc>>,
    pub status: HealthState,
    pub error_rate_1h: Option<String>,
    pub p99_latency_ms: Option<i64>,
    pub invocations_1h: Option<i64>,
}

impl HealthStatus {
    pub fn new(status: HealthState) -> Self {
        Self {
            last_check: None,
            status,
            error_rate_1h: None,
            p99_latency_ms: None,
            invocations_1h: None,
        }
    }
}

/// ValueObject: DeploymentState
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeploymentState {
    pub project: String,
    pub context: String,
    pub unit_name: String,
    pub unit_type: DeployUnitType,
    pub environment: String,
    pub region: String,
    pub status: DeploymentStatus,
    pub version: i64,
    pub artifact: ArtifactInfo,
    pub config: DeployedConfig,
    pub aws_resources: AwsResources,
    pub health: HealthStatus,
    pub deployed_at: DateTime<Utc>,
    pub deployed_by: String,
    pub description: Option<String>,
}

impl DeploymentState {
    pub fn new(
        project: String,
        context: String,
        unit_name: String,
        unit_type: DeployUnitType,
        environment: String,
        region: String,
        status: DeploymentStatus,
        artifact: ArtifactInfo,
        health: HealthStatus,
        deployed_at: DateTime<Utc>,
        deployed_by: String,
    ) -> Self {
        Self {
            project,
            context,
            unit_name,
            unit_type,
            environment,
            region,
            status,
            version: 0,
            artifact,
            config: DeployedConfig::default(),
            aws_resources: AwsResources::default(),
            health,
            deployed_at,
            deployed_by,
            description: None,
        }
    }
}

/// ValueObject: ConfigChange
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigChange {
    pub field: String,
    pub from: Option<serde_json::Value>,
    pub to: Option<serde_json::Value>,
}

impl ConfigChange {
    pub fn new(field: String) -> Self {
        Self {
            field,
            from: None,
            to: None,
        }
    }
}

/// ValueObject: DeployEvent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployEvent {
    pub event_type: EventType,
    pub version: i64,
    pub actor: String,
    pub changes: Vec<ConfigChange>,
    pub message: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: Option<i64>,
}

impl DeployEvent {
    pub fn new(event_type: EventType, actor: String, timestamp: DateTime<Utc>) -> Self {
        Self {
            event_type,
            version: 0,
            actor,
            changes: Vec::new(),
            message: None,
            timestamp,
            duration_ms: None,
        }
    }
}

/// ValueObject: Action
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub action_type: ActionType,
    pub description: String,
    pub risk: ActionRisk,
    pub details: serde_json::Value,
}

impl Action {
    pub fn new(action_type: ActionType, description: String, risk: ActionRisk) -> Self {
        Self {
            action_type,
            description,
            risk,
            details: serde_json::json!({}),
        }
    }
}

/// ValueObject: ActionResult
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
    pub resource_updates: std::collections::HashMap<String, String>,
    pub duration_ms: i64,
}

impl ActionResult {
    pub fn new(message: String) -> Self {
        Self {
            success: false,
            message,
            resource_updates: std::collections::HashMap::new(),
            duration_ms: 0,
        }
    }
}
