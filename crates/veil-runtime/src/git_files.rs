//! File read/write/list for git-backed projects, over a `GitOrigin` working
//! tree (honouring the hybrid-model subpath).
//!
//! S3-backed projects continue to use `storage::application::{read_file,
//! write_file,list_files}` against `repos/{id}/{branch}/{path}`. Git-backed
//! projects instead: checkout the branch → operate on the working tree at
//! `{workdir}/{subpath}` → (for writes) commit + push to the provider.

use std::path::PathBuf;

use veil_server::git_origin::{CheckoutMode, GitOrigin};

/// Stable per-repo checkout cache dir. Reused across requests so we are not
/// re-cloning the whole repo on every read.
fn work_dir(repo_id: &str) -> PathBuf {
    let short: String = repo_id.chars().take(16).collect();
    std::env::temp_dir()
        .join("veil-git-work")
        .join(short)
}

/// Read a file from a git-backed project working tree.
/// Returns `Ok(None)` when the file does not exist.
pub fn read_file(
    origin: &GitOrigin,
    branch: &str,
    rel_path: &str,
) -> Result<Option<String>, String> {
    let work = work_dir(&origin.repo_id);
    origin.checkout(&work, branch, CheckoutMode::FetchKeepDirty)?;
    let root = origin.project_root(&work);
    let full = root.join(rel_path);
    match std::fs::read(&full) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", full.display())),
    }
}

/// List files (repo-relative to the project root / subpath) in a git-backed project.
pub fn list_files(
    origin: &GitOrigin,
    branch: &str,
    prefix: &str,
) -> Result<Vec<String>, String> {
    let work = work_dir(&origin.repo_id);
    origin.checkout(&work, branch, CheckoutMode::FetchKeepDirty)?;
    let root = origin.project_root(&work);
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    let prefix = prefix.trim_start_matches('/');
    out.retain(|p| prefix.is_empty() || p.starts_with(prefix));
    out.sort();
    Ok(out)
}

fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if matches!(
                name.as_str(),
                ".git" | "target" | "generated" | "node_modules" | "dist"
            ) {
                continue;
            }
            walk(base, &path, out);
        } else if name != ".veil-session.json" {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Write a file into a git-backed project, commit, and push the branch to the
/// provider. Returns the pushed commit SHA.
pub fn write_file(
    origin: &GitOrigin,
    branch: &str,
    rel_path: &str,
    content: &str,
    message: &str,
    author_name: Option<&str>,
    author_email: Option<&str>,
) -> Result<String, String> {
    let work = work_dir(&origin.repo_id);
    origin.checkout(work.as_path(), branch, CheckoutMode::FetchKeepDirty)?;
    let root = origin.project_root(&work);
    let full = root.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(&full, content.as_bytes()).map_err(|e| format!("write {}: {e}", full.display()))?;

    // Record the human as author; runtime service identity is the committer
    // (via git_origin's GIT_COMMITTER_* env defaults).
    if let Some(n) = author_name {
        // SAFETY: single-threaded request path; env is process-local config.
        unsafe { std::env::set_var("VEIL_GIT_AUTHOR_NAME", n) };
    }
    if let Some(email) = author_email {
        unsafe { std::env::set_var("VEIL_GIT_AUTHOR_EMAIL", email) };
    }

    let info = origin.commit_and_push(&work, message, branch);
    match info {
        Ok(info) => Ok(info.sha),
        Err(e) if e.contains("nothing to commit") => {
            // Identical content already committed — treat as a successful no-op
            // and return the current tip so callers see a stable SHA.
            origin
                .remote_tip(branch)
                .ok_or_else(|| format!("no-op write and no remote tip for {branch}: {e}"))
        }
        Err(e) => Err(e),
    }
}
