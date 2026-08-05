//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Repository: ChangeRequestRepo
#[async_trait]
pub trait ChangeRequestRepo: Send + Sync {
    async fn find(&self, id: Uuid) -> Result<Option<ChangeRequest>, DomainError>;
    async fn list_by_repo(
        &self,
        repo_id: Uuid,
        status: Option<PrStatus>,
    ) -> Result<Vec<ChangeRequest>, DomainError>;
    async fn list_open(&self, repo_id: Uuid) -> Result<Vec<ChangeRequest>, DomainError>;
    async fn list_all(&self, status: Option<PrStatus>) -> Result<Vec<ChangeRequest>, DomainError>;
    async fn save(&self, cr: ChangeRequest) -> Result<(), DomainError>;
}

/// Repository: ApprovalRepo
#[async_trait]
pub trait ApprovalRepo: Send + Sync {
    async fn find_for_pr(&self, pr_id: Uuid) -> Result<Vec<Approval>, DomainError>;
    async fn save(&self, approval: Approval) -> Result<(), DomainError>;
}

/// Repository: CiRunRepo
#[async_trait]
pub trait CiRunRepo: Send + Sync {
    async fn latest_for_pr(&self, pr_id: Uuid) -> Result<Option<CiRun>, DomainError>;
    async fn list_for_pr(&self, pr_id: Uuid) -> Result<Vec<CiRun>, DomainError>;
    async fn save(&self, run: CiRun) -> Result<(), DomainError>;
}

/// Repository: CommentRepo
#[async_trait]
pub trait CommentRepo: Send + Sync {
    async fn list_for_pr(&self, pr_id: Uuid) -> Result<Vec<ReviewComment>, DomainError>;
    async fn save(&self, comment: ReviewComment) -> Result<(), DomainError>;
    async fn resolve(&self, id: Uuid) -> Result<(), DomainError>;
}

/// Port: GitService
#[async_trait]
pub trait GitService: Send + Sync {
    async fn init_repo(&self, slug: String) -> Result<(), DomainError>;
    async fn repo_exists(&self, slug: String) -> Result<bool, DomainError>;
    async fn create_branch(
        &self,
        slug: String,
        branch_name: String,
        from_ref: String,
    ) -> Result<String, DomainError>;
    async fn delete_branch(&self, slug: String, branch_name: String) -> Result<(), DomainError>;
    async fn list_branches(&self, slug: String) -> Result<Vec<String>, DomainError>;
    async fn get_head(&self, slug: String, branch: String) -> Result<String, DomainError>;
    async fn commit_file(
        &self,
        slug: String,
        branch: String,
        path: String,
        content: String,
        message: String,
        author: String,
    ) -> Result<String, DomainError>;
    async fn read_file(
        &self,
        slug: String,
        branch: String,
        path: String,
    ) -> Result<Option<String>, DomainError>;
    async fn list_files(&self, slug: String, branch: String) -> Result<Vec<String>, DomainError>;
    async fn log(
        &self,
        slug: String,
        branch: String,
        limit: i64,
    ) -> Result<serde_json::Value, DomainError>;
    async fn merge(
        &self,
        slug: String,
        source: String,
        target: String,
        message: String,
        author: String,
    ) -> Result<String, DomainError>;
    async fn diff_files(
        &self,
        slug: String,
        base_ref: String,
        head_ref: String,
    ) -> Result<serde_json::Value, DomainError>;
    async fn can_merge(
        &self,
        slug: String,
        source: String,
        target: String,
    ) -> Result<serde_json::Value, DomainError>;
}
