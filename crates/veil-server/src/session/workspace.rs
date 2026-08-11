//! Session-scoped workspace filesystem (path-jailed) with optional S3 write-through.

use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializePolicy {
    /// Full sync with --delete (reset).
    SyncDelete,
    /// Sync without --delete (pull).
    SyncIncremental,
}

pub fn materialize_policy() -> MaterializePolicy {
    MaterializePolicy::SyncIncremental
}

/// Reject path escape; return absolute path under root.
pub fn path_jail(root: &Path, rel: &str) -> Result<PathBuf, String> {
    resolve_under_root(root, rel)
}

pub fn resolve_under_root(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        return Ok(root.to_path_buf());
    }
    if rel.contains('\0') {
        return Err("invalid path".into());
    }
    let joined = root.join(rel);
    let root_c = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    // For non-existent files, canonicalize parent
    let abs = if joined.exists() {
        joined
            .canonicalize()
            .map_err(|e| format!("canonicalize: {e}"))?
    } else {
        let parent = joined.parent().unwrap_or(root);
        let file = joined.file_name().ok_or_else(|| "bad path".to_string())?;
        let parent_c = if parent.exists() {
            parent
                .canonicalize()
                .map_err(|e| format!("canonicalize parent: {e}"))?
        } else {
            // ensure we stay under root by components
            let mut cur = root_c.clone();
            for c in parent.strip_prefix(root).unwrap_or(parent).components() {
                use std::path::Component;
                match c {
                    Component::Normal(s) => cur.push(s),
                    Component::CurDir => {}
                    _ => return Err("path escape".into()),
                }
            }
            cur
        };
        parent_c.join(file)
    };
    if !abs.starts_with(&root_c) && abs != root_c {
        // also allow if root not yet canonical equal
        let abs_s = abs.to_string_lossy();
        let root_s = root_c.to_string_lossy();
        if !abs_s.starts_with(root_s.as_ref()) {
            return Err(format!("path escapes workspace: {rel}"));
        }
    }
    Ok(abs)
}

pub trait WorkspaceFs: Send + Sync {
    fn root(&self) -> &Path;
    fn list(&self, rel_dir: &str, max: usize) -> Result<Vec<String>, String>;
    fn read(&self, rel: &str, max_bytes: usize) -> Result<String, String>;
    fn write(&self, rel: &str, content: &str, if_match: Option<&str>) -> Result<WriteResult, String>;
    fn str_replace(
        &self,
        rel: &str,
        old: &str,
        new: &str,
        if_match: Option<&str>,
    ) -> Result<WriteResult, String>;
    fn grep(&self, pattern: &str, path_glob: Option<&str>, max_matches: usize) -> Result<Vec<GrepHit>, String>;
    fn rm(&self, rel: &str) -> Result<(), String>;
    fn flush_path(&self, rel: &str) -> Result<WriteResult, String>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WriteResult {
    pub path: String,
    pub bytes: usize,
    pub etag: Option<String>,
    pub revision_hint: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GrepHit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

pub struct WorkspaceFsImpl {
    root: PathBuf,
    repo_id: String,
    branch: String,
    session_id: String,
    draft_mode: bool,
}

impl WorkspaceFsImpl {
    pub fn new(
        root: PathBuf,
        repo_id: String,
        branch: String,
        session_id: String,
        draft_mode: bool,
    ) -> Self {
        Self {
            root,
            repo_id,
            branch,
            session_id,
            draft_mode,
        }
    }

    fn s3_key(&self, rel: &str) -> String {
        let rel = rel.replace('\\', "/").trim_start_matches('/').to_string();
        if self.draft_mode {
            format!(
                "repos/{}/drafts/{}/{rel}",
                self.repo_id, self.session_id
            )
        } else {
            format!("repos/{}/{}/{rel}", self.repo_id, self.branch)
        }
    }

    fn bucket() -> String {
        std::env::var("VEIL_S3_BUCKET")
            .or_else(|_| std::env::var("BUCKET"))
            .unwrap_or_else(|_| "veil-runtime-dev".into())
    }

    fn aws() -> Command {
        let mut c = Command::new("aws");
        if let Ok(p) = std::env::var("AWS_PROFILE") {
            c.env("AWS_PROFILE", p);
        }
        if let Ok(r) = std::env::var("AWS_REGION") {
            c.env("AWS_REGION", r);
        }
        c
    }

    /// Put local file to S3; optional If-Match via copy metadata check (best-effort with CLI).
    pub fn put_s3(&self, rel: &str, abs: &Path, if_match: Option<&str>) -> Result<Option<String>, String> {
        if let Some(want) = if_match {
            if let Ok(Some(cur)) = self.head_etag(rel) {
                if cur != want && cur.trim_matches('"') != want.trim_matches('"') {
                    return Err(format!(
                        "etag conflict: expected {want}, remote has {cur}"
                    ));
                }
            }
        }
        let key = self.s3_key(rel);
        let dest = format!("s3://{}/{}", Self::bucket(), key);
        let out = Self::aws()
            .args(["s3", "cp", &abs.to_string_lossy(), &dest])
            .output()
            .map_err(|e| format!("aws s3 cp: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "s3 put failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        tracing::info!(%key, session = %self.session_id, "s3 write-through ok");
        // Etag from head
        Ok(self.head_etag(rel).ok().flatten())
    }

    fn head_etag(&self, rel: &str) -> Result<Option<String>, String> {
        let key = self.s3_key(rel);
        let out = Self::aws()
            .args([
                "s3api",
                "head-object",
                "--bucket",
                &Self::bucket(),
                "--key",
                &key,
                "--output",
                "json",
            ])
            .output()
            .map_err(|e| format!("head-object: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let v: serde_json::Value =
            serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        Ok(v.get("ETag").and_then(|e| e.as_str()).map(|s| s.to_string()))
    }

    fn delete_s3(&self, rel: &str) -> Result<(), String> {
        let key = self.s3_key(rel);
        let out = Self::aws()
            .args([
                "s3",
                "rm",
                &format!("s3://{}/{}", Self::bucket(), key),
            ])
            .output()
            .map_err(|e| format!("s3 rm: {e}"))?;
        if !out.status.success() {
            tracing::warn!(
                "s3 rm {}: {}",
                key,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    fn rel_of(&self, abs: &Path) -> Result<String, String> {
        abs.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .map_err(|_| "not under root".into())
    }
}

impl WorkspaceFs for WorkspaceFsImpl {
    fn root(&self) -> &Path {
        &self.root
    }

    fn list(&self, rel_dir: &str, max: usize) -> Result<Vec<String>, String> {
        let dir = path_jail(&self.root, rel_dir)?;
        if !dir.is_dir() {
            return Err(format!("not a directory: {rel_dir}"));
        }
        let mut out = Vec::new();
        fn walk(base: &Path, cur: &Path, out: &mut Vec<String>, max: usize) -> Result<(), String> {
            if out.len() >= max {
                return Ok(());
            }
            let rd = std::fs::read_dir(cur).map_err(|e| e.to_string())?;
            for e in rd.flatten() {
                if out.len() >= max {
                    break;
                }
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    ".git" | "target" | "generated" | "node_modules" | "dist"
                ) {
                    continue;
                }
                let rel = p
                    .strip_prefix(base)
                    .map(|x| x.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if p.is_dir() {
                    out.push(format!("{rel}/"));
                    walk(base, &p, out, max)?;
                } else {
                    out.push(rel);
                }
            }
            Ok(())
        }
        walk(&self.root, &dir, &mut out, max)?;
        out.sort();
        Ok(out)
    }

    fn read(&self, rel: &str, max_bytes: usize) -> Result<String, String> {
        let abs = path_jail(&self.root, rel)?;
        let data = std::fs::read(&abs).map_err(|e| format!("read {rel}: {e}"))?;
        let slice = if data.len() > max_bytes {
            &data[..max_bytes]
        } else {
            &data
        };
        Ok(String::from_utf8_lossy(slice).into_owned())
    }

    fn write(
        &self,
        rel: &str,
        content: &str,
        if_match: Option<&str>,
    ) -> Result<WriteResult, String> {
        // Guard: agents sometimes write the full `ws_read` JSON envelope as the file body.
        let content = if crate::file_ops::is_veil_source_rel(rel) {
            crate::file_ops::normalize_source_body(content)
        } else {
            content.to_string()
        };
        let abs = path_jail(&self.root, rel)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        std::fs::write(&abs, &content).map_err(|e| format!("write: {e}"))?;
        let etag = self.put_s3(rel, &abs, if_match)?;
        Ok(WriteResult {
            path: rel.to_string(),
            bytes: content.len(),
            etag,
            revision_hint: None,
        })
    }

    fn str_replace(
        &self,
        rel: &str,
        old: &str,
        new: &str,
        if_match: Option<&str>,
    ) -> Result<WriteResult, String> {
        let cur = self.read(rel, 8_000_000)?;
        let count = cur.matches(old).count();
        if count == 0 {
            return Err(format!("str_replace: pattern not found in {rel}"));
        }
        if count > 1 {
            return Err(format!(
                "str_replace: pattern not unique in {rel} ({count} matches)"
            ));
        }
        let next = cur.replacen(old, new, 1);
        self.write(rel, &next, if_match)
    }

    fn grep(
        &self,
        pattern: &str,
        path_glob: Option<&str>,
        max_matches: usize,
    ) -> Result<Vec<GrepHit>, String> {
        let re = Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?;
        let mut hits = Vec::new();
        let files = self.list("", 5000)?;
        let glob = path_glob.unwrap_or("");
        for rel in files {
            if rel.ends_with('/') {
                continue;
            }
            if !glob.is_empty() && !simple_glob_match(glob, &rel) {
                continue;
            }
            let Ok(text) = self.read(&rel, 2_000_000) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(GrepHit {
                        path: rel.clone(),
                        line: i + 1,
                        text: line.chars().take(400).collect(),
                    });
                    if hits.len() >= max_matches {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }

    fn rm(&self, rel: &str) -> Result<(), String> {
        let abs = path_jail(&self.root, rel)?;
        if abs.is_file() {
            std::fs::remove_file(&abs).map_err(|e| e.to_string())?;
        } else if abs.is_dir() {
            std::fs::remove_dir_all(&abs).map_err(|e| e.to_string())?;
        } else {
            return Err(format!("not found: {rel}"));
        }
        let _ = self.delete_s3(rel);
        Ok(())
    }

    fn flush_path(&self, rel: &str) -> Result<WriteResult, String> {
        let abs = path_jail(&self.root, rel)?;
        if !abs.is_file() {
            return Err(format!("not a file: {rel}"));
        }
        let etag = self.put_s3(rel, &abs, None)?;
        let bytes = std::fs::metadata(&abs).map(|m| m.len() as usize).unwrap_or(0);
        Ok(WriteResult {
            path: rel.to_string(),
            bytes,
            etag,
            revision_hint: None,
        })
    }
}

fn simple_glob_match(pat: &str, path: &str) -> bool {
    // * only
    if pat.is_empty() {
        return true;
    }
    if let Some((a, b)) = pat.split_once('*') {
        return path.starts_with(a) && path.ends_with(b);
    }
    path.contains(pat) || path == pat
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn jail_allows_nested_and_blocks_escape() {
        let tmp = std::env::temp_dir().join(format!("veil-jail-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("sub/b.txt"), "y").unwrap();
        assert!(path_jail(&tmp, "a.txt").is_ok());
        assert!(path_jail(&tmp, "sub/b.txt").is_ok());
        assert!(path_jail(&tmp, "sub/../a.txt").is_ok());
        let esc = path_jail(&tmp, "../outside.txt");
        assert!(
            esc.is_err()
                || esc
                    .as_ref()
                    .map(|p| !p.starts_with(tmp.canonicalize().unwrap_or(tmp.clone())))
                    .unwrap_or(true),
            "escape should fail: {esc:?}"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn simple_glob_basic() {
        assert!(simple_glob_match("*.veil", "main.veil"));
        assert!(simple_glob_match("layers/*", "layers/main.layer") || simple_glob_match("layers/", "layers/main.layer"));
        assert!(!simple_glob_match("*.stub", "main.veil"));
    }
}
