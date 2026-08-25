//! Terraform step — init / plan / apply.
//!
//! Fetches .tf files from project S3 source, writes to a working directory,
//! runs terraform commands, captures outputs.

use super::config;
use super::types::*;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Run the infrastructure (terraform) step.
///
/// 1. Write terraform files to working dir
/// 2. terraform init (with backend config)
/// 3. terraform plan -out=plan.tfplan
/// 4. If not dry_run: terraform apply plan.tfplan
/// 5. Capture and return outputs
pub async fn run(
    slug: &str,
    infra_config: &InfraConfig,
    tf_files: &[(String, Vec<u8>)],
    dry_run: bool,
) -> Result<TerraformResult, String> {
    let tf_dir = config::terraform_dir(slug);

    // Ensure working directory exists
    tokio::fs::create_dir_all(&tf_dir)
        .await
        .map_err(|e| format!("Failed to create terraform dir: {e}"))?;

    // Write terraform files
    for (filename, content) in tf_files {
        let path = tf_dir.join(filename);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| format!("Failed to write {filename}: {e}"))?;
    }

    // terraform init
    let _init_output = terraform_init(&tf_dir, infra_config).await?;
    info!(slug, "terraform init complete");

    // terraform plan
    let plan_output = terraform_plan(&tf_dir).await?;
    info!(slug, changes = plan_output.has_changes, "terraform plan complete");

    if dry_run || !plan_output.has_changes {
        return Ok(TerraformResult {
            plan_output: plan_output.raw_output,
            has_changes: plan_output.has_changes,
            applied: false,
            outputs: Default::default(),
        });
    }

    // terraform apply
    let _apply_output = terraform_apply(&tf_dir).await?;
    info!(slug, "terraform apply complete");

    // Capture outputs
    let outputs = terraform_output(&tf_dir).await.unwrap_or_default();

    Ok(TerraformResult {
        plan_output: plan_output.raw_output,
        has_changes: true,
        applied: true,
        outputs,
    })
}

/// Run drift detection (plan only, never apply).
/// Returns exit code semantics: 0 = no changes, 2 = drift detected.
pub async fn detect_drift(
    slug: &str,
    infra_config: &InfraConfig,
    tf_files: &[(String, Vec<u8>)],
) -> Result<DriftStatus, String> {
    let tf_dir = config::terraform_dir(slug);

    tokio::fs::create_dir_all(&tf_dir)
        .await
        .map_err(|e| format!("Failed to create terraform dir: {e}"))?;

    for (filename, content) in tf_files {
        let path = tf_dir.join(filename);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| format!("Failed to write {filename}: {e}"))?;
    }

    terraform_init(&tf_dir, infra_config).await?;

    // Plan with -detailed-exitcode
    let output = Command::new("terraform")
        .args(["plan", "-detailed-exitcode", "-no-color"])
        .current_dir(&tf_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("terraform plan failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let exit_code = output.status.code().unwrap_or(1);

    match exit_code {
        0 => Ok(DriftStatus::in_sync()),
        2 => {
            // Count changes from plan output
            let changes = count_plan_changes(&stdout);
            Ok(DriftStatus::drifted(changes, stdout))
        }
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(format!("terraform plan failed (exit {exit_code}): {stderr}"))
        }
    }
}

// ─── Internal ───────────────────────────────────────────────────────────────

/// Result of the terraform step.
#[derive(Debug, Clone)]
pub struct TerraformResult {
    pub plan_output: String,
    pub has_changes: bool,
    pub applied: bool,
    pub outputs: std::collections::HashMap<String, String>,
}

struct PlanOutput {
    raw_output: String,
    has_changes: bool,
}

async fn terraform_init(tf_dir: &Path, config: &InfraConfig) -> Result<String, String> {
    let output = Command::new("terraform")
        .args([
            "init",
            "-no-color",
            &format!("-backend-config=bucket={}", config.backend_bucket),
            &format!("-backend-config=key={}", config.backend_key),
            &format!("-backend-config=region={}", config.backend_region),
        ])
        .current_dir(tf_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("terraform init failed to start: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("terraform init failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn terraform_plan(tf_dir: &Path) -> Result<PlanOutput, String> {
    let output = Command::new("terraform")
        .args(["plan", "-out=plan.tfplan", "-detailed-exitcode", "-no-color"])
        .current_dir(tf_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("terraform plan failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let exit_code = output.status.code().unwrap_or(1);

    match exit_code {
        0 => Ok(PlanOutput {
            raw_output: stdout,
            has_changes: false,
        }),
        2 => Ok(PlanOutput {
            raw_output: stdout,
            has_changes: true,
        }),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("terraform plan failed (exit {exit_code}): {stderr}"))
        }
    }
}

async fn terraform_apply(tf_dir: &Path) -> Result<String, String> {
    let output = Command::new("terraform")
        .args(["apply", "-no-color", "-auto-approve", "plan.tfplan"])
        .current_dir(tf_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("terraform apply failed to start: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("terraform apply failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn terraform_output(
    tf_dir: &Path,
) -> Result<std::collections::HashMap<String, String>, String> {
    let output = Command::new("terraform")
        .args(["output", "-json", "-no-color"])
        .current_dir(tf_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("terraform output failed: {e}"))?;

    if !output.status.success() {
        return Ok(Default::default());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|e| format!("parse terraform output: {e}"))?;

    let mut outputs = std::collections::HashMap::new();
    if let Some(obj) = parsed.as_object() {
        for (key, val) in obj {
            if let Some(v) = val.get("value").and_then(|v| v.as_str()) {
                outputs.insert(key.clone(), v.to_string());
            }
        }
    }

    Ok(outputs)
}

/// Count the number of resource changes in terraform plan output.
fn count_plan_changes(output: &str) -> usize {
    // Terraform outputs "Plan: X to add, Y to change, Z to destroy."
    for line in output.lines() {
        if line.starts_with("Plan:") {
            let numbers: Vec<usize> = line
                .split_whitespace()
                .filter_map(|w| w.parse::<usize>().ok())
                .collect();
            return numbers.iter().sum();
        }
    }
    // Fallback: count lines with +/- resource markers
    output
        .lines()
        .filter(|l| {
            l.trim_start().starts_with("+ ")
                || l.trim_start().starts_with("~ ")
                || l.trim_start().starts_with("- ")
        })
        .count()
}
