//! Live platform operations for Bus handlers (PVR-011–014).
//! Filesystem projects under `projects_dir`; git via `git` CLI; compile via `veil`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

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

pub fn list_repos() -> Value {
    let dir = projects_dir();
    let projects = veil_server::list_projects(&dir).unwrap_or_default();
    json!({ "repos": projects, "projects_dir": dir.to_string_lossy() })
}

pub fn create_repo(name: &str, _description: Option<&str>) -> Value {
    let dir = projects_dir();
    match veil_server::create_project(&dir, name) {
        Ok(info) => json!(info),
        Err(e) => json!({ "error": e }),
    }
}

pub fn write_file(repo: &str, path: &str, content: &str, branch: Option<&str>) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let rel = Path::new(path);
    if rel.is_absolute() || path.contains("..") {
        return json!({ "error": "invalid path" });
    }
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&full, content) {
        Ok(()) => {
            let _ = branch;
            json!({
                "ok": true,
                "path": path,
                "bytes": content.len(),
                "repo": repo,
            })
        }
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub fn read_file(repo: &str, path: &str, _branch: Option<&str>) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    if path.contains("..") {
        return json!({ "error": "invalid path" });
    }
    let full = root.join(path);
    match std::fs::read_to_string(&full) {
        Ok(content) => json!({ "path": path, "content": content }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub fn list_files(repo: &str, prefix: Option<&str>) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let base = prefix.map(|p| root.join(p)).unwrap_or(root.clone());
    let mut files = Vec::new();
    walk(&base, &root, &mut files);
    files.sort();
    json!({ "repo": repo, "files": files })
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "target" || name == "generated" {
            continue;
        }
        if p.is_dir() {
            walk(&p, root, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
}

pub fn list_branches(repo: &str) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let out = Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(&root)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            let branches: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
            json!({ "repo": repo, "branches": branches })
        }
        Ok(o) => json!({
            "error": String::from_utf8_lossy(&o.stderr).to_string(),
            "hint": "git required in project"
        }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub fn create_branch(repo: &str, name: &str, from: Option<&str>) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let mut args = vec!["branch".to_string(), name.to_string()];
    if let Some(f) = from {
        args.push(f.to_string());
    }
    let out = Command::new("git").args(&args).current_dir(&root).output();
    match out {
        Ok(o) if o.status.success() => json!({ "ok": true, "branch": name }),
        Ok(o) => json!({ "error": String::from_utf8_lossy(&o.stderr).to_string() }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub fn get_diff(repo: &str, from_ref: Option<&str>, to_ref: Option<&str>) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let mut args = vec!["diff".to_string()];
    if let (Some(a), Some(b)) = (from_ref, to_ref) {
        args.push(format!("{a}...{b}"));
    }
    let out = Command::new("git").args(&args).current_dir(&root).output();
    match out {
        Ok(o) => json!({
            "repo": repo,
            "diff": String::from_utf8_lossy(&o.stdout).to_string(),
            "stderr": String::from_utf8_lossy(&o.stderr).to_string(),
            "ok": o.status.success(),
        }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub fn get_commit_log(repo: &str, limit: usize) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let out = Command::new("git")
        .args([
            "log",
            &format!("-{limit}"),
            "--pretty=format:%h|%an|%s|%ci",
        ])
        .current_dir(&root)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let commits: Vec<Value> = String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| {
                    let parts: Vec<&str> = l.splitn(4, '|').collect();
                    json!({
                        "hash": parts.first().copied().unwrap_or(""),
                        "author": parts.get(1).copied().unwrap_or(""),
                        "subject": parts.get(2).copied().unwrap_or(""),
                        "date": parts.get(3).copied().unwrap_or(""),
                    })
                })
                .collect();
            json!({ "repo": repo, "commits": commits })
        }
        Ok(o) => json!({ "error": String::from_utf8_lossy(&o.stderr).to_string() }),
        Err(e) => json!({ "error": e.to_string() }),
    }
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

pub fn deploy_local(repo: &str) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let compile = compile_project(repo);
    if compile.get("ok") != Some(&json!(true)) {
        return json!({ "error": "compile failed", "compile": compile });
    }
    let artifact_dir = veil_server::veil_home_dir()
        .join("artifacts")
        .join(repo);
    let record = json!({
        "repo": repo,
        "path": root.to_string_lossy(),
        "status": "local_registered",
        "artifact_dir": artifact_dir.to_string_lossy(),
    });
    let _ = std::fs::write(
        artifact_dir.join("deploy.json"),
        serde_json::to_string_pretty(&record).unwrap_or_default(),
    );
    record
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

/// Dispatch bus message type to platform impl.
pub fn handle_bus(msg: &Value) -> Value {
    let ty = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let strf = |k: &str| msg.get(k).and_then(|v| v.as_str());
    match ty {
        "ListRepos" | "ListReposTool" => list_repos(),
        "CreateRepo" | "CreateRepoTool" => {
            create_repo(strf("name").unwrap_or(""), strf("description"))
        }
        "WriteFile" | "WriteFileTool" => write_file(
            strf("repo_id").or_else(|| strf("repo")).unwrap_or(""),
            strf("path").unwrap_or(""),
            strf("content").unwrap_or(""),
            strf("branch"),
        ),
        "ReadFile" | "ReadFileTool" => read_file(
            strf("repo_id").or_else(|| strf("repo")).unwrap_or(""),
            strf("path").unwrap_or(""),
            strf("branch"),
        ),
        "ListFiles" | "ListFilesTool" => list_files(
            strf("repo_id").or_else(|| strf("repo")).unwrap_or(""),
            strf("prefix"),
        ),
        "ListBranches" | "ListBranchesTool" => {
            list_branches(strf("repo_id").or_else(|| strf("repo")).unwrap_or(""))
        }
        "CreateBranch" | "CreateBranchTool" => create_branch(
            strf("repo_id").or_else(|| strf("repo")).unwrap_or(""),
            strf("name").unwrap_or(""),
            strf("from"),
        ),
        "GetDiff" | "DiffTool" => get_diff(
            strf("repo_id").or_else(|| strf("repo")).unwrap_or(""),
            strf("from_ref"),
            strf("to_ref"),
        ),
        "GetCommitLog" | "LogTool" => get_commit_log(
            strf("repo_id").or_else(|| strf("repo")).unwrap_or(""),
            msg.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize,
        ),
        "Compile" | "CompileTool" => {
            compile_project(strf("repo_id").or_else(|| strf("repo")).unwrap_or(""))
        }
        "Deploy" | "DeployTool" => {
            deploy_local(strf("repo_id").or_else(|| strf("repo")).unwrap_or(""))
        }
        "HealthCheck" => json!({ "status": "ok", "service": "veil-runtime", "bus_mode": "live" }),
        "LoadConfig" => {
            let cfg = veil_server::load_config_or_default();
            json!({
                "projects_dir": cfg.projects_dir_path().to_string_lossy(),
                "configured": cfg.configured,
                "config_path": veil_server::config_path().to_string_lossy(),
            })
        }
        "HandleAgentMessage" | "HandleToolCall" => {
            // PVR-016: document ACP path; optional local echo with project context
            let project = strf("project").or_else(|| strf("repo")).unwrap_or("");
            let prompt = strf("message").or_else(|| strf("prompt")).unwrap_or("");
            json!({
                "status": "accepted",
                "project": project,
                "prompt_len": prompt.len(),
                "hint": "Use IDE agent dock (veil-server /api/p/{project}/agent/turn) for full ACP turns",
                "ide_path": format!("/api/p/{project}/agent/turn"),
            })
        }
        other => json!({
            "error": "not_implemented",
            "handler": other,
        }),
    }
}
