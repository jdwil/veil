//! Frontend build step — veil gen TypeScript → npm install → npm run build.

use super::config;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Result of the frontend build step.
#[derive(Debug, Clone)]
pub struct FrontendBuildResult {
    /// Path to the build output directory (static files).
    pub build_dir: String,
}

/// Run the frontend build step:
/// 1. veil gen {main_veil} -t typescript -o generated/
/// 2. npm install
/// 3. npm run build
/// Output: generated/build/ or generated/dist/ directory with static files.
pub async fn run(
    slug: &str,
    veil_file: &str,
    source_dir: &Path,
    component_deps: &[super::component_deps::ComponentDep],
) -> Result<FrontendBuildResult, String> {
    let gen_dir = config::generated_dir(slug);

    tokio::fs::create_dir_all(&gen_dir)
        .await
        .map_err(|e| format!("create gen dir: {e}"))?;

    // Step 1: veil gen
    let veil_path = source_dir.join(veil_file);
    let gen_output = Command::new("veil")
        .args([
            "gen",
            &veil_path.to_string_lossy(),
            "-t",
            "typescript",
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
        return Err(format!("veil gen (typescript) failed: {stderr}"));
    }
    info!(slug, "veil gen (typescript) complete");

    // Step 1b: Materialize cross-project UI components (data-driven; no-op when
    // there are no external component deps). See deploy::component_deps.
    super::component_deps::materialize_component_deps(&gen_dir, component_deps)
        .await
        .map_err(|e| format!("materialize component deps: {e}"))?;
    if !component_deps.is_empty() {
        info!(
            slug,
            n_providers = component_deps.len(),
            "cross-project components materialized into frontend build tree"
        );
    }

    // Step 2: npm install
    let npm_install = Command::new("npm")
        .args(["install", "--prefer-offline", "--no-audit"])
        .current_dir(&gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("npm install failed to start: {e}"))?;

    if !npm_install.status.success() {
        let stderr = String::from_utf8_lossy(&npm_install.stderr);
        return Err(format!("npm install failed: {stderr}"));
    }
    info!(slug, "npm install complete");

    // Step 3: npm run build
    let npm_build = Command::new("npm")
        .args(["run", "build"])
        .current_dir(&gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("npm run build failed to start: {e}"))?;

    if !npm_build.status.success() {
        let stderr = String::from_utf8_lossy(&npm_build.stderr);
        return Err(format!("npm run build failed: {stderr}"));
    }
    info!(slug, "npm run build complete");

    // Determine build output dir: try build/, then dist/
    let build_dir = if gen_dir.join("build").exists() {
        gen_dir.join("build")
    } else if gen_dir.join("dist").exists() {
        gen_dir.join("dist")
    } else {
        return Err("No build/ or dist/ output directory found after npm run build".into());
    };

    Ok(FrontendBuildResult {
        build_dir: build_dir.to_string_lossy().to_string(),
    })
}
