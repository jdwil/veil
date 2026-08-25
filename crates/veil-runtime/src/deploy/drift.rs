//! Drift detection — periodic or on-demand terraform plan to detect unapplied changes.
//!
//! Non-destructive: plan only, never auto-apply.

use super::terraform;
use super::types::*;
use tracing::{info, warn};

/// Run drift detection for a project.
/// Returns DriftStatus indicating whether infrastructure has drifted.
pub async fn check_drift(
    slug: &str,
    infra_config: &InfraConfig,
    tf_files: &[(String, Vec<u8>)],
) -> Result<DriftStatus, String> {
    info!(slug, "starting drift detection");

    let result = terraform::detect_drift(slug, infra_config, tf_files).await?;

    if result.detected {
        warn!(
            slug,
            changes = result.changes,
            "infrastructure drift detected"
        );
    } else {
        info!(slug, "no drift detected — infrastructure in sync");
    }

    Ok(result)
}
