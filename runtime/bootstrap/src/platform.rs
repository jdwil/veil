//! Live platform operations for Bus handlers (PVR-011–014).
//! Filesystem projects under `projects_dir`; git via `git` CLI; compile via `veil`.
//!
//! CAP-003: `HANDLER_NAMES` is the single registry the trampoline uses.
//! CAP-004: `FileSystem` / `GitRepo` local adapters for DI / tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

/// Canonical bus handler names (CAP-003). Mirrors generated `register_handlers`.
/// Host trampoline is the single registration entry; do not hardcode elsewhere.
pub const HANDLER_NAMES: &[&str] = &[
    "CreateRepo",
    "ListRepos",
    "WriteFile",
    "ReadFile",
    "ListFiles",
    "CreateBranch",
    "ListBranches",
    "GetDiff",
    "Compile",
    "Deploy",
    "GetCommitLog",
    "CreateRepoTool",
    "WriteFileTool",
    "ReadFileTool",
    "ListFilesTool",
    "CreateBranchTool",
    "ListBranchesTool",
    "DiffTool",
    "CompileTool",
    "DeployTool",
    "ListReposTool",
    "LogTool",
    "HealthCheck",
    "LoadConfig",
    "HandleConnection",
    "HandleAgentMessage",
    "HandleToolCall",
    "ParseManifest",
    "ReadAllManifests",
    "LoadEnvConfig",
    "WireApplication",
    "RunSecurityScan",
    "StartHarness",
];

/// CAP-003-style registration: call once from the trampoline.
pub fn register_all<F>(mut register: F)
where
    F: FnMut(&'static str),
{
    for name in HANDLER_NAMES {
        register(name);
    }
}

// ─── CAP-004: system ports (local defaults) ─────────────────────────────────

/// Injectable filesystem port (local projects tree).
pub trait FileSystem: Send + Sync {
    fn read(&self, path: &Path) -> Result<String, String>;
    fn write(&self, path: &Path, content: &str) -> Result<(), String>;
    fn list(&self, dir: &Path) -> Result<Vec<String>, String>;
}

/// Injectable git port.
pub trait GitRepo: Send + Sync {
    fn branches(&self, repo: &Path) -> Result<Vec<String>, String>;
    fn log(&self, repo: &Path, limit: usize) -> Result<Vec<String>, String>;
}

/// Default FS rooted at `root` (rejects path escape).
pub struct LocalFileSystem {
    pub root: PathBuf,
}

impl LocalFileSystem {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let canon_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        if let Ok(c) = full.canonicalize() {
            if !c.starts_with(&canon_root) {
                return Err("path escapes root".into());
            }
            return Ok(c);
        }
        // File may not exist yet (write) — check parent
        if let Some(parent) = full.parent() {
            if let Ok(p) = parent.canonicalize() {
                if !p.starts_with(&canon_root) {
                    return Err("path escapes root".into());
                }
            }
        }
        Ok(full)
    }
}

impl FileSystem for LocalFileSystem {
    fn read(&self, path: &Path) -> Result<String, String> {
        let full = self.resolve(path)?;
        std::fs::read_to_string(&full).map_err(|e| e.to_string())
    }

    fn write(&self, path: &Path, content: &str) -> Result<(), String> {
        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&full, content).map_err(|e| e.to_string())
    }

    fn list(&self, dir: &Path) -> Result<Vec<String>, String> {
        let full = self.resolve(dir)?;
        let mut out = Vec::new();
        walk_names(&full, &full, &mut out);
        out.sort();
        Ok(out)
    }
}

fn walk_names(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "target" {
            continue;
        }
        if p.is_dir() {
            walk_names(&p, root, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
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

#[cfg(test)]
mod fs_tests {
    use super::*;

    #[test]
    fn local_fs_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "veil_fs_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fs = LocalFileSystem::new(&dir);
        fs.write(Path::new("a/b.txt"), "hello").unwrap();
        assert_eq!(fs.read(Path::new("a/b.txt")).unwrap(), "hello");
        let list = fs.list(Path::new(".")).unwrap();
        assert!(list.iter().any(|p| p.ends_with("b.txt")), "{list:?}");
        let _ = std::fs::remove_dir_all(&dir);
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
    // CAP-004: inject LocalFileSystem rooted at project
    let fs = LocalFileSystem::new(&root);
    match fs.write(rel, content) {
        Ok(()) => {
            let _ = branch;
            json!({
                "ok": true,
                "path": path,
                "bytes": content.len(),
                "repo": repo,
            })
        }
        Err(e) => json!({ "error": e }),
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
    let fs = LocalFileSystem::new(&root);
    match fs.read(Path::new(path)) {
        Ok(content) => json!({ "path": path, "content": content }),
        Err(e) => json!({ "error": e }),
    }
}

pub fn list_files(repo: &str, prefix: Option<&str>) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    let fs = LocalFileSystem::new(&root);
    let prefix_path = Path::new(prefix.unwrap_or("."));
    match fs.list(prefix_path) {
        Ok(files) => json!({ "repo": repo, "files": files }),
        Err(e) => json!({ "error": e }),
    }
}

pub fn list_branches(repo: &str) -> Value {
    let root = match project_root(repo) {
        Ok(r) => r,
        Err(e) => return json!({ "error": e }),
    };
    // CAP-004: GitRepo port
    match LocalGit.branches(&root) {
        Ok(branches) => json!({ "repo": repo, "branches": branches }),
        Err(e) => json!({ "error": e, "hint": "git required in project" }),
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
    // CAP-004: detailed log still via git CLI; GitRepo::log is oneline summary
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
            // Also exercise GitRepo port for summary lines
            let _summary = LocalGit.log(&root, limit);
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
