//! Artifact Registry — Phase 1 of Platform Primitives.
//!
//! Versioned, signed-off packages with contribution metadata.
//! DynamoDB for registry metadata (same VEIL_DDB_TABLE), S3 for artifact blobs
//! (same BUCKET). Resolve APIs let the SPA harness and backend host query
//! contributions by tenant + principal.

#[allow(dead_code)]
pub mod storage;
#[allow(dead_code)]
pub mod types;

pub use storage::ArtifactRegistryStore;
pub use types::*;
