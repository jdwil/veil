//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: ExtensionScope
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ExtensionScope {
    #[default]
    Platform,
    Product,
    Tenant,
}

/// enum: ExtensionKind
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ExtensionKind {
    #[default]
    Reaction,
    Signal,
    Activation,
    UiPanel,
}

/// enum: ExtensionProvenance
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ExtensionProvenance {
    #[default]
    Stock,
    Custom,
}

/// enum: ExtensionRunStatus
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ExtensionRunStatus {
    #[default]
    Succeeded,
    Skipped,
    Failed,
}

/// ValueObject: ExtensionLineage
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionLineage {
    pub extension_id: Uuid,
    pub version: i64,
}

impl ExtensionLineage {
    pub fn new(extension_id: Uuid) -> Self {
        Self {
            extension_id,
            version: 0,
        }
    }
}

/// ValueObject: ExtensionRecord
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionRecord {
    pub extension_id: Uuid,
    pub scope: ExtensionScope,
    pub product_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub initiative_id: Option<Uuid>,
    pub kind: ExtensionKind,
    pub provenance: ExtensionProvenance,
    pub name: String,
    pub description: Option<String>,
    pub current_version: i64,
    pub params_schema: Option<serde_json::Value>,
    pub capabilities: Vec<String>,
    pub palette_layer_refs: Vec<String>,
    pub source_uri: String,
    pub created_from: Option<ExtensionLineage>,
    pub created_on: DateTime<Utc>,
    pub updated_on: DateTime<Utc>,
    pub archived: bool,
}

impl ExtensionRecord {
    pub fn new(
        extension_id: Uuid,
        scope: ExtensionScope,
        kind: ExtensionKind,
        provenance: ExtensionProvenance,
        name: String,
        source_uri: String,
    ) -> Self {
        Self {
            extension_id,
            scope,
            product_id: None,
            tenant_id: None,
            initiative_id: None,
            kind,
            provenance,
            name,
            description: None,
            current_version: 0,
            params_schema: None,
            capabilities: Vec::new(),
            palette_layer_refs: Vec::new(),
            source_uri,
            created_from: None,
            created_on: Utc::now(),
            updated_on: Utc::now(),
            archived: false,
        }
    }
}

/// ValueObject: ExtensionVersion
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionVersion {
    pub extension_id: Uuid,
    pub version: i64,
    pub source_commit: String,
    pub artifact_uris: serde_json::Value,
    pub published_on: DateTime<Utc>,
    pub published_by: Option<Uuid>,
    pub changelog: Option<String>,
}

impl ExtensionVersion {
    pub fn new(extension_id: Uuid, source_commit: String, published_on: DateTime<Utc>) -> Self {
        Self {
            extension_id,
            version: 0,
            source_commit,
            artifact_uris: serde_json::json!({}),
            published_on,
            published_by: None,
            changelog: None,
        }
    }
}

/// ValueObject: ExtensionInvokeResult
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionInvokeResult {
    pub status: ExtensionRunStatus,
    pub message: Option<String>,
    pub outputs: serde_json::Value,
}

impl ExtensionInvokeResult {
    pub fn new(status: ExtensionRunStatus) -> Self {
        Self {
            status,
            message: None,
            outputs: serde_json::json!({}),
        }
    }
}

/// ValueObject: ExtensionInvokeRequest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionInvokeRequest {
    pub extension_id: Uuid,
    pub version: i64,
    pub kind: String,
    pub params: serde_json::Value,
    pub context: serde_json::Value,
}

impl ExtensionInvokeRequest {
    pub fn new(extension_id: Uuid, kind: String) -> Self {
        Self {
            extension_id,
            version: 0,
            kind,
            params: serde_json::json!({}),
            context: serde_json::json!({}),
        }
    }
}

/// ValueObject: UiMountRequest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiMountRequest {
    pub extension_id: Uuid,
    pub version: i64,
    pub slot: String,
    pub props: serde_json::Value,
}

impl UiMountRequest {
    pub fn new(extension_id: Uuid, slot: String) -> Self {
        Self {
            extension_id,
            version: 0,
            slot,
            props: serde_json::json!({}),
        }
    }
}

/// ValueObject: UiMountHandle
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiMountHandle {
    pub mount_id: String,
    pub asset_uri: String,
}

impl UiMountHandle {
    pub fn new(mount_id: String, asset_uri: String) -> Self {
        Self {
            mount_id,
            asset_uri,
        }
    }
}
