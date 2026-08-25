//! Frontend deploy step — S3 sync + CloudFront invalidation.

use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Deploy frontend assets: sync build output to S3 and invalidate CloudFront.
pub async fn run(
    slug: &str,
    build_dir: &str,
    target_bucket: &str,
    cloudfront_distribution_id: Option<&str>,
) -> Result<DeployFrontendResult, String> {
    // Step 1: S3 sync (using AWS CLI — most reliable for recursive sync)
    let s3_target = format!("s3://{target_bucket}/");

    let sync_output = Command::new("aws")
        .args([
            "s3",
            "sync",
            build_dir,
            &s3_target,
            "--delete",
            "--no-progress",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("aws s3 sync failed to start: {e}"))?;

    if !sync_output.status.success() {
        let stderr = String::from_utf8_lossy(&sync_output.stderr);
        return Err(format!("aws s3 sync failed: {stderr}"));
    }
    info!(slug, bucket = target_bucket, "S3 sync complete");

    // Step 2: CloudFront invalidation (if distribution configured)
    let invalidation_id = if let Some(dist_id) = cloudfront_distribution_id {
        let inv_output = Command::new("aws")
            .args([
                "cloudfront",
                "create-invalidation",
                "--distribution-id",
                dist_id,
                "--paths",
                "/*",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("CloudFront invalidation failed to start: {e}"))?;

        if !inv_output.status.success() {
            let stderr = String::from_utf8_lossy(&inv_output.stderr);
            // Non-fatal: log warning but don't fail the deploy
            tracing::warn!(slug, "CloudFront invalidation failed: {stderr}");
            None
        } else {
            let stdout = String::from_utf8_lossy(&inv_output.stdout);
            // Parse invalidation ID from JSON output
            let inv_id = serde_json::from_str::<serde_json::Value>(&stdout)
                .ok()
                .and_then(|v| {
                    v.get("Invalidation")
                        .and_then(|i| i.get("Id"))
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string())
                });
            info!(slug, dist_id, inv_id = ?inv_id, "CloudFront invalidation created");
            inv_id
        }
    } else {
        None
    };

    Ok(DeployFrontendResult {
        bucket: target_bucket.to_string(),
        invalidation_id,
    })
}

#[derive(Debug, Clone)]
pub struct DeployFrontendResult {
    pub bucket: String,
    pub invalidation_id: Option<String>,
}
