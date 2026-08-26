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
pub mod contribution_store;

#[cfg(test)]
mod tests;

pub use storage::ArtifactRegistryStore;
pub use types::*;
pub use contribution_store::{
    ContributionManifest, ContributionManifestStore, CreateContributionBody, PatchContributionBody,
};
