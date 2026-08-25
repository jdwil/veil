//! Deploy pipeline types — job tracking, steps, status, configuration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// ─── Job & Step ─────────────────────────────────────────────────────────────

/// A deploy job represents a single pipeline execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployJob {
    pub id: String,
    pub project_slug: String,
    pub environment: String,
    pub status: JobStatus,
    pub steps: Vec<StepResult>,
    pub triggered_by: String,
    pub triggered_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub dry_run: bool,
    /// Git commit SHA from the project's S3 git origin.
    pub commit_sha: Option<String>,
    /// Terraform state version after apply.
    pub terraform_state_version: Option<String>,
    /// Build artifact hash.
    pub artifact_hash: Option<String>,
    /// Error message if failed.
    pub error: Option<String>,
}

impl DeployJob {
    pub fn new(
        project_slug: String,
        environment: String,
        triggered_by: String,
        dry_run: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            project_slug,
            environment,
            status: JobStatus::Pending,
            steps: Vec::new(),
            triggered_by,
            triggered_at: Utc::now(),
            completed_at: None,
            dry_run,
            commit_sha: None,
            terraform_state_version: None,
            artifact_hash: None,
            error: None,
        }
    }
}

/// Overall status of a deploy job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    AwaitingApproval,
    Running,
    Completed,
    Failed,
    Cancelled,
    RolledBack,
}

/// Which pipeline steps to run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStepKind {
    Infrastructure,
    Build,
    Deploy,
}

impl DeployStepKind {
    pub fn all() -> Vec<Self> {
        vec![Self::Infrastructure, Self::Build, Self::Deploy]
    }

    pub fn from_str_vec(items: &[String]) -> Vec<Self> {
        if items.iter().any(|s| s == "all") {
            return Self::all();
        }
        items
            .iter()
            .filter_map(|s| match s.as_str() {
                "infrastructure" => Some(Self::Infrastructure),
                "build" => Some(Self::Build),
                "deploy" => Some(Self::Deploy),
                _ => None,
            })
            .collect()
    }
}

/// Result of a single step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step: DeployStepKind,
    pub status: StepStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub output: Option<String>,
    pub error: Option<String>,
}

impl StepResult {
    pub fn pending(step: DeployStepKind) -> Self {
        Self {
            step,
            status: StepStatus::Pending,
            started_at: None,
            completed_at: None,
            output: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

// ─── Configuration ──────────────────────────────────────────────────────────

/// Deploy configuration loaded from project veil.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDeployConfig {
    pub deploy_type: DeployType,
    pub infrastructure: Option<InfraConfig>,
    pub build: Option<BuildConfig>,
    pub artifacts: Option<ArtifactConfig>,
    pub gates: HashMap<String, GatePolicy>,
}

impl Default for ProjectDeployConfig {
    fn default() -> Self {
        Self {
            deploy_type: DeployType::Lambda,
            infrastructure: None,
            build: None,
            artifacts: None,
            gates: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployType {
    Lambda,
    Frontend,
    Ecs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraConfig {
    pub backend_bucket: String,
    pub backend_key: String,
    pub backend_region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    pub target: BuildTarget,
    /// For Lambda cross-compilation.
    pub rust_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildTarget {
    Rust,
    Typescript,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactConfig {
    pub bucket: String,
}

/// Approval gate policy for an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatePolicy {
    None,
    SignOff,
}

impl GatePolicy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "sign_off" | "signoff" => Self::SignOff,
            _ => Self::None,
        }
    }
}

// ─── Drift ──────────────────────────────────────────────────────────────────

/// Drift detection result from terraform plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftStatus {
    pub detected: bool,
    pub changes: usize,
    pub plan_output: String,
    pub checked_at: DateTime<Utc>,
}

impl DriftStatus {
    pub fn in_sync() -> Self {
        Self {
            detected: false,
            changes: 0,
            plan_output: String::new(),
            checked_at: Utc::now(),
        }
    }

    pub fn drifted(changes: usize, plan_output: String) -> Self {
        Self {
            detected: true,
            changes,
            plan_output,
            checked_at: Utc::now(),
        }
    }
}

// ─── API Request/Response ───────────────────────────────────────────────────

/// POST /api/projects/{slug}/deploy request body.
#[derive(Debug, Deserialize)]
pub struct TriggerDeployRequest {
    pub environment: String,
    #[serde(default = "default_steps")]
    pub steps: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}

fn default_steps() -> Vec<String> {
    vec!["all".into()]
}

/// POST /api/projects/{slug}/deploy response.
#[derive(Debug, Serialize)]
pub struct TriggerDeployResponse {
    pub job_id: String,
    pub status: JobStatus,
}

/// GET /api/projects/{slug}/deploy/status response.
#[derive(Debug, Serialize)]
pub struct DeployStatusResponse {
    pub last_deploy: Option<DeployJobSummary>,
    pub drift: Option<DriftStatus>,
    pub pending_changes: PendingChanges,
}

#[derive(Debug, Serialize)]
pub struct DeployJobSummary {
    pub id: String,
    pub status: JobStatus,
    pub environment: String,
    pub at: DateTime<Utc>,
    pub steps: Vec<StepResult>,
}

#[derive(Debug, Serialize)]
pub struct PendingChanges {
    pub infra: bool,
    pub code: bool,
}

/// GET /api/projects/{slug}/deploy/plan response.
#[derive(Debug, Serialize)]
pub struct PlanResponse {
    pub plan_output: String,
    pub changes: Vec<String>,
}

/// GET /api/projects/{slug}/deploy/history response item.
#[derive(Debug, Serialize)]
pub struct DeployHistoryItem {
    pub id: String,
    pub status: JobStatus,
    pub environment: String,
    pub at: DateTime<Utc>,
    pub by: String,
    pub steps: Vec<DeployStepKind>,
}
