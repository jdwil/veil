//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: CompilationTarget
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum CompilationTarget {
    #[default]
    LinuxX86_64,
    LinuxAarch64,
}

/// enum: DeployTarget
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeployTarget {
    Lambda {
        function_name: String,
        region: String,
    },
    Container {
        repository_uri: String,
        tag: String,
    },
}

/// enum: DeploymentStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeploymentStatus {
    InProgress,
    Succeeded,
    Failed { reason: String },
}

/// enum: ChangeType
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
}

/// enum: DependencyKind
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum DependencyKind {
    #[default]
    Layer,
    Stub,
    Repo,
}

/// ValueObject: RepoId
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoId {
    pub value: String,
}

impl RepoId {
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<RepoId> for String {
    fn from(v: RepoId) -> String {
        v.value
    }
}

impl From<String> for RepoId {
    fn from(s: String) -> Self {
        Self { value: s }
    }
}

/// ValueObject: ArtifactId
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactId {
    pub value: String,
}

impl ArtifactId {
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<ArtifactId> for String {
    fn from(v: ArtifactId) -> String {
        v.value
    }
}

impl From<String> for ArtifactId {
    fn from(s: String) -> Self {
        Self { value: s }
    }
}

/// ValueObject: LayerId
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerId {
    pub value: String,
}

impl LayerId {
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<LayerId> for String {
    fn from(v: LayerId) -> String {
        v.value
    }
}

impl From<String> for LayerId {
    fn from(s: String) -> Self {
        Self { value: s }
    }
}

/// ValueObject: StubId
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StubId {
    pub value: String,
}

impl StubId {
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

impl From<StubId> for String {
    fn from(v: StubId) -> String {
        v.value
    }
}

impl From<String> for StubId {
    fn from(s: String) -> Self {
        Self { value: s }
    }
}

/// Aggregate: Repo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repo {
    pub id: RepoId,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub default_branch: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Repo {
    pub fn new(id: RepoId, name: String, slug: String, default_branch: String) -> Self {
        Self {
            id,
            name,
            slug,
            description: None,
            default_branch,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

/// ValueObject: BranchInfo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub head_commit: String,
    pub updated_at: DateTime<Utc>,
}

impl BranchInfo {
    pub fn new(name: String, head_commit: String) -> Self {
        Self {
            name,
            head_commit,
            updated_at: Utc::now(),
        }
    }
}

/// ValueObject: TagInfo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: String,
    pub commit: String,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl TagInfo {
    pub fn new(name: String, commit: String) -> Self {
        Self {
            name,
            commit,
            message: None,
            created_at: Utc::now(),
        }
    }
}

/// ValueObject: CommitInfo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: DateTime<Utc>,
    pub parent_hashes: Vec<String>,
    pub files_changed: Vec<String>,
}

impl CommitInfo {
    pub fn new(hash: String, message: String, author: String, timestamp: DateTime<Utc>) -> Self {
        Self {
            hash,
            message,
            author,
            timestamp,
            parent_hashes: Vec::new(),
            files_changed: Vec::new(),
        }
    }
}

/// ValueObject: DiffEntry
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffEntry {
    pub path: String,
    pub change_type: ChangeType,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

impl DiffEntry {
    pub fn new(path: String, change_type: ChangeType) -> Self {
        Self {
            path,
            change_type,
            old_content: None,
            new_content: None,
        }
    }
}

/// Entity: ArtifactMetadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub id: ArtifactId,
    pub repo_id: RepoId,
    pub branch: String,
    pub commit_hash: String,
    pub content_hash: String,
    pub target: CompilationTarget,
    pub s3_key: String,
    pub size_bytes: i64,
    pub compiled_at: DateTime<Utc>,
}

impl ArtifactMetadata {
    pub fn new(
        id: ArtifactId,
        repo_id: RepoId,
        branch: String,
        commit_hash: String,
        content_hash: String,
        target: CompilationTarget,
        s3_key: String,
        compiled_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            repo_id,
            branch,
            commit_hash,
            content_hash,
            target,
            s3_key,
            size_bytes: 0,
            compiled_at,
        }
    }
}

/// ValueObject: DeploymentRecord
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeploymentRecord {
    pub artifact_id: ArtifactId,
    pub target: DeployTarget,
    pub deployed_at: DateTime<Utc>,
    pub status: DeploymentStatus,
}

impl DeploymentRecord {
    pub fn new(
        artifact_id: ArtifactId,
        target: DeployTarget,
        deployed_at: DateTime<Utc>,
        status: DeploymentStatus,
    ) -> Self {
        Self {
            artifact_id,
            target,
            deployed_at,
            status,
        }
    }
}

/// Entity: LayerMetadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerMetadata {
    pub id: LayerId,
    pub name: String,
    pub repo_id: RepoId,
    pub versions: Vec<String>,
    pub latest_version: Option<String>,
    pub registered_at: DateTime<Utc>,
}

impl LayerMetadata {
    pub fn new(id: LayerId, name: String, repo_id: RepoId, registered_at: DateTime<Utc>) -> Self {
        Self {
            id,
            name,
            repo_id,
            versions: Vec::new(),
            latest_version: None,
            registered_at,
        }
    }
}

/// Entity: StubMetadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StubMetadata {
    pub id: StubId,
    pub name: String,
    pub crate_name: String,
    pub version: String,
    pub registered_at: DateTime<Utc>,
}

impl StubMetadata {
    pub fn new(
        id: StubId,
        name: String,
        crate_name: String,
        version: String,
        registered_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            name,
            crate_name,
            version,
            registered_at,
        }
    }
}

/// ValueObject: DependencyEdge
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub dependent: RepoId,
    pub dependency: String,
    pub kind: DependencyKind,
}

impl DependencyEdge {
    pub fn new(dependent: RepoId, dependency: String, kind: DependencyKind) -> Self {
        Self {
            dependent,
            dependency,
            kind,
        }
    }
}
