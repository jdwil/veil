//! Real git origin on the ProductHost bucket.
//!
//! On-bucket layout matches `git-remote-object-store` (gix + S3) **bundle**
//! engine: one git bundle per ref tip. Session workdirs are native git
//! checkouts. See `docs/ADR_GIT_ORIGIN_S3.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

const FORMAT_BUNDLE: &str = "bundle";

/// Origin is on when durable sessions are on, unless `VEIL_GIT_ORIGIN` forces it.
/// Self-contained (no `session` / `s3_workspace` import) to avoid a module cycle.
pub fn origin_enabled() -> bool {
    match env_flag("VEIL_GIT_ORIGIN", "auto") {
        Flag::Off => false,
        Flag::On => true,
        Flag::Auto => match env_flag("VEIL_SESSIONS", "auto") {
            Flag::Off => false,
            Flag::On => true,
            Flag::Auto => {
                let mode = std::env::var("VEIL_SOURCE_MODE")
                    .unwrap_or_else(|_| "prefer_s3".into())
                    .to_ascii_lowercase();
                !matches!(mode.as_str(), "disk" | "fs" | "filesystem" | "local")
            }
        },
    }
}

enum Flag {
    On,
    Off,
    Auto,
}

fn env_flag(name: &str, default: &str) -> Flag {
    match std::env::var(name)
        .unwrap_or_else(|_| default.into())
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "no" => Flag::Off,
        "1" | "true" | "on" | "yes" => Flag::On,
        _ => Flag::Auto,
    }
}

pub fn origin_prefix(repo_id: &str) -> String {
    format!("git/{}/", repo_id.trim().trim_matches('/'))
}

pub fn default_git_branch() -> String {
    std::env::var("VEIL_SOURCE_BRANCH").unwrap_or_else(|_| "main".into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutMode {
    /// Fetch remotes; do not discard local uncommitted work.
    FetchKeepDirty,
    /// `reset --hard` to the remote tip.
    ResetHard,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub parent: Option<String>,
    pub branch: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub sha: String,
    pub message: String,
    pub author: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusFile {
    pub path: String,
    /// Porcelain XY status (`M`, `A`, `D`, `??`, …).
    pub status: String,
}

pub struct GitOrigin {
    pub repo_id: String,
}

impl GitOrigin {
    pub fn new(repo_id: impl Into<String>) -> Self {
        Self {
            repo_id: repo_id.into(),
        }
    }

    pub fn exists(&self) -> bool {
        store_get(&format!("{}FORMAT", origin_prefix(&self.repo_id))).is_some()
            || store_get(&format!("{}HEAD", origin_prefix(&self.repo_id))).is_some()
    }

    /// Create origin from a working tree if the remote is empty.
    pub fn ensure_from_workdir(&self, seed: &Path, branch: &str) -> Result<String, String> {
        if self.exists() {
            return self
                .remote_tip(branch)
                .or_else(|| self.remote_tip(&default_git_branch()))
                .ok_or_else(|| format!("origin {} exists but has no tip", self.repo_id));
        }
        init_repo(seed, branch)?;
        if !has_source_files(seed) {
            return Err(format!(
                "cannot init origin {}: no source files in {}",
                self.repo_id,
                seed.display()
            ));
        }
        git(seed, &["add", "-A"])?;
        git(seed, &["commit", "-m", "Initial commit"])?;
        git(seed, &["branch", "-M", branch])?;
        self.push(seed, branch)
    }

    /// If origin is missing, import `repos/{id}/{branch}/` (legacy tree) as the first commit.
    pub fn import_legacy_tree(&self, branch: &str) -> Result<Option<String>, String> {
        if self.exists() {
            return Ok(self.remote_tip(branch));
        }
        if fs_store_root().is_some() {
            return Ok(None);
        }
        let tmp = unique_tmp(&format!("veil-git-import-{}", &self.repo_id[..8.min(self.repo_id.len())]));
        fs::create_dir_all(&tmp).map_err(|e| format!("mkdir import: {e}"))?;
        let src = format!(
            "s3://{}/{}/{branch}/",
            bucket(),
            format!("repos/{}", self.repo_id)
        );
        let out = aws_base()
            .args(["s3", "sync", &src, &tmp.to_string_lossy(), "--exact-timestamps"])
            .output()
            .map_err(|e| format!("aws s3 sync import: {e}"))?;
        if !out.status.success() {
            let _ = fs::remove_dir_all(&tmp);
            return Ok(None);
        }
        if !has_source_files(&tmp) {
            let _ = fs::remove_dir_all(&tmp);
            return Ok(None);
        }
        let sha = self.ensure_from_workdir(&tmp, branch);
        let _ = fs::remove_dir_all(&tmp);
        sha.map(Some)
    }

    /// Ensure origin exists (import legacy tree or seed workdir).
    pub fn ensure(&self, seed: Option<&Path>, branch: &str) -> Result<(), String> {
        if self.exists() {
            return Ok(());
        }
        if let Some(seed) = seed {
            if has_source_files(seed) {
                self.ensure_from_workdir(seed, branch)?;
                return Ok(());
            }
        }
        if self.import_legacy_tree(branch)?.is_some() {
            return Ok(());
        }
        Err(format!(
            "no git origin for {} and no seed/legacy tree to import",
            self.repo_id
        ))
    }

    pub fn checkout(
        &self,
        work: &Path,
        branch: &str,
        mode: CheckoutMode,
    ) -> Result<String, String> {
        self.ensure(if work.exists() { Some(work) } else { None }, branch)
            .or_else(|_| self.ensure(None, &default_git_branch()))?;
        fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;

        let remote = self
            .download_tip(branch)
            .or_else(|| self.download_tip(&default_git_branch()));
        let Some(remote) = remote else {
            return Err(format!("origin {} has no bundles yet", self.repo_id));
        };

        if !work.join(".git").is_dir() {
            clone_from_bundle(work, &remote.bundle_path, &remote.branch)?;
            if remote.branch != branch {
                git(work, &["checkout", "-B", branch])?;
            }
            let _ = fs::remove_file(&remote.bundle_path);
            return git(work, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
        }

        git(
            work,
            &[
                "fetch",
                &remote.bundle_path.to_string_lossy(),
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )?;
        let _ = fs::remove_file(&remote.bundle_path);

        let local_branch = current_branch(work).unwrap_or_default();
        if local_branch != branch {
            if branch_exists_local(work, branch) || remote_branch_exists(work, branch) {
                git(work, &["checkout", branch])?;
            } else {
                git(work, &["checkout", "-B", branch])?;
            }
        }

        match mode {
            CheckoutMode::ResetHard => {
                let tip = format!("origin/{branch}");
                if ref_exists(work, &tip) {
                    git(work, &["reset", "--hard", &tip])?;
                }
            }
            CheckoutMode::FetchKeepDirty => {
                if !status_dirty(work)? {
                    let tip = format!("origin/{branch}");
                    if ref_exists(work, &tip) {
                        let _ = git(work, &["merge", "--ff-only", &tip]);
                    }
                }
            }
        }
        git(work, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
    }

    pub fn create_branch(&self, work: &Path, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("branch name required".into());
        }
        if !work.join(".git").is_dir() {
            return Err("create_branch: workdir is not a git checkout".into());
        }
        git(work, &["checkout", "-B", name])?;
        Ok(())
    }

    pub fn commit(&self, work: &Path, message: &str) -> Result<CommitInfo, String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("commit message required".into());
        }
        if !work.join(".git").is_dir() {
            return Err("commit: workdir is not a git checkout".into());
        }
        ensure_gitignore(work)?;
        git(work, &["add", "-A"])?;
        if !status_dirty(work)? {
            return Err(
                "nothing to commit — working tree clean. Edit with write_source first.".into(),
            );
        }
        let parent = git(work, &["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string());
        git(work, &["commit", "-m", message])?;
        let sha = git(work, &["rev-parse", "HEAD"])?.trim().to_string();
        let branch = current_branch(work)?;
        let files = changed_files(work, parent.as_deref())?;
        Ok(CommitInfo {
            sha,
            message: message.to_string(),
            parent,
            branch,
            files,
        })
    }

    pub fn push(&self, work: &Path, branch: &str) -> Result<String, String> {
        if !work.join(".git").is_dir() {
            return Err("push: workdir is not a git checkout".into());
        }
        git(work, &["rev-parse", "--verify", branch])?;
        let sha = git(work, &["rev-parse", branch])?.trim().to_string();
        let bundle = unique_tmp(&format!("veil-{}.bundle", &sha[..8.min(sha.len())]));
        git(
            work,
            &[
                "bundle",
                "create",
                &bundle.to_string_lossy(),
                branch,
            ],
        )?;
        let bytes = fs::read(&bundle).map_err(|e| format!("read bundle: {e}"))?;
        let _ = fs::remove_file(&bundle);

        let prefix = origin_prefix(&self.repo_id);
        store_put(&format!("{prefix}FORMAT"), FORMAT_BUNDLE.as_bytes())?;
        let prev = store_get(&format!("{prefix}refs/heads/{branch}/TIP"))
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != &sha);

        store_put(
            &format!("{prefix}refs/heads/{branch}/{sha}.bundle"),
            &bytes,
        )?;
        store_put(
            &format!("{prefix}refs/heads/{branch}/TIP"),
            sha.as_bytes(),
        )?;
        if branch == default_git_branch() || !self.exists_head() {
            store_put(&format!("{prefix}HEAD"), format!("refs/heads/{branch}").as_bytes())?;
        }
        if let Some(old) = prev {
            let _ = store_delete(&format!("{prefix}refs/heads/{branch}/{old}.bundle"));
        }
        let _ = self.publish_checkout_cache(work, branch);
        Ok(sha)
    }

    pub fn commit_and_push(&self, work: &Path, message: &str, branch: &str) -> Result<CommitInfo, String> {
        let mut info = self.commit(work, message)?;
        if info.branch != branch && !branch.is_empty() {
            git(work, &["branch", "-M", branch])?;
            info.branch = branch.to_string();
        }
        let sha = self.push(work, &info.branch)?;
        info.sha = sha;
        Ok(info)
    }

    /// Merge `source` into `target` in `work` and push `target`.
    pub fn merge_and_push(
        &self,
        work: &Path,
        source: &str,
        target: &str,
    ) -> Result<String, String> {
        if source != target && work.join(".git").is_dir() && branch_exists_local(work, source) {
            let _ = self.push(work, source);
        }
        self.checkout(work, target, CheckoutMode::ResetHard)?;
        if source != target {
            if let Some(remote) = self.download_tip(source) {
                let _ = git(
                    work,
                    &[
                        "fetch",
                        &remote.bundle_path.to_string_lossy(),
                        &format!("+refs/heads/{source}:refs/remotes/origin/{source}"),
                    ],
                );
                let _ = fs::remove_file(&remote.bundle_path);
            }
            let merge_ref = if ref_exists(work, source) {
                source.to_string()
            } else {
                format!("origin/{source}")
            };
            git(
                work,
                &[
                    "merge",
                    "--no-ff",
                    "-m",
                    &format!("Merge branch '{source}'"),
                    &merge_ref,
                ],
            )?;
        }
        self.push(work, target)
    }

    pub fn remote_tip(&self, branch: &str) -> Option<String> {
        store_get(&format!(
            "{}refs/heads/{branch}/TIP",
            origin_prefix(&self.repo_id)
        ))
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    }

    pub fn log(&self, work: &Path, n: usize) -> Result<Vec<LogEntry>, String> {
        let n = n.max(1).min(100).to_string();
        let out = git(
            work,
            &[
                "log",
                &format!("-{n}"),
                "--format=%H%x09%P%x09%cI%x09%an%x09%s",
            ],
        )?;
        let mut entries = Vec::new();
        for line in out.lines() {
            let mut parts = line.splitn(5, '\t');
            let sha = parts.next().unwrap_or("").to_string();
            if sha.is_empty() {
                continue;
            }
            let parent = parts
                .next()
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty());
            let created_at = parts.next().unwrap_or("").to_string();
            let author = parts.next().unwrap_or("").to_string();
            let message = parts.next().unwrap_or("").to_string();
            let files = git(
                work,
                &["diff-tree", "--no-commit-id", "--name-only", "-r", &sha],
            )
            .unwrap_or_default()
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
            entries.push(LogEntry {
                sha,
                message,
                author,
                parent,
                created_at,
                files,
            });
        }
        Ok(entries)
    }

    /// `git status --porcelain` in a checkout.
    pub fn status_files(work: &Path) -> Result<Vec<StatusFile>, String> {
        if !work.join(".git").is_dir() {
            return Ok(vec![]);
        }
        let out = git(work, &["status", "--porcelain", "-uall"])?;
        Ok(out
            .lines()
            .filter_map(|line| {
                if line.len() < 4 {
                    return None;
                }
                let status = line[..2].trim().to_string();
                let path = line[3..].trim().replace(" -> ", "/");
                if path.is_empty() {
                    return None;
                }
                Some(StatusFile { path, status })
            })
            .collect())
    }

    /// Working-tree diff vs `HEAD` (`git diff HEAD` + untracked as new files).
    pub fn working_diff(work: &Path) -> Result<String, String> {
        if !work.join(".git").is_dir() {
            return Ok(String::new());
        }
        let tracked = git(work, &["diff", "--no-color", "HEAD"]).unwrap_or_default();
        let untracked = git(work, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
        if untracked.trim().is_empty() {
            return Ok(tracked);
        }
        let mut out = tracked;
        for path in untracked.lines().map(str::trim).filter(|s| !s.is_empty()) {
            let body = fs::read_to_string(work.join(path)).unwrap_or_default();
            out.push_str(&format!("diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"));
            for line in body.lines() {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// Unified diff `from...to` using origin bundles (no shared workdir).
    pub fn unified_diff_refs(&self, from: &str, to: &str) -> Result<String, String> {
        let tmp = unique_tmp("diff-refs");
        self.checkout(&tmp, to, CheckoutMode::ResetHard)?;
        if let Some(remote) = self.download_tip(from) {
            let _ = git(
                &tmp,
                &[
                    "fetch",
                    &remote.bundle_path.to_string_lossy(),
                    &format!("+refs/heads/{from}:refs/remotes/origin/{from}"),
                ],
            );
            let _ = fs::remove_file(&remote.bundle_path);
        }
        let spec = format!("origin/{from}...HEAD");
        let patch = git(&tmp, &["diff", "--no-color", &spec]).unwrap_or_default();
        let _ = fs::remove_dir_all(&tmp);
        Ok(patch)
    }

    /// Checkout `branch` into a fresh temp dir. Caller deletes it.
    pub fn checkout_tmp(&self, branch: &str) -> Result<PathBuf, String> {
        let tmp = unique_tmp(&format!("ref-{branch}"));
        self.checkout(&tmp, branch, CheckoutMode::ResetHard)?;
        Ok(tmp)
    }

    pub fn list_tree_at(&self, work: &Path) -> Result<Vec<String>, String> {
        let out = git(work, &["ls-tree", "-r", "--name-only", "HEAD"])?;
        Ok(out
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    fn exists_head(&self) -> bool {
        store_get(&format!("{}HEAD", origin_prefix(&self.repo_id))).is_some()
    }

    fn download_tip(&self, branch: &str) -> Option<DownloadedBundle> {
        let sha = self.remote_tip(branch)?;
        let key = format!(
            "{}refs/heads/{branch}/{sha}.bundle",
            origin_prefix(&self.repo_id)
        );
        let bytes = store_get(&key)?;
        let path = unique_tmp(&format!("{sha}.bundle"));
        if fs::write(&path, bytes).is_err() {
            return None;
        }
        Some(DownloadedBundle {
            branch: branch.to_string(),
            sha,
            bundle_path: path,
        })
    }

    /// Mirror the pushed tree to `repos/{id}/{branch}/` for compile/HTTP cache.
    pub fn publish_checkout_cache(&self, work: &Path, branch: &str) -> Result<(), String> {
        if fs_store_root().is_some() {
            return Ok(());
        }
        let dest = format!("s3://{}/repos/{}/{}/", bucket(), self.repo_id, branch);
        let out = aws_base()
            .args([
                "s3",
                "sync",
                &work.to_string_lossy(),
                &dest,
                "--exclude",
                ".git/*",
                "--exclude",
                ".veil-session.json",
                "--exclude",
                "target/*",
                "--exclude",
                "generated/*",
                "--exclude",
                "node_modules/*",
            ])
            .output()
            .map_err(|e| format!("aws s3 sync checkout cache: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "checkout cache sync failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(())
    }
}

struct DownloadedBundle {
    branch: String,
    #[allow(dead_code)]
    sha: String,
    bundle_path: PathBuf,
}

fn init_repo(work: &Path, branch: &str) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    if !work.join(".git").is_dir() {
        git(work, &["init", "-b", branch])?;
    }
    ensure_gitignore(work)?;
    Ok(())
}

/// `git clone <bundle> <work>` — dest must be missing or empty. If `work` already
/// has files (session marker), clone to a sibling and move `.git` + tracked files.
fn clone_from_bundle(work: &Path, bundle: &Path, branch: &str) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    let empty = fs::read_dir(work)
        .map(|rd| rd.filter_map(|e| e.ok()).count() == 0)
        .unwrap_or(true);
    let dest = if empty {
        work.to_path_buf()
    } else {
        unique_tmp("clone")
    };
    if dest != work {
        fs::create_dir_all(&dest).map_err(|e| format!("mkdir clone: {e}"))?;
    }
    // Clone from the parent so we can pass an absolute dest.
    let parent = dest.parent().unwrap_or(Path::new("/tmp"));
    let name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("clone dest name")?;
    let mut cmd = Command::new("git");
    cmd.current_dir(parent)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=VEIL",
            "-c",
            "user.email=veil@localhost",
            "clone",
            "--branch",
            branch,
            &bundle.to_string_lossy(),
            name,
        ]);
    let out = cmd
        .output()
        .map_err(|e| format!("git clone bundle: {e}"))?;
    if !out.status.success() {
        // Retry without --branch (bundle may advertise HEAD only).
        let mut cmd = Command::new("git");
        cmd.current_dir(parent)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "clone",
                &bundle.to_string_lossy(),
                name,
            ]);
        let out2 = cmd
            .output()
            .map_err(|e| format!("git clone bundle: {e}"))?;
        if !out2.status.success() {
            return Err(format!(
                "git clone bundle failed: {}",
                String::from_utf8_lossy(&out2.stderr).trim()
            ));
        }
    }
    if dest != work {
        // Move checkout contents onto the existing workdir (keep session marker).
        copy_tree(&dest, work)?;
        let _ = fs::remove_dir_all(&dest);
    }
    ensure_gitignore(work)?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fn rec(from: &Path, to: &Path) -> Result<(), String> {
        fs::create_dir_all(to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
        for e in fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
            let e = e.map_err(|e| format!("readdir: {e}"))?;
            let src = e.path();
            let dst = to.join(e.file_name());
            if src.is_dir() {
                rec(&src, &dst)?;
            } else {
                fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
            }
        }
        Ok(())
    }
    rec(from, to)
}

fn ensure_gitignore(work: &Path) -> Result<(), String> {
    let gi = work.join(".gitignore");
    if gi.is_file() {
        let cur = fs::read_to_string(&gi).unwrap_or_default();
        if !cur.lines().any(|l| l.trim() == ".veil-session.json") {
            let mut next = cur;
            if !next.ends_with('\n') && !next.is_empty() {
                next.push('\n');
            }
            next.push_str(".veil-session.json\n");
            fs::write(&gi, next).map_err(|e| format!("write .gitignore: {e}"))?;
        }
        return Ok(());
    }
    fs::write(
        gi,
        ".veil-session.json\ntarget/\ngenerated/\nnode_modules/\ndist/\n",
    )
    .map_err(|e| format!("write .gitignore: {e}"))
}

pub fn status_dirty(work: &Path) -> Result<bool, String> {
    if !work.join(".git").is_dir() {
        return Ok(false);
    }
    let out = git(work, &["status", "--porcelain"])?;
    Ok(out.lines().any(|l| !l.trim().is_empty()))
}

fn current_branch(work: &Path) -> Result<String, String> {
    let s = git(work, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(s.trim().to_string())
}

fn branch_exists_local(work: &Path, name: &str) -> bool {
    git(work, &["rev-parse", "--verify", &format!("refs/heads/{name}")]).is_ok()
}

fn remote_branch_exists(work: &Path, name: &str) -> bool {
    git(
        work,
        &["rev-parse", "--verify", &format!("refs/remotes/origin/{name}")],
    )
    .is_ok()
}

fn ref_exists(work: &Path, name: &str) -> bool {
    git(work, &["rev-parse", "--verify", name]).is_ok()
}

fn changed_files(work: &Path, parent: Option<&str>) -> Result<Vec<String>, String> {
    let out = if let Some(p) = parent {
        git(work, &["diff-tree", "--no-commit-id", "--name-only", "-r", p, "HEAD"])?
    } else {
        git(work, &["ls-files"])?
    };
    let mut files: Vec<String> = out
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    files.sort();
    Ok(files)
}

pub fn has_source_files(root: &Path) -> bool {
    fn rec(p: &Path) -> bool {
        let Ok(rd) = fs::read_dir(p) else {
            return false;
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
                if rec(&path) {
                    return true;
                }
            } else if name.ends_with(".veil")
                || name.ends_with(".layer")
                || name == "veil.toml"
                || name == "MISSION.md"
            {
                return true;
            }
        }
        false
    }
    rec(root)
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", git_author_name())
        .env("GIT_AUTHOR_EMAIL", git_author_email())
        .env("GIT_COMMITTER_NAME", git_author_name())
        .env("GIT_COMMITTER_EMAIL", git_author_email())
        .args(["-c", "user.name=VEIL", "-c", "user.email=veil@localhost"])
        .args(args);
    let out = cmd
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn git_author_name() -> String {
    std::env::var("VEIL_GIT_AUTHOR_NAME")
        .or_else(|_| std::env::var("VEIL_DEV_USER"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "VEIL".into())
}

fn git_author_email() -> String {
    std::env::var("VEIL_GIT_AUTHOR_EMAIL").unwrap_or_else(|_| "veil@localhost".into())
}

fn unique_tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("veil-git-origin");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!("{}-{}", uuid::Uuid::new_v4(), name))
}

fn bucket() -> String {
    std::env::var("VEIL_S3_BUCKET")
        .or_else(|_| std::env::var("BUCKET"))
        .unwrap_or_else(|_| "veil-runtime-dev".into())
}

fn aws_base() -> Command {
    let mut c = Command::new("aws");
    if let Ok(p) = std::env::var("AWS_PROFILE") {
        c.env("AWS_PROFILE", p);
    }
    if let Ok(r) = std::env::var("AWS_REGION") {
        c.env("AWS_REGION", r);
    }
    c
}

fn fs_store_root() -> Option<PathBuf> {
    std::env::var("VEIL_GIT_STORE_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn store_put(key: &str, bytes: &[u8]) -> Result<(), String> {
    if let Some(root) = fs_store_root() {
        let path = root.join(key);
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))?;
        }
        return fs::write(path, bytes).map_err(|e| format!("write {key}: {e}"));
    }
    let dest = format!("s3://{}/{key}", bucket());
    let mut child = aws_base()
        .args(["s3", "cp", "-", &dest])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("aws s3 cp: {e}"))?;
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or("aws s3 cp: no stdin")?;
        stdin
            .write_all(bytes)
            .map_err(|e| format!("aws s3 cp write: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("aws s3 cp wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 cp {dest} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

fn store_get(key: &str) -> Option<Vec<u8>> {
    if let Some(root) = fs_store_root() {
        return fs::read(root.join(key)).ok();
    }
    let src = format!("s3://{}/{key}", bucket());
    let out = aws_base().args(["s3", "cp", &src, "-"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(out.stdout)
}

fn store_delete(key: &str) -> Result<(), String> {
    if let Some(root) = fs_store_root() {
        let _ = fs::remove_file(root.join(key));
        return Ok(());
    }
    let out = aws_base()
        .args(["s3", "rm", &format!("s3://{}/{key}", bucket())])
        .output()
        .map_err(|e| format!("aws s3 rm: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 rm {key} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_store<F: FnOnce(&Path)>(f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = unique_tmp("store");
        fs::create_dir_all(&root).unwrap();
        // SAFETY: tests are serialized by ENV_LOCK; this process is the only writer.
        unsafe {
            std::env::set_var("VEIL_GIT_STORE_ROOT", &root);
            std::env::set_var("VEIL_GIT_ORIGIN", "1");
        }
        f(&root);
        unsafe {
            std::env::remove_var("VEIL_GIT_STORE_ROOT");
            std::env::remove_var("VEIL_GIT_ORIGIN");
        }
        let _ = fs::remove_dir_all(&root);
    }

    fn seed_tree() -> PathBuf {
        let p = unique_tmp("seed");
        fs::create_dir_all(p.join("layers")).unwrap();
        fs::write(p.join("main.veil"), "pkg DlxBus\n").unwrap();
        fs::write(p.join("MISSION.md"), "# Bus\n").unwrap();
        fs::write(p.join("veil.toml"), "[package]\nname = \"dlx-bus\"\n").unwrap();
        fs::write(p.join("layers/main.layer"), "layer Main\n").unwrap();
        p
    }

    #[test]
    fn origin_roundtrip_commit_branch_merge() {
        with_store(|_| {
            let origin = GitOrigin::new("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
            let seed = seed_tree();
            let sha0 = origin.ensure_from_workdir(&seed, "main").unwrap();
            assert_eq!(sha0.len(), 40);
            assert!(origin.exists());

            let work = unique_tmp("sess-a");
            origin
                .checkout(&work, "main", CheckoutMode::ResetHard)
                .unwrap();
            assert!(work.join("main.veil").is_file());
            assert!(work.join(".git").is_dir());

            origin.create_branch(&work, "feat-bus").unwrap();
            fs::write(work.join("main.veil"), "pkg DlxBus\n  rec Topic\n").unwrap();
            let c = origin
                .commit_and_push(&work, "feat: add Topic", "feat-bus")
                .unwrap();
            assert_eq!(c.branch, "feat-bus");
            assert_ne!(c.sha, sha0);

            let other = unique_tmp("sess-b");
            origin
                .checkout(&other, "feat-bus", CheckoutMode::ResetHard)
                .unwrap();
            let body = fs::read_to_string(other.join("main.veil")).unwrap();
            assert!(body.contains("Topic"));

            let patch = origin.unified_diff_refs("main", "feat-bus").unwrap();
            assert!(
                patch.contains("Topic"),
                "git diff main...feat-bus should contain Topic, got:\n{patch}"
            );

            origin.merge_and_push(&work, "feat-bus", "main").unwrap();
            let mainline = unique_tmp("sess-main");
            origin
                .checkout(&mainline, "main", CheckoutMode::ResetHard)
                .unwrap();
            let merged = fs::read_to_string(mainline.join("main.veil")).unwrap();
            assert!(merged.contains("Topic"));

            let _ = fs::remove_dir_all(&seed);
            let _ = fs::remove_dir_all(&work);
            let _ = fs::remove_dir_all(&other);
            let _ = fs::remove_dir_all(&mainline);
        });
    }

    #[test]
    fn second_session_does_not_see_uncommitted() {
        with_store(|_| {
            let origin = GitOrigin::new("11111111-2222-3333-4444-555555555555");
            let seed = seed_tree();
            origin.ensure_from_workdir(&seed, "main").unwrap();

            let a = unique_tmp("iso-a");
            origin.checkout(&a, "main", CheckoutMode::ResetHard).unwrap();
            fs::write(a.join("main.veil"), "pkg Dirty\n").unwrap();

            let b = unique_tmp("iso-b");
            origin.checkout(&b, "main", CheckoutMode::ResetHard).unwrap();
            let body = fs::read_to_string(b.join("main.veil")).unwrap();
            assert!(body.contains("DlxBus"));
            assert!(!body.contains("Dirty"));

            let _ = fs::remove_dir_all(&seed);
            let _ = fs::remove_dir_all(&a);
            let _ = fs::remove_dir_all(&b);
        });
    }
}
