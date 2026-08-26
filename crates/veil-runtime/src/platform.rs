//! Local FS/git ports, compile, and artifact/layer listing for ProductHost.
//! Platform domain HTTP is in `platform_http` (REST), not a message bus.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

// ─── CAP-004: git port (local defaults) ─────────────────────────────────────

/// Injectable git port.
pub trait GitRepo: Send + Sync {
    fn branches(&self, repo: &Path) -> Result<Vec<String>, String>;
    fn log(&self, repo: &Path, limit: usize) -> Result<Vec<String>, String>;
}

/// Git via CLI (local).
pub struct LocalGit;

impl GitRepo for LocalGit {
    fn branches(&self, repo: &Path) -> Result<Vec<String>, String> {
        let out = Command::new("git")
            .args(["-C", &repo.to_string_lossy(), "branch", "--format=%(refname:short)"])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    fn log(&self, repo: &Path, limit: usize) -> Result<Vec<String>, String> {
        let out = Command::new("git")
            .args([
                "-C",
                &repo.to_string_lossy(),
                "log",
                &format!("-{limit}"),
                "--oneline",
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect())
    }
}

pub fn projects_dir() -> PathBuf {
    veil_server::ensure_projects_dir_exists()
        .unwrap_or_else(|_| veil_server::default_projects_dir())
}

pub fn project_root(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains("..") || name.contains('/') {
        return Err("invalid project name".into());
    }
    let root = projects_dir().join(name);
    if !root.is_dir() {
        return Err(format!("project not found: {name}"));
    }
    Ok(root)
}

pub fn compile_project(repo: &str) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    // Prefer primary *.veil package
    let veil_bin = std::env::var("VEIL_BIN").unwrap_or_else(|_| {
        // monorepo default
        let cand = PathBuf::from("target/release/veil");
        if cand.is_file() {
            cand.to_string_lossy().to_string()
        } else {
            "veil".into()
        }
    });
    let packages: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("veil"))
        .collect();
    if packages.is_empty() {
        return json!({ "error": "no .veil packages", "hint": "veil init" });
    }
    let pkg = &packages[0];
    let out = Command::new(&veil_bin)
        .args(["check", &pkg.to_string_lossy()])
        .current_dir(&root)
        .output();
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let artifact_dir = veil_server::veil_home_dir()
                .join("artifacts")
                .join(repo);
            let _ = std::fs::create_dir_all(&artifact_dir);
            let ok = o.status.success();
            if ok {
                let _ = std::fs::write(
                    artifact_dir.join("last-check.txt"),
                    format!("ok\n{stdout}\n{stderr}"),
                );
            }
            json!({
                "ok": ok,
                "repo": repo,
                "package": pkg.file_name().and_then(|n| n.to_str()),
                "stdout": stdout,
                "stderr": stderr,
                "artifact_dir": artifact_dir.to_string_lossy(),
            })
        }
        Err(e) => json!({
            "error": e.to_string(),
            "hint": "set VEIL_BIN to path of veil CLI"
        }),
    }
}

pub fn list_artifacts(repo: Option<&str>) -> Value {
    let base = veil_server::veil_home_dir().join("artifacts");
    if !base.is_dir() {
        return json!({ "artifacts": [] });
    }
    let mut arts = Vec::new();
    if let Some(r) = repo {
        let p = base.join(r);
        if p.is_dir() {
            arts.push(json!({ "repo": r, "path": p.to_string_lossy() }));
        }
    } else if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                arts.push(json!({
                    "repo": e.file_name().to_string_lossy(),
                    "path": e.path().to_string_lossy(),
                }));
            }
        }
    }
    json!({ "artifacts": arts })
}

pub fn list_layers() -> Value {
    let mut layers = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("VEIL_LAYERS_DIR") {
        candidates.push(PathBuf::from(d));
    }
    candidates.push(PathBuf::from("layers"));
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../layers"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("layers"));
            candidates.push(parent.join("../../../layers"));
        }
    }
    // Walk ancestors of CWD for layers/
    if let Ok(mut cur) = std::env::current_dir() {
        for _ in 0..6 {
            candidates.push(cur.join("layers"));
            if !cur.pop() {
                break;
            }
        }
    }
    for dir in candidates {
        if !dir.is_dir() {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("layer") {
                    layers.push(json!({
                        "name": p.file_stem().and_then(|s| s.to_str()),
                        "path": p.to_string_lossy(),
                    }));
                }
            }
        }
        if !layers.is_empty() {
            break;
        }
    }
    json!({ "layers": layers })
}

