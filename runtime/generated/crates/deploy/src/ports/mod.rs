//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: DeploymentStateStore
#[async_trait]
pub trait DeploymentStateStore: Send + Sync {
    async fn get_current(
        &self,
        environment: String,
        unit_name: String,
    ) -> Result<Option<DeploymentState>, DomainError>;
    async fn save_current(&self, state: DeploymentState) -> Result<(), DomainError>;
    async fn save_version(&self, state: DeploymentState) -> Result<(), DomainError>;
    async fn get_version(
        &self,
        environment: String,
        unit_name: String,
        version: i64,
    ) -> Result<Option<DeploymentState>, DomainError>;
    async fn list_versions(
        &self,
        environment: String,
        unit_name: String,
        limit: i64,
    ) -> Result<Vec<DeploymentState>, DomainError>;
    async fn append_event(
        &self,
        environment: String,
        unit_name: String,
        event: DeployEvent,
    ) -> Result<(), DomainError>;
    async fn get_events(
        &self,
        environment: String,
        unit_name: String,
        limit: i64,
    ) -> Result<Vec<DeployEvent>, DomainError>;
    async fn list_deployments(&self) -> Result<Vec<DeploymentState>, DomainError>;
}

/// Port: ActionExecutor
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    async fn execute_action(
        &self,
        action: Action,
        state: DeploymentState,
    ) -> Result<ActionResult, DomainError>;
}

/// Port: DeployExec
#[async_trait]
pub trait DeployExec: Send + Sync {
    async fn list_environments(&self) -> Result<String, DomainError>;
    async fn read_project_deploy(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
    ) -> Result<String, DomainError>;
    async fn sync_hub_to_s3(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
    ) -> Result<String, DomainError>;
    async fn plan_provision(
        &self,
        project_slug: String,
        environment: String,
    ) -> Result<String, DomainError>;
    async fn plan_provision_repo(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
        environment: String,
    ) -> Result<String, DomainError>;
    async fn start_provision(
        &self,
        project_slug: String,
        environment: String,
    ) -> Result<String, DomainError>;
    async fn start_provision_repo(
        &self,
        repo_id: String,
        branch: String,
        slug: String,
        environment: String,
    ) -> Result<String, DomainError>;
    async fn get_provision_job(&self, job_id: String) -> Result<String, DomainError>;
    async fn provision_unit(
        &self,
        project_slug: String,
        environment: String,
        unit_name: String,
    ) -> Result<String, DomainError>;
    async fn clear_unit_state(
        &self,
        environment: String,
        unit_name: String,
    ) -> Result<String, DomainError>;
}
