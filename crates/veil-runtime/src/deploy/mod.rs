//! Deploy pipeline — Terraform + Build + Deploy lifecycle.
//!
//! All execution happens within the runtime container. No external CI.
//! Steps: infrastructure (terraform) → build (rust/frontend) → deploy (lambda/s3).
//!
//! This module is the runtime-managed pipeline. The existing `crates/deploy`
//! crate handles unit-level reconciliation; this handles the full pipeline
//! lifecycle including terraform, code generation, compilation, and deployment.

#![allow(dead_code)]

pub mod build_frontend;
pub mod build_rust;
pub mod config;
pub mod deploy_frontend;
pub mod deploy_lambda;
pub mod drift;
pub mod gates;
pub mod pipeline;
pub mod terraform;
pub mod types;

pub use pipeline::PipelineState;
pub use types::*;
