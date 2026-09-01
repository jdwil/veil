//! Rust build step — veil gen → cargo build → zip packaging for Lambda.

use super::config;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Result of the Rust build step.
#[derive(Debug, Clone)]
pub struct RustBuildResult {
    /// Path to the final artifact (e.g. lambda.zip).
    pub artifact_path: String,
    /// SHA256 hash of the artifact.
    pub artifact_hash: String,
}

/// Run the Rust build step:
/// 1. veil gen {main_veil} -t rust -o generated/
/// 2. cargo build --release --target {target}
/// 3. Package binary into lambda.zip
pub async fn run(
    slug: &str,
    veil_file: &str,
    source_dir: &Path,
    rust_target: &str,
) -> Result<RustBuildResult, String> {
    let gen_dir = config::generated_dir(slug);
    let output_dir = config::build_output_dir(slug);

    tokio::fs::create_dir_all(&gen_dir)
        .await
        .map_err(|e| format!("create gen dir: {e}"))?;
    tokio::fs::create_dir_all(&output_dir)
        .await
        .map_err(|e| format!("create output dir: {e}"))?;

    // Step 1: veil gen
    let veil_path = source_dir.join(veil_file);
    let gen_output = Command::new("veil")
        .args([
            "gen",
            &veil_path.to_string_lossy(),
            "-t",
            "rust",
            "-o",
            &gen_dir.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("veil gen failed to start: {e}"))?;

    if !gen_output.status.success() {
        let stderr = String::from_utf8_lossy(&gen_output.stderr);
        return Err(format!("veil gen failed: {stderr}"));
    }
    info!(slug, "veil gen (rust) complete");

    // Step 2: cargo build
    let build_output = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--target",
            rust_target,
        ])
        .current_dir(&gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("cargo build failed to start: {e}"))?;

    if !build_output.status.success() {
        let stderr = String::from_utf8_lossy(&build_output.stderr);
        return Err(format!("cargo build failed: {stderr}"));
    }
    info!(slug, "cargo build complete");

    // Step 3: Package into lambda.zip
    // The rust codegen emits the Lambda entrypoint as the `veil_bin` binary
    // (crates/veil_bin — see rust/workspace.rs). AWS custom runtimes require the
    // packaged binary to be named `bootstrap`, so we locate the produced binary
    // (preferring an explicit `bootstrap` if a future codegen emits one, else the
    // current `veil_bin`) and copy it to `bootstrap` inside the zip.
    let release_dir = gen_dir
        .join("target")
        .join(rust_target)
        .join("release");
    let binary_path = {
        let bootstrap_bin = release_dir.join("bootstrap");
        let veil_bin = release_dir.join("veil_bin");
        if bootstrap_bin.exists() {
            bootstrap_bin
        } else if veil_bin.exists() {
            veil_bin
        } else {
            return Err(format!(
                "no lambda binary found in {} (looked for 'bootstrap' and 'veil_bin')",
                release_dir.display()
            ));
        }
    };
    let bootstrap_dest = output_dir.join("bootstrap");
    let zip_path = output_dir.join("lambda.zip");

    tokio::fs::copy(&binary_path, &bootstrap_dest)
        .await
        .map_err(|e| format!("copy binary: {e}"))?;

    let zip_output = Command::new("zip")
        .args(["-j", &zip_path.to_string_lossy(), &bootstrap_dest.to_string_lossy()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("zip failed to start: {e}"))?;

    if !zip_output.status.success() {
        let stderr = String::from_utf8_lossy(&zip_output.stderr);
        return Err(format!("zip failed: {stderr}"));
    }
    info!(slug, "lambda.zip packaged");

    // Compute hash
    let artifact_hash = compute_sha256(&zip_path).await?;

    Ok(RustBuildResult {
        artifact_path: zip_path.to_string_lossy().to_string(),
        artifact_hash,
    })
}

async fn compute_sha256(path: &Path) -> Result<String, String> {
    // Use system sha256sum binary for a proper cryptographic hash.
    let output = Command::new("sha256sum")
        .arg(path.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("sha256sum failed: {e}"))?;

    if !output.status.success() {
        return Err("sha256sum returned non-zero exit code".into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| "sha256sum produced empty output".into())
}
