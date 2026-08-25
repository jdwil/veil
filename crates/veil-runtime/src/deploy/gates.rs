//! Approval gates — configurable per-environment approval logic.
//!
//! When gate = "sign_off":
//! - Deploy request creates a PENDING job with status AwaitingApproval
//! - Human approves via API → then pipeline executes
//! When gate = "none":
//! - Deploy executes immediately

use super::types::*;

/// Check whether a deploy needs approval before proceeding.
pub fn requires_approval(config: &ProjectDeployConfig, environment: &str) -> bool {
    config
        .gates
        .get(environment)
        .map(|g| *g == GatePolicy::SignOff)
        .unwrap_or(false)
}

/// Validate an approval action. Returns Ok(()) if the job can be approved.
pub fn validate_approval(job: &DeployJob) -> Result<(), String> {
    match job.status {
        JobStatus::AwaitingApproval => Ok(()),
        JobStatus::Pending => Err("Job is not yet in awaiting_approval state".into()),
        JobStatus::Running => Err("Job is already running".into()),
        JobStatus::Completed => Err("Job already completed".into()),
        JobStatus::Failed => Err("Job has failed — cannot approve a failed job".into()),
        JobStatus::Cancelled => Err("Job was cancelled".into()),
        JobStatus::RolledBack => Err("Job was rolled back".into()),
    }
}
