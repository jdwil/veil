//! Domain types.

#![allow(unused_imports)]

use crate::domain::messages::*;
use crate::ports::{DomainError, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// enum: AuthStrategy
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AuthStrategy {
    Bus,
    External {
        provider_url: String,
        client_id: String,
    },
}

/// enum: Severity
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum Severity {
    #[default]
    Info,
    Warning,
    Error,
    Critical,
}

/// ValueObject: Dependency
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dependency {
    pub trait_name: String,
    pub adapter: Option<String>,
    pub provided_by: Option<String>,
    pub env: Vec<String>,
}

impl Dependency {
    pub fn new(trait_name: String) -> Self {
        Self {
            trait_name,
            adapter: None,
            provided_by: None,
            env: Vec::new(),
        }
    }
}

/// ValueObject: HandlerInput
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandlerInput {
    pub name: String,
    pub type_desc: String,
}

impl HandlerInput {
    pub fn new(name: String, type_desc: String) -> Self {
        Self { name, type_desc }
    }
}

/// ValueObject: HandlerDef
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandlerDef {
    pub function: String,
    pub inputs: Vec<HandlerInput>,
}

impl HandlerDef {
    pub fn new(function: String) -> Self {
        Self {
            function,
            inputs: Vec::new(),
        }
    }
}

/// ValueObject: Manifest
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub context: String,
    pub crate_name: String,
    pub deps: std::collections::HashMap<String, Dependency>,
    pub handlers: std::collections::HashMap<String, HandlerDef>,
}

impl Manifest {
    pub fn new(context: String, crate_name: String) -> Self {
        Self {
            context,
            crate_name,
            deps: std::collections::HashMap::new(),
            handlers: std::collections::HashMap::new(),
        }
    }
}

/// ValueObject: ScanFinding
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanFinding {
    pub scanner: String,
    pub severity: Severity,
    pub message: String,
    pub location: Option<String>,
    pub rule_id: Option<String>,
}

impl ScanFinding {
    pub fn new(scanner: String, severity: Severity, message: String) -> Self {
        Self {
            scanner,
            severity,
            message,
            location: None,
            rule_id: None,
        }
    }
}

/// ValueObject: ScanReport
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanReport {
    pub scanned_at: DateTime<Utc>,
    pub passed: bool,
    pub findings: Vec<ScanFinding>,
    pub summary: std::collections::HashMap<String, i64>,
}

impl ScanReport {
    pub fn new(scanned_at: DateTime<Utc>) -> Self {
        Self {
            scanned_at,
            passed: false,
            findings: Vec::new(),
            summary: std::collections::HashMap::new(),
        }
    }
}

/// ValueObject: ScanConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanConfig {
    pub fail_threshold: Severity,
    pub skip_clippy: bool,
    pub skip_audit: bool,
    pub skip_deny: bool,
    pub custom_rules: Vec<String>,
}

impl ScanConfig {
    pub fn new(fail_threshold: Severity) -> Self {
        Self {
            fail_threshold,
            skip_clippy: false,
            skip_audit: false,
            skip_deny: false,
            custom_rules: Vec::new(),
        }
    }
}

/// ValueObject: EnvConfig
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvConfig {
    pub vars: std::collections::HashMap<String, String>,
}

impl EnvConfig {
    pub fn new() -> Self {
        Self {
            vars: std::collections::HashMap::new(),
        }
    }
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self::new()
    }
}
