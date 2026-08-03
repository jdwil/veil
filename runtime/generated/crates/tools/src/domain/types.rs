//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// ValueObject: CreateRepoArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateRepoArgs {
    pub name: String,
    pub description: Option<String>,
}

impl CreateRepoArgs {
    pub fn new(name: String) -> Self {
        Self {
            name,
            description: None,
        }
    }
}

/// ValueObject: WriteFileArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteFileArgs {
    pub repo_id: String,
    pub branch: String,
    pub path: String,
    pub content: String,
    pub message: String,
}

impl WriteFileArgs {
    pub fn new(
        repo_id: String,
        branch: String,
        path: String,
        content: String,
        message: String,
    ) -> Self {
        Self {
            repo_id,
            branch,
            path,
            content,
            message,
        }
    }
}

/// ValueObject: ReadFileArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadFileArgs {
    pub repo_id: String,
    pub branch: String,
    pub path: String,
}

impl ReadFileArgs {
    pub fn new(repo_id: String, branch: String, path: String) -> Self {
        Self {
            repo_id,
            branch,
            path,
        }
    }
}

/// ValueObject: ListFilesArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListFilesArgs {
    pub repo_id: String,
    pub branch: String,
    pub prefix: Option<String>,
}

impl ListFilesArgs {
    pub fn new(repo_id: String, branch: String) -> Self {
        Self {
            repo_id,
            branch,
            prefix: None,
        }
    }
}

/// ValueObject: CreateBranchArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateBranchArgs {
    pub repo_id: String,
    pub name: String,
    pub from_ref: String,
}

impl CreateBranchArgs {
    pub fn new(repo_id: String, name: String, from_ref: String) -> Self {
        Self {
            repo_id,
            name,
            from_ref,
        }
    }
}

/// ValueObject: ListBranchesArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListBranchesArgs {
    pub repo_id: String,
}

impl ListBranchesArgs {
    pub fn new(repo_id: String) -> Self {
        Self { repo_id }
    }
}

impl From<ListBranchesArgs> for String {
    fn from(v: ListBranchesArgs) -> String {
        v.repo_id
    }
}

impl From<String> for ListBranchesArgs {
    fn from(s: String) -> Self {
        Self { repo_id: s }
    }
}

/// ValueObject: DiffArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffArgs {
    pub repo_id: String,
    pub from_ref: String,
    pub to_ref: String,
}

impl DiffArgs {
    pub fn new(repo_id: String, from_ref: String, to_ref: String) -> Self {
        Self {
            repo_id,
            from_ref,
            to_ref,
        }
    }
}

/// ValueObject: CompileArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompileArgs {
    pub repo_id: String,
    pub branch: String,
    pub target: String,
}

impl CompileArgs {
    pub fn new(repo_id: String, branch: String, target: String) -> Self {
        Self {
            repo_id,
            branch,
            target,
        }
    }
}

/// ValueObject: DeployArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployArgs {
    pub artifact_id: String,
    pub target: String,
    pub tag: Option<String>,
}

impl DeployArgs {
    pub fn new(artifact_id: String, target: String) -> Self {
        Self {
            artifact_id,
            target,
            tag: None,
        }
    }
}

/// ValueObject: ListReposArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListReposArgs {}

/// ValueObject: LogArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogArgs {
    pub repo_id: String,
    pub branch: Option<String>,
    pub limit: Option<i64>,
}

impl LogArgs {
    pub fn new(repo_id: String) -> Self {
        Self {
            repo_id,
            branch: None,
            limit: None,
        }
    }
}

/// ValueObject: ValidateReactionPaletteArgs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateReactionPaletteArgs {
    pub node_kinds: Vec<String>,
}

impl ValidateReactionPaletteArgs {
    pub fn new() -> Self {
        Self {
            node_kinds: Vec::new(),
        }
    }
}

impl Default for ValidateReactionPaletteArgs {
    fn default() -> Self {
        Self::new()
    }
}
