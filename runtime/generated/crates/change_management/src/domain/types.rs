//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: PrStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum PrStatus {
    #[default]
    Draft,
    ReadyForReview,
    Approved,
    ChangesRequested,
    Merging,
    Merged,
    Rejected,
    Closed,
}

/// enum: CiStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum CiStatus {
    #[default]
    Pending,
    Running,
    Passed,
    Failed,
    Skipped,
}

/// enum: DiffKind
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum DiffKind {
    #[default]
    Added,
    Modified,
    Removed,
    Moved,
}

/// ValueObject: StructuralChange
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralChange {
    pub kind: DiffKind,
    pub path: String,
    pub detail: String,
    pub construct_type: Option<String>,
}

impl StructuralChange {
    pub fn new(kind: DiffKind, path: String, detail: String) -> Self {
        Self {
            kind,
            path,
            detail,
            construct_type: None,
        }
    }
}

/// ValueObject: DiffSummary
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub description: String,
    pub changes: Vec<StructuralChange>,
    pub files_changed: i64,
    pub additions: i64,
    pub removals: i64,
}

impl DiffSummary {
    pub fn new(description: String) -> Self {
        Self {
            description,
            changes: Vec::new(),
            files_changed: 0,
            additions: 0,
            removals: 0,
        }
    }
}

/// ValueObject: ReviewComment
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: Uuid,
    pub pr_id: Uuid,
    pub author: String,
    pub construct_path: Option<String>,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub resolved: bool,
}

impl ReviewComment {
    pub fn new(id: Uuid, pr_id: Uuid, author: String, body: String) -> Self {
        Self {
            id,
            pr_id,
            author,
            construct_path: None,
            body,
            created_at: Utc::now(),
            resolved: false,
        }
    }
}

/// ValueObject: Approval
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub pr_id: Uuid,
    pub reviewer: String,
    pub approved_at: DateTime<Utc>,
    pub comment: Option<String>,
}

impl Approval {
    pub fn new(pr_id: Uuid, reviewer: String, approved_at: DateTime<Utc>) -> Self {
        Self {
            pr_id,
            reviewer,
            approved_at,
            comment: None,
        }
    }
}

/// ValueObject: CiRun
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiRun {
    pub id: Uuid,
    pub pr_id: Uuid,
    pub commit_hash: String,
    pub status: CiStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub logs_key: Option<String>,
    pub error_summary: Option<String>,
}

impl CiRun {
    pub fn new(
        id: Uuid,
        pr_id: Uuid,
        commit_hash: String,
        status: CiStatus,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            pr_id,
            commit_hash,
            status,
            started_at,
            completed_at: None,
            duration_ms: None,
            logs_key: None,
            error_summary: None,
        }
    }
}

/// Aggregate: PullRequest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub id: Uuid,
    pub repo_id: Uuid,
    pub title: String,
    pub description: String,
    pub jira_ticket: String,
    pub source_branch: String,
    pub target_branch: String,
    pub author: String,
    pub status: PrStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    pub merged_by: Option<String>,
    pub merge_commit: Option<String>,
}

impl PullRequest {
    pub fn new(
        id: Uuid,
        repo_id: Uuid,
        title: String,
        description: String,
        jira_ticket: String,
        source_branch: String,
        target_branch: String,
        author: String,
        status: PrStatus,
    ) -> Self {
        Self {
            id,
            repo_id,
            title,
            description,
            jira_ticket,
            source_branch,
            target_branch,
            author,
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            merged_at: None,
            merged_by: None,
            merge_commit: None,
        }
    }
}
