//! Contribution build step — veil gen TypeScript → generate vite.config.ts (library mode) →
//! npm install → vite build → single ES module bundle (dist/index.js + optional dist/style.css).
//!
//! Unlike `build_frontend.rs` which builds a full SPA, this builds a **library bundle**:
//! - Entry: src/index.ts (re-exports named Svelte components)
//! - Output: dist/index.js (single ES module) + dist/style.css (optional)
//! - Externals: svelte, svelte/internal (provided by the harness at runtime)
//! - Format: ES module (import/export, not IIFE or CJS)

use super::config;
use super::types::ContributionConfig;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

/// Result of the contribution build step.
#[derive(Debug, Clone)]
pub struct ContributionBuildResult {
    /// Path to the output JS bundle (dist/index.js).
    pub bundle_path: String,
    /// Path to the output CSS file, if generated (dist/style.css).
    pub css_path: Option<String>,
    /// SHA256 hash of the bundle for cache-busting.
    pub bundle_hash: String,
}

/// Generate the vite.config.ts for library mode.
/// This is written into the generated directory before npm install / vite build.
fn generate_vite_library_config(contribution: &ContributionConfig) -> String {
    let externals_json: Vec<String> = contribution
        .externals
        .iter()
        .map(|e| format!("'{}'", e))
        .collect();
    let externals_str = externals_json.join(", ");

    format!(
        r#"import {{ defineConfig }} from 'vite';
import {{ svelte }} from '@sveltejs/vite-plugin-svelte';
import path from 'path';

export default defineConfig({{
  plugins: [
    svelte({{
      compilerOptions: {{
        // Library mode: components are compiled but not mounted
        css: 'external',
      }},
    }}),
  ],
  build: {{
    lib: {{
      entry: path.resolve(__dirname, '{entry}'),
      formats: ['es'],
      fileName: () => 'index.js',
    }},
    rollupOptions: {{
      external: [{externals}],
      output: {{
        // Single chunk — no code splitting for contribution bundles
        inlineDynamicImports: true,
        assetFileNames: (assetInfo) => {{
          if (assetInfo.name && assetInfo.name.endsWith('.css')) {{
            return 'style.css';
          }}
          return assetInfo.name || 'asset';
        }},
      }},
    }},
    outDir: 'dist',
    emptyOutDir: true,
    // Produce clean ES module output
    target: 'es2022',
    minify: 'esbuild',
    sourcemap: false,
  }},
}});
"#,
        entry = contribution.entry,
        externals = externals_str,
    )
}

/// Generate a package.json suitable for library-mode contribution builds.
fn generate_contribution_package_json(contribution: &ContributionConfig) -> String {
    format!(
        r#"{{
  "name": "{id}",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {{
    "build": "vite build"
  }},
  "devDependencies": {{
    "@sveltejs/vite-plugin-svelte": "^5.0.0",
    "svelte": "^5.34.0",
    "typescript": "^5.8.0",
    "vite": "^6.3.0"
  }}
}}
"#,
        id = contribution.contribution_id,
    )
}

/// Generate svelte.config.js for the library build (minimal, no adapter needed).
fn generate_svelte_config() -> &'static str {
    r#"import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  preprocess: vitePreprocess(),
};
"#
}

/// Run the contribution build pipeline:
/// 1. veil gen {main_veil} -t typescript -o generated/
/// 2. Write vite.config.ts (library mode) + package.json + svelte.config.js
/// 3. npm install
/// 4. vite build → dist/index.js + optional dist/style.css
pub async fn run(
    slug: &str,
    veil_file: &str,
    source_dir: &Path,
    contribution: &ContributionConfig,
) -> Result<ContributionBuildResult, String> {
    let gen_dir = config::generated_dir(slug);

    tokio::fs::create_dir_all(&gen_dir)
        .await
        .map_err(|e| format!("create gen dir: {e}"))?;

    // Step 1: veil gen (produces TypeScript/Svelte source into generated/)
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
    info!(slug, "veil gen (typescript) complete for contribution build");

    // Step 2: Write build config files (overwrite any codegen-produced ones)
    let vite_config = generate_vite_library_config(contribution);
    tokio::fs::write(gen_dir.join("vite.config.ts"), &vite_config)
        .await
        .map_err(|e| format!("write vite.config.ts: {e}"))?;

    let package_json = generate_contribution_package_json(contribution);
    tokio::fs::write(gen_dir.join("package.json"), &package_json)
        .await
        .map_err(|e| format!("write package.json: {e}"))?;

    let svelte_config = generate_svelte_config();
    tokio::fs::write(gen_dir.join("svelte.config.js"), svelte_config)
        .await
        .map_err(|e| format!("write svelte.config.js: {e}"))?;

    info!(slug, "contribution build config written (vite library mode)");

    // Step 3: npm install
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
    info!(slug, "npm install complete (contribution)");

    // Step 4: vite build (library mode via config)
    let vite_build = Command::new("npx")
        .args(["vite", "build"])
        .current_dir(&gen_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("vite build failed to start: {e}"))?;

    if !vite_build.status.success() {
        let stderr = String::from_utf8_lossy(&vite_build.stderr);
        return Err(format!("vite build (library mode) failed: {stderr}"));
    }
    info!(slug, "vite build (library mode) complete");

    // Verify output
    let dist_dir = gen_dir.join("dist");
    let bundle_path = dist_dir.join("index.js");
    if !bundle_path.exists() {
        return Err("dist/index.js not found after vite build — library entry may be misconfigured".into());
    }

    let css_path = dist_dir.join("style.css");
    let css_exists = css_path.exists();

    // Compute SHA256 hash of the bundle
    let bundle_hash = compute_sha256(&bundle_path).await?;

    Ok(ContributionBuildResult {
        bundle_path: bundle_path.to_string_lossy().to_string(),
        css_path: if css_exists {
            Some(css_path.to_string_lossy().to_string())
        } else {
            None
        },
        bundle_hash,
    })
}

/// Compute SHA256 hash of a file using the system sha256sum binary.
async fn compute_sha256(path: &Path) -> Result<String, String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("sha256sum failed: {e}"))?;

    if !output.status.success() {
        return Err("sha256sum returned non-zero".into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = stdout
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_string();

    Ok(hash)
}
