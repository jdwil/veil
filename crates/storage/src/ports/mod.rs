//! Trait definitions (async traits).

#![allow(unused_imports)]

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::types::*;
pub use veil_shared::*;
pub use veil_shared::{DomainError, ValidationError};

/// Port: ObjectStorage
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    async fn put(&self, key: String, data: Vec<u8>) -> Result<(), DomainError>;
    async fn get(&self, key: String) -> Result<Vec<u8>, DomainError>;
    async fn delete(&self, key: String) -> Result<(), DomainError>;
    async fn exists(&self, key: String) -> Result<bool, DomainError>;
    async fn list(&self, prefix: String) -> Result<Vec<String>, DomainError>;
    async fn size(&self, key: String) -> Result<i64, DomainError>;
}

/// Port: MetadataStore
#[async_trait]
pub trait MetadataStore: Send + Sync {
    async fn create_repo(&self, metadata: Repo) -> Result<(), DomainError>;
    async fn get_repo(&self, id: RepoId) -> Result<Repo, DomainError>;
    async fn list_repos(&self) -> Result<Vec<Repo>, DomainError>;
    async fn update_repo(&self, metadata: Repo) -> Result<(), DomainError>;
    async fn delete_repo(&self, id: RepoId) -> Result<(), DomainError>;
    async fn put_branch(&self, repo_id: RepoId, branch: BranchInfo) -> Result<(), DomainError>;
    async fn get_branch(&self, repo_id: RepoId, name: String) -> Result<BranchInfo, DomainError>;
    async fn list_branches(&self, repo_id: RepoId) -> Result<Vec<BranchInfo>, DomainError>;
    async fn delete_branch(&self, repo_id: RepoId, name: String) -> Result<(), DomainError>;
    async fn put_tag(&self, repo_id: RepoId, tag: TagInfo) -> Result<(), DomainError>;
    async fn get_tag(&self, repo_id: RepoId, name: String) -> Result<TagInfo, DomainError>;
    async fn list_tags(&self, repo_id: RepoId) -> Result<Vec<TagInfo>, DomainError>;
    async fn delete_tag(&self, repo_id: RepoId, name: String) -> Result<(), DomainError>;
    async fn put_commit(&self, repo_id: RepoId, commit: CommitInfo) -> Result<(), DomainError>;
    async fn list_commits(
        &self,
        repo_id: RepoId,
        branch: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CommitInfo>, DomainError>;
    async fn file_history(
        &self,
        repo_id: RepoId,
        path: String,
        limit: i64,
    ) -> Result<Vec<CommitInfo>, DomainError>;
    async fn put_artifact(&self, artifact: ArtifactMetadata) -> Result<(), DomainError>;
    async fn get_artifact(&self, id: ArtifactId) -> Result<ArtifactMetadata, DomainError>;
    async fn find_artifact_by_hash(
        &self,
        content_hash: String,
        target: CompilationTarget,
    ) -> Result<Option<ArtifactMetadata>, DomainError>;
    async fn list_artifacts(
        &self,
        repo_id: RepoId,
        branch: Option<String>,
    ) -> Result<Vec<ArtifactMetadata>, DomainError>;
    async fn put_deployment(&self, record: DeploymentRecord) -> Result<(), DomainError>;
    async fn list_deployments(
        &self,
        artifact_id: ArtifactId,
    ) -> Result<Vec<DeploymentRecord>, DomainError>;
    async fn put_layer(&self, layer: LayerMetadata) -> Result<(), DomainError>;
    async fn get_layer(&self, name: String) -> Result<LayerMetadata, DomainError>;
    async fn list_layers(&self) -> Result<Vec<LayerMetadata>, DomainError>;
    async fn put_stub(&self, stub: StubMetadata) -> Result<(), DomainError>;
    async fn get_stub(&self, crate_name: String) -> Result<StubMetadata, DomainError>;
    async fn list_stubs(&self) -> Result<Vec<StubMetadata>, DomainError>;
    async fn put_dependency(&self, edge: DependencyEdge) -> Result<(), DomainError>;
    async fn get_dependencies(&self, repo_id: RepoId) -> Result<Vec<DependencyEdge>, DomainError>;
    async fn get_dependents(&self, dependency: String) -> Result<Vec<DependencyEdge>, DomainError>;
}
