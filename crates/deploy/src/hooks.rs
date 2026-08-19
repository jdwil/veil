//! Generic pre-deploy hook runner (INV-001 `role:deploy_hook`).
//!
//! Walks `[dependencies]` (deps first), and for each project that declares
//! hooks, ensures `veil_hooks` is built and runs it with the **consumer**
//! DeployContext. No SNS/SQS/Bus strings here.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use veil_ir::{
    collect_construct_inventory, collect_deploy_hooks, find_project_root, load_package_entry,
    resolve_dependency_graph, LayerRegistry,
};

/// Result of running the hook graph for a consumer project.
#[derive(Debug, Clone)]
pub struct HookRunReport {
    pub detail: String,
    pub ran: usize,
    pub skipped: usize,
}

/// Build DeployContext JSON and run every hook binary in the dep graph.
/// `consumer_root` is the project being deployed. Fail closed on non-zero.
pub fn run_deploy_hooks(
    consumer_root: &Path,
    environment: &str,
    stack: &serde_json::Value,
    units: &serde_json::Value,
) -> Result<HookRunReport, String> {
    let graph = resolve_dependency_graph(consumer_root).unwrap_or_else(|_| {
        vec![consumer_root
            .canonicalize()
            .unwrap_or_else(|_| consumer_root.to_path_buf())]
    });

    let consumer_pkg = package_name(consumer_root);
    let consumer_veil = main_veil_path(consumer_root);
    let (consumer_sol, consumer_reg) = match load_solution(&consumer_veil) {
        Ok(v) => v,
        Err(e) => {
            return Ok(HookRunReport {
                detail: format!("no consumer VEIL ({e}) — skip hooks"),
                ran: 0,
                skipped: graph.len(),
            });
        }
    };
    let constructs = serde_json::to_value(collect_construct_inventory(
        &consumer_sol,
        &consumer_reg,
        &consumer_pkg,
    ))
    .unwrap_or_else(|_| json!([]));

    let service_name = stack
        .get("service")
        .and_then(|v| v.as_str())
        .or_else(|| stack.get("names").and_then(|n| n.get("base")).and_then(|v| v.as_str()))
        .unwrap_or(consumer_pkg.as_str())
        .trim_start_matches("veil-")
        .to_string();
    let resource_prefix = stack
        .get("resource_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("veil")
        .to_string();
    let stack_names = stack
        .get("names")
        .cloned()
        .or_else(|| stack.get("stack").and_then(|s| s.get("names")).cloned())
        .unwrap_or(json!({}));

    let context = json!({
        "service_name": service_name,
        "environment": environment,
        "resource_prefix": resource_prefix,
        "stack": stack_names,
        "units": units,
        "constructs": constructs,
    });

    let ctx_path = std::env::temp_dir().join(format!(
        "veil-deploy-context-{}-{}.json",
        consumer_pkg,
        std::process::id()
    ));
    std::fs::write(&ctx_path, serde_json::to_vec_pretty(&context).map_err(|e| e.to_string())?)
        .map_err(|e| format!("write deploy context: {e}"))?;

    let mut ran = 0;
    let mut skipped = 0;
    let mut details = Vec::new();

    for root in &graph {
        let veil_path = main_veil_path(root);
        let Ok((sol, reg)) = load_solution(&veil_path) else {
            skipped += 1;
            continue;
        };
        let hooks = collect_deploy_hooks(&sol, &reg);
        if hooks.is_empty() {
            skipped += 1;
            continue;
        }
        let names: Vec<&str> = hooks.iter().map(|c| c.name.as_str()).collect();
        let bin = ensure_hooks_bin(root)?;
        let status = run_hooks_bin(&bin, &ctx_path, stack_names.as_object())?;
        details.push(format!(
            "{}: {} ({})",
            package_name(root),
            names.join(","),
            status
        ));
        ran += 1;
    }

    let _ = std::fs::remove_file(&ctx_path);
    Ok(HookRunReport {
        detail: if details.is_empty() {
            "no deploy hooks in project or dependencies".into()
        } else {
            details.join("; ")
        },
        ran,
        skipped,
    })
}

/// Plan helper: list hook steps for the consumer + deps (no compile).
pub fn plan_hook_steps(consumer_root: &Path) -> Vec<serde_json::Value> {
    let graph = resolve_dependency_graph(consumer_root).unwrap_or_default();
    let mut steps = Vec::new();
    for root in &graph {
        let Ok((sol, reg)) = load_solution(&main_veil_path(root)) else {
            continue;
        };
        for hook in collect_deploy_hooks(&sol, &reg) {
            let pkg = package_name(root);
            steps.push(json!({
                "id": format!("hook:{}:{}", pkg, hook.name),
                "label": format!("Deploy hook {}::{}", pkg, hook.name),
                "phase": "hooks",
                "action": "update",
            }));
        }
    }
    steps
}

fn package_name(root: &Path) -> String {
    load_package_entry(root)
        .map(|e| e.use_name)
        .or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "project".into())
}

fn main_veil_path(root: &Path) -> PathBuf {
    if let Some(entry) = load_package_entry(root) {
        let p = root.join(entry.veil);
        if p.is_file() {
            return p;
        }
    }
    let fallback = root.join("main.veil");
    if fallback.is_file() {
        return fallback;
    }
    root.join("main.veil")
}

fn load_solution(
    veil_path: &Path,
) -> Result<(veil_ir::ast::Solution, LayerRegistry), String> {
    if !veil_path.is_file() {
        return Err(format!("missing {}", veil_path.display()));
    }
    let registry = LayerRegistry::for_veil_file(veil_path)?;
    let src = std::fs::read_to_string(veil_path)
        .map_err(|e| format!("read {}: {e}", veil_path.display()))?;
    let tokens = veil_parser::lex(&src);
    let sol = veil_parser::parse_with_registry(&tokens, registry.clone()).map_err(|errs| {
        errs.into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let _ = find_project_root(veil_path);
    Ok((sol, registry))
}

fn ensure_hooks_bin(root: &Path) -> Result<PathBuf, String> {
    let backend = root.join("generated/backend");
    let bin = backend.join("target/release/veil_hooks");
    if bin.is_file() {
        return Ok(bin);
    }
    let main = main_veil_path(root);
    if !main.is_file() {
        return Err(format!("no main.veil under {}", root.display()));
    }
    let veil = std::env::var("VEIL_BIN").unwrap_or_else(|_| "veil".into());
    let gen_out = Command::new(&veil)
        .args(["gen", "main.veil", "-o", "generated/backend", "-t", "rust"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("veil gen spawn: {e}"))?;
    if !gen_out.status.success() {
        let err = String::from_utf8_lossy(&gen_out.stderr);
        return Err(format!("veil gen failed: {}", tail(&err, 800)));
    }
    let build = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "veil_hooks",
            "--manifest-path",
            "generated/backend/Cargo.toml",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo build veil_hooks spawn: {e}"))?;
    if !build.status.success() {
        let err = String::from_utf8_lossy(&build.stderr);
        return Err(format!("cargo build -p veil_hooks failed: {}", tail(&err, 1200)));
    }
    if !bin.is_file() {
        return Err(format!("veil_hooks binary missing at {}", bin.display()));
    }
    Ok(bin)
}

fn run_hooks_bin(
    bin: &Path,
    ctx_path: &Path,
    stack_names: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.env("VEIL_DEPLOY_CONTEXT", ctx_path);
    if let Some(names) = stack_names {
        if let Ok(s) = serde_json::to_string(&serde_json::Value::Object(names.clone())) {
            cmd.env("VEIL_STACK_JSON", s);
        }
    }
    // AWS_* inherited from the provisioner process.
    let out = cmd
        .output()
        .map_err(|e| format!("veil_hooks spawn: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!(
            "veil_hooks exited {}: {}",
            out.status,
            tail(&stderr, 1600)
        ));
    }
    Ok(tail(&stderr, 400))
}

fn tail(s: &str, n: usize) -> String {
    let t: String = s.chars().rev().take(n).collect();
    t.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_hook_steps_empty_without_veil() {
        let dir = std::env::temp_dir().join(format!("veil-hook-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("veil.toml"), "name = \"empty\"\n").unwrap();
        let steps = plan_hook_steps(&dir);
        assert!(steps.is_empty(), "{steps:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
