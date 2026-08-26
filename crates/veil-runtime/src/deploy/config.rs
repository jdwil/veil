//! Deploy configuration loading from project veil.toml metadata.

use super::types::*;
use std::collections::HashMap;

/// Parse deploy config from a project's veil.toml `[deploy]` section.
/// Accepts a serde_json::Value representing the deploy table.
pub fn parse_deploy_config(value: &serde_json::Value) -> ProjectDeployConfig {
    let deploy_type = value
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| match s {
            "frontend" => DeployType::Frontend,
            "ecs" => DeployType::Ecs,
            "contribution" => DeployType::Contribution,
            _ => DeployType::Lambda,
        })
        .unwrap_or(DeployType::Lambda);

    let infrastructure = value.get("infrastructure").map(|v| InfraConfig {
        backend_bucket: v
            .get("backend_bucket")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        backend_key: v
            .get("backend_key")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        backend_region: v
            .get("backend_region")
            .and_then(|b| b.as_str())
            .unwrap_or("us-west-2")
            .to_string(),
    });

    let build = value.get("build").map(|v| BuildConfig {
        target: v
            .get("target")
            .and_then(|t| t.as_str())
            .map(|s| match s {
                "typescript" => BuildTarget::Typescript,
                _ => BuildTarget::Rust,
            })
            .unwrap_or(BuildTarget::Rust),
        rust_target: v
            .get("rust_target")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string()),
    });

    let artifacts = value.get("artifacts").map(|v| ArtifactConfig {
        bucket: v
            .get("bucket")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
    });

    let contribution = value.get("contribution").map(|v| ContributionConfig {
        app_id: v
            .get("app_id")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        contribution_id: v
            .get("id")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        name: v
            .get("name")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        bucket: v
            .get("bucket")
            .and_then(|s| s.as_str())
            .unwrap_or("dlx-ai-contributions")
            .to_string(),
        cdn_base_url: v
            .get("cdn_base_url")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        order: v
            .get("order")
            .and_then(|n| n.as_u64())
            .unwrap_or(100) as u32,
        slots: v.get("slots").cloned().unwrap_or(serde_json::Value::Object(Default::default())),
        entry: v
            .get("entry")
            .and_then(|s| s.as_str())
            .unwrap_or("src/index.ts")
            .to_string(),
        externals: v
            .get("externals")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec!["svelte".to_string(), "svelte/internal".to_string()]),
    });

    let mut gates = HashMap::new();
    if let Some(g) = value.get("gates").and_then(|v| v.as_object()) {
        for (env, policy) in g {
            if let Some(p) = policy.as_str() {
                gates.insert(env.clone(), GatePolicy::from_str(p));
            }
        }
    }

    ProjectDeployConfig {
        deploy_type,
        infrastructure,
        build,
        artifacts,
        contribution,
        gates,
    }
}

/// Resolve the working directory for a deploy job.
pub fn working_dir(slug: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/deploy/{}", slug))
}

/// Terraform working directory.
pub fn terraform_dir(slug: &str) -> std::path::PathBuf {
    working_dir(slug).join("terraform")
}

/// Generated source output directory.
pub fn generated_dir(slug: &str) -> std::path::PathBuf {
    working_dir(slug).join("generated")
}

/// Build output directory.
pub fn build_output_dir(slug: &str) -> std::path::PathBuf {
    working_dir(slug).join("output")
}
