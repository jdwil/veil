//! S3-backed project workspace for production-like local/dev (and ECS).
//!
//! When `VEIL_SOURCE_MODE=s3` (strict) or `prefer_s3` (try S3 first):
//! 1. Resolve slug → repo id (DDB META or `VEIL_REPO_MAP`)
//! 2. Materialize `s3://$BUCKET/repos/{id}/{branch}/` → `$TMP/veil-s3-ws/{slug}/`
//! 3. Serve via [`FilesystemProvider`] with **write-through** back to S3
//!
//! Source of truth is the project git origin (S3 bundles or GitHub remote) —
//! not `VEIL_PROJECTS_DIR`. ACP should use MCP `write_source` / structured
//! edits; raw FS tools under monorepo CWD are wrong.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::filesystem::FilesystemProvider;
use super::{FileInfo, FileKind, SourceProvider};
use async_trait::async_trait;
use veil_ir::LayerRegistry;

use crate::project_layout::{collect_project_files, is_source_editable};

/// Strict S3-only vs try-S3-then-disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeSourceMode {
    Disk,
    PreferS3,
    S3,
    /// Machine-local catalog (SQLite/JSON) + GitHub origin. No DashLX DDB/S3.
    Local,
}

pub fn ide_source_mode() -> IdeSourceMode {
    if crate::config::platform_local() {
        return IdeSourceMode::Local;
    }
    match std::env::var("VEIL_SOURCE_MODE")
        .unwrap_or_else(|_| "prefer_s3".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "s3" | "remote" | "object" | "object_store" => IdeSourceMode::S3,
        "prefer_s3" | "prefer-s3" | "hybrid" => IdeSourceMode::PreferS3,
        "disk" | "fs" | "filesystem" => IdeSourceMode::Disk,
        "local" | "personal" => IdeSourceMode::Local,
        other => {
            tracing::warn!(%other, "unknown VEIL_SOURCE_MODE; using prefer_s3");
            IdeSourceMode::PreferS3
        }
    }
}

fn bucket() -> String {
    std::env::var("VEIL_S3_BUCKET")
        .or_else(|_| std::env::var("BUCKET"))
        .unwrap_or_else(|_| "veil-runtime-dev".into())
}

fn ddb_table() -> String {
    std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into())
}

fn branch() -> String {
    std::env::var("VEIL_SOURCE_BRANCH").unwrap_or_else(|_| "main".into())
}

/// Delete S3 checkout + git origin and leftover DDB `REPO#{id}` items.
/// Call after catalog META delete so the store does not accumulate e2e junk.
pub fn purge_repo_store(repo_id: &str) -> Result<(), String> {
    if repo_id.is_empty() || repo_id.contains('/') || repo_id.contains("..") {
        return Err("invalid repo_id".into());
    }
    let b = bucket();
    let table = ddb_table();
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-west-2".into());
    for prefix in [
        format!("s3://{b}/repos/{repo_id}/"),
        format!("s3://{b}/git/{repo_id}/"),
    ] {
        let status = aws_base()
            .args(["s3", "rm", &prefix, "--recursive", "--region", &region])
            .status()
            .map_err(|e| format!("s3 rm {prefix}: {e}"))?;
        if !status.success() {
            tracing::warn!(%prefix, %status, "purge_repo_store s3 rm non-zero (may already be gone)");
        }
    }
    // Remaining SKs under REPO#{id} (BRANCH#, …) after META delete.
    let q = aws_base()
        .args([
            "dynamodb",
            "query",
            "--table-name",
            &table,
            "--region",
            &region,
            "--key-condition-expression",
            "PK = :p",
            "--expression-attribute-values",
            &format!("{{\":p\":{{\"S\":\"REPO#{repo_id}\"}}}}"),
            "--projection-expression",
            "PK,SK",
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("ddb query REPO#: {e}"))?;
    if q.status.success() {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&q.stdout) {
            if let Some(items) = v.get("Items").and_then(|i| i.as_array()) {
                for it in items {
                    let pk = it.pointer("/PK/S").and_then(|x| x.as_str()).unwrap_or("");
                    let sk = it.pointer("/SK/S").and_then(|x| x.as_str()).unwrap_or("");
                    if pk.is_empty() || sk.is_empty() {
                        continue;
                    }
                    let _ = aws_base()
                        .args([
                            "dynamodb",
                            "delete-item",
                            "--table-name",
                            &table,
                            "--region",
                            &region,
                            "--key",
                            &format!("{{\"PK\":{{\"S\":\"{pk}\"}},\"SK\":{{\"S\":\"{sk}\"}}}}"),
                        ])
                        .status();
                }
            }
        }
    }
    invalidate_identity(None, Some(repo_id));
    Ok(())
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

/// Materialized workspace root for a remote (S3) project slug (`$TMP/veil-s3-ws/{slug}`).
pub fn workspace_root(slug: &str) -> PathBuf {
    let base = std::env::var("VEIL_S3_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("veil-s3-ws"));
    base.join(slug)
}

fn repo_id_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Reverse cache: repo_id → product slug.
fn slug_by_repo_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn cache_identity(slug: &str, repo_id: &str) {
    if let Ok(mut guard) = repo_id_cache().lock() {
        guard.insert(slug.to_string(), repo_id.to_string());
        // Also allow looking up by raw repo id string as if it were a "slug".
        if slug != repo_id {
            guard.insert(repo_id.to_string(), repo_id.to_string());
        }
    }
    if let Ok(mut guard) = slug_by_repo_cache().lock() {
        guard.insert(repo_id.to_string(), slug.to_string());
    }
    forget_missing(slug);
}

fn missing_identity_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Instant>>
{
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Instant>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

const MISSING_TTL: Duration = Duration::from_secs(45);

fn remember_missing(slug: &str) {
    if let Ok(mut g) = missing_identity_cache().lock() {
        g.insert(slug.to_string(), Instant::now());
    }
}

fn forget_missing(slug: &str) {
    if let Ok(mut g) = missing_identity_cache().lock() {
        g.remove(slug);
    }
}

fn is_known_missing(slug: &str) -> bool {
    let Ok(mut g) = missing_identity_cache().lock() else {
        return false;
    };
    match g.get(slug) {
        Some(t) if t.elapsed() < MISSING_TTL => true,
        Some(_) => {
            g.remove(slug);
            false
        }
        None => false,
    }
}

/// Canonical product identity: **one** product slug + repo UUID.
///
/// Callers often pass either the product slug (`agent-registry`) **or** the repo
/// UUID from `/projects/{id}/ide`. Without canonicalization those create *two*
/// sticky mainline sessions / workdirs for the same S3 tree — agent writes land
/// in one, IDE reads the other, and Changes looks empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIdentity {
    /// Product slug from DDB META (preferred for sticky keys + session.slug).
    pub slug: String,
    /// Repo UUID (S3 prefix `repos/{repo_id}/…`).
    pub repo_id: String,
}

fn looks_like_repo_uuid(s: &str) -> bool {
    // 8-4-4-4-12 hex UUID
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    let is_hex = |c: u8| c.is_ascii_hexdigit();
    for (i, &c) in b.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if c != b'-' {
                    return false;
                }
            }
            _ => {
                if !is_hex(c) {
                    return false;
                }
            }
        }
    }
    true
}

/// Fetch product slug from DDB `REPO#{id}/META` (cheap GetItem, not scan).
pub fn lookup_slug_for_repo_id(repo_id: &str) -> Result<Option<String>, String> {
    if let Ok(guard) = slug_by_repo_cache().lock() {
        if let Some(s) = guard.get(repo_id) {
            return Ok(Some(s.clone()));
        }
    }
    let table = ddb_table();
    let key = format!(r##"{{"PK":{{"S":"REPO#{repo_id}"}},"SK":{{"S":"META"}}}}"##);
    let out = aws_base()
        .args([
            "dynamodb",
            "get-item",
            "--table-name",
            &table,
            "--key",
            &key,
            "--projection-expression",
            "#d",
            "--expression-attribute-names",
            r##"{"#d":"data"}"##,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws dynamodb get-item: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws dynamodb get-item failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ddb json: {e}"))?;
    let data = v
        .pointer("/Item/data/S")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if data.is_empty() {
        return Ok(None);
    }
    let meta: serde_json::Value =
        serde_json::from_str(data).map_err(|e| format!("repo meta json: {e}"))?;
    let slug = meta
        .get("slug")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ref s) = slug {
        cache_identity(s, repo_id);
    }
    crate::git_origin::register_origin_from_repo_json(repo_id, &meta);
    Ok(slug)
}

/// Resolve any project key (product slug, display name, or repo UUID) to a
/// single [`ProjectIdentity`]. Prefer this over raw `resolve_repo_id` when
/// opening sessions so IDE + agent share one sticky mainline.
pub fn resolve_project_identity(raw: &str) -> Result<ProjectIdentity, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty project identity".into());
    }
    if matches!(ide_source_mode(), IdeSourceMode::Local) {
        if let Some(id) = lookup_local_catalog_identity(raw) {
            return Ok(id);
        }
        return Err(format!(
            "no local catalog repo for '{raw}' — create_project first (GitHub origin)"
        ));
    }
    let key = crate::project_layout::slugify_name(raw);
    if key.is_empty() {
        return Err(format!("invalid project identity: {raw:?}"));
    }

    // Cached: key is repo_id → product slug
    if let Ok(guard) = slug_by_repo_cache().lock() {
        if let Some(slug) = guard.get(&key) {
            return Ok(ProjectIdentity {
                slug: slug.clone(),
                repo_id: key,
            });
        }
    }
    // Cached: key is product slug → repo_id
    if let Ok(guard) = repo_id_cache().lock() {
        if let Some(repo_id) = guard.get(&key) {
            let slug = slug_by_repo_cache()
                .lock()
                .ok()
                .and_then(|g| g.get(repo_id).cloned())
                .unwrap_or_else(|| key.clone());
            return Ok(ProjectIdentity {
                slug,
                repo_id: repo_id.clone(),
            });
        }
    }

    if is_known_missing(&key) {
        return Err(format!(
            "no S3/DDB repo for slug '{key}' — seed with scripts/seed-repo-s3.sh or set VEIL_REPO_MAP"
        ));
    }

    // UUID path: GetItem REPO#{id}/META → product slug (avoids dual sticky).
    if looks_like_repo_uuid(&key) {
        if let Some(slug) = lookup_slug_for_repo_id(&key)? {
            cache_identity(&slug, &key);
            return Ok(ProjectIdentity { slug, repo_id: key });
        }
        // META missing but S3 tree may exist — degrade to id-as-slug.
        let repo_id = resolve_repo_id_uncached(&key)?;
        cache_identity(&repo_id, &repo_id);
        return Ok(ProjectIdentity {
            slug: repo_id.clone(),
            repo_id,
        });
    }

    // Product slug / name path.
    let repo_id = resolve_repo_id_uncached(&key)?;
    // Keep caller's product slug (key) unless META has a better one for this repo.
    let slug = lookup_slug_for_repo_id(&repo_id)?
        .filter(|s| !s.is_empty())
        .unwrap_or(key);
    cache_identity(&slug, &repo_id);
    Ok(ProjectIdentity { slug, repo_id })
}

/// Resolve repo UUID for a project slug **or** repo UUID key.
pub fn resolve_repo_id(slug: &str) -> Result<String, String> {
    Ok(resolve_project_identity(slug)?.repo_id)
}

fn resolve_repo_id_uncached(slug: &str) -> Result<String, String> {
    // Explicit map: relay=cfb3…,foo=…
    if let Ok(map) = std::env::var("VEIL_REPO_MAP") {
        for part in map.split(',') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                if k.trim() == slug {
                    return Ok(v.trim().to_string());
                }
            }
        }
    }
    // Process-local cache (DDB scan is expensive on every IDE open)
    if let Ok(guard) = repo_id_cache().lock() {
        if let Some(id) = guard.get(slug) {
            return Ok(id.clone());
        }
    }
    // DDB scan META rows for slug in JSON data
    let table = ddb_table();
    let out = aws_base()
        .args([
            "dynamodb",
            "scan",
            "--table-name",
            &table,
            "--projection-expression",
            "PK,SK,#d",
            "--expression-attribute-names",
            r##"{"#d":"data"}"##,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws dynamodb scan: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws dynamodb scan failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ddb json: {e}"))?;
    let items = v
        .get("Items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    for item in items {
        let sk = item.pointer("/SK/S").and_then(|s| s.as_str()).unwrap_or("");
        if sk != "META" {
            continue;
        }
        let pk = item.pointer("/PK/S").and_then(|s| s.as_str()).unwrap_or("");
        let data = item
            .pointer("/data/S")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if data.is_empty() {
            continue;
        }
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(data) {
            let s = meta.get("slug").and_then(|x| x.as_str()).unwrap_or("");
            let id_in_meta = meta
                .pointer("/id/value")
                .and_then(|x| x.as_str())
                .or_else(|| meta.get("id").and_then(|x| x.as_str()))
                .unwrap_or("");
            // Match product slug **or** repo UUID key.
            let pk_id = pk.strip_prefix("REPO#").unwrap_or("");
            if s == slug || pk_id == slug || id_in_meta == slug {
                if let Some(id) = pk.strip_prefix("REPO#") {
                    let id = id.to_string();
                    let product_slug = if s.is_empty() {
                        slug.to_string()
                    } else {
                        s.to_string()
                    };
                    cache_identity(&product_slug, &id);
                    return Ok(id);
                }
            }
        }
    }
    // Fallback: treat slug as repo id segment if S3 prefix exists
    let b = bucket();
    let br = branch();
    let probe = format!("s3://{b}/repos/{slug}/{br}/veil.toml");
    let probe2 = format!("s3://{b}/repos/{slug}/{br}/main.veil");
    for p in [probe, probe2] {
        let st = aws_base()
            .args(["s3", "ls", &p])
            .output()
            .map_err(|e| format!("aws s3 ls: {e}"))?;
        if st.status.success() && !st.stdout.is_empty() {
            cache_identity(slug, slug);
            return Ok(slug.to_string());
        }
    }
    remember_missing(slug);
    Err(format!(
        "no S3/DDB repo for slug '{slug}' — seed with scripts/seed-repo-s3.sh or set VEIL_REPO_MAP"
    ))
}

/// Strict remote mode: never write product source under `VEIL_PROJECTS_DIR`.
pub fn allow_disk_project_create() -> bool {
    matches!(
        ide_source_mode(),
        IdeSourceMode::Disk | IdeSourceMode::PreferS3
    )
}

/// Repos from the personal catalog (`.veil-meta.json`). No AWS.
pub fn list_local_catalog_projects() -> Result<Vec<crate::project_layout::ProjectInfo>, String> {
    let repos = load_local_catalog_repos();
    let mut out: Vec<crate::project_layout::ProjectInfo> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for repo in repos {
        let slug = repo
            .get("slug")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if slug.is_empty() || !seen.insert(slug.clone()) {
            continue;
        }
        let id = repo
            .pointer("/id/value")
            .or_else(|| repo.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        crate::git_origin::register_origin_from_repo_json(id, &repo);
        if !id.is_empty() {
            cache_identity(&slug, id);
        }
        let origin = repo.get("origin");
        let path = crate::git_origin::remote_config_from_json(origin)
            .map(|c| format!("github:{}", c.repo))
            .unwrap_or_else(|| format!("local:{slug}"));
        out.push(crate::project_layout::ProjectInfo {
            name: slug,
            path,
            is_git: true,
            package_count: 0,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn load_local_catalog_repos() -> Vec<serde_json::Value> {
    let path = crate::config::local_catalog_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(map) = v.get("repos").and_then(|r| r.as_object()) else {
        return Vec::new();
    };
    map.values().cloned().collect()
}

fn lookup_local_catalog_identity(raw: &str) -> Option<ProjectIdentity> {
    let needle = raw.trim();
    if needle.is_empty() {
        return None;
    }
    let slug_key = crate::project_layout::slugify_name(needle);
    for repo in load_local_catalog_repos() {
        let id = repo
            .pointer("/id/value")
            .or_else(|| repo.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let slug = repo.get("slug").and_then(|s| s.as_str()).unwrap_or("");
        let name = repo.get("name").and_then(|s| s.as_str()).unwrap_or("");
        if id == needle
            || slug == needle
            || slug == slug_key
            || crate::project_layout::slugify_name(name) == slug_key
        {
            if id.is_empty() || slug.is_empty() {
                continue;
            }
            crate::git_origin::register_origin_from_repo_json(id, &repo);
            cache_identity(slug, id);
            return Some(ProjectIdentity {
                slug: slug.to_string(),
                repo_id: id.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod identity_tests {
    use super::{ProjectIdentity, looks_like_repo_uuid};

    #[test]
    fn uuid_shape() {
        assert!(looks_like_repo_uuid("328d843b-a853-4ff8-a3cf-0a28e4747c18"));
        assert!(!looks_like_repo_uuid("agent-registry"));
        assert!(!looks_like_repo_uuid("328d843b-a853-4ff8-a3cf-0a28e4747c1")); // short
        assert!(!looks_like_repo_uuid("not-a-uuid-at-all-xxxxxxxxxxxxxxx"));
    }

    #[test]
    fn identity_eq() {
        let a = ProjectIdentity {
            slug: "agent-registry".into(),
            repo_id: "328d843b-a853-4ff8-a3cf-0a28e4747c18".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}

/// Put a text object at `repos/{repo_id}/{branch}/{rel_path}` (stdin → `aws s3 cp -`).
///
/// Does **not** touch the projects hub directory — durable store only.
pub fn put_repo_text(repo_id: &str, rel_path: &str, content: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;

    let rel = rel_path.trim_start_matches('/').replace('\\', "/");
    if rel.is_empty() || rel.contains("..") {
        return Err(format!("invalid relative path: {rel_path}"));
    }
    let key = format!("repos/{}/{}/{}", repo_id, branch(), rel);
    let dest = format!("s3://{}/{}", bucket(), key);
    let mut child = aws_base()
        .args(["s3", "cp", "-", &dest])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("aws s3 cp spawn: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "aws s3 cp: no stdin".to_string())?;
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| format!("aws s3 cp write stdin: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("aws s3 cp wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 cp - → {dest} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    tracing::info!(%key, bytes = content.len(), "s3 put scaffold ok");
    Ok(())
}

/// Seed INIT scaffold into S3 for a new repo (DDB META must already exist).
///
/// Writes only to object storage (+ process-local id cache). Never creates
/// `{VEIL_PROJECTS_DIR}/{name}` — materialize uses `$TMP/veil-s3-ws` on open.
///
/// `name` may be a display title; scaffold uses slug for package identity.
pub fn seed_new_repo_scaffold(repo_id: &str, name: &str) -> Result<Vec<String>, String> {
    if repo_id.trim().is_empty() {
        return Err("seed_new_repo_scaffold: empty repo_id".into());
    }
    let files = crate::project_layout::scaffold_file_contents(name)?;
    let mut written = Vec::new();
    let skip_s3 = matches!(ide_source_mode(), IdeSourceMode::Local);
    for (rel, content) in files {
        if !skip_s3 {
            put_repo_text(repo_id, &rel, &content)?;
        }
        written.push(rel);
    }
    let slug = crate::project_layout::slugify_name(name);
    // Prefer the new repo for this slug (overwrite any stale cache / missing TTL).
    cache_identity(&slug, repo_id);
    cache_identity(name, repo_id);
    if crate::git_origin::origin_enabled() {
        let origin = crate::git_origin::GitOrigin::for_repo(repo_id);
        // Subpath-bound project on a (possibly shared) repo: seed only the
        // subdir with a FRESH commit; never graft or touch sibling subpaths.
        if origin.subpath().is_some() {
            let files = crate::project_layout::scaffold_file_contents(name)?;
            match origin.seed_subpath(&files, &branch()) {
                Ok(_) => {}
                Err(e) => {
                    if origin.is_git_remote() {
                        return Err(format!("git subpath seed failed: {e}"));
                    }
                    tracing::warn!(%repo_id, error = %e, "subpath seed after scaffold failed");
                }
            }
            tracing::info!(%repo_id, %slug, display = %name, "seed_new_repo_scaffold (subpath) ok");
            return Ok(written);
        }
        let tmp = std::env::temp_dir().join(format!("veil-git-seed-{repo_id}"));
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = (|| -> Result<(), String> {
            for (rel, content) in crate::project_layout::scaffold_file_contents(name)? {
                let p = tmp.join(&rel);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("mkdir seed {}: {e}", parent.display()))?;
                }
                std::fs::write(&p, content)
                    .map_err(|e| format!("write seed {}: {e}", p.display()))?;
            }
            crate::git_origin::GitOrigin::for_repo(repo_id).ensure_from_workdir(&tmp, &branch())?;
            Ok(())
        })() {
            let _ = std::fs::remove_dir_all(&tmp);
            if crate::git_origin::GitOrigin::for_repo(repo_id).is_git_remote() {
                return Err(format!("git remote origin init failed: {e}"));
            }
            tracing::warn!(%repo_id, error = %e, "git origin init after scaffold failed");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
    tracing::info!(%repo_id, %slug, display = %name, "seed_new_repo_scaffold ok");
    Ok(written)
}

/// Drop cached slug→repo_id mapping (e.g. before re-create).
pub fn invalidate_repo_id_cache(slug: &str) {
    if let Ok(mut guard) = repo_id_cache().lock() {
        guard.remove(slug);
        // Also drop display-name keys that slugify to the same id
        let drop_keys: Vec<String> = guard
            .keys()
            .filter(|k| crate::project_layout::slugify_name(k) == slug || k.as_str() == slug)
            .cloned()
            .collect();
        for k in drop_keys {
            guard.remove(&k);
        }
    }
}

/// Drop both slug→repo and repo→slug cache entries after delete / re-create.
pub fn invalidate_identity(slug: Option<&str>, repo_id: Option<&str>) {
    if let Some(s) = slug.map(str::trim).filter(|s| !s.is_empty()) {
        invalidate_repo_id_cache(s);
        remember_missing(s);
    }
    if let Some(rid) = repo_id.map(str::trim).filter(|s| !s.is_empty()) {
        let mapped = slug_by_repo_cache()
            .lock()
            .ok()
            .and_then(|g| g.get(rid).cloned());
        if let Ok(mut g) = slug_by_repo_cache().lock() {
            g.remove(rid);
        }
        if let Ok(mut g) = repo_id_cache().lock() {
            g.retain(|_, v| v != rid);
        }
        if let Some(s) = mapped {
            invalidate_repo_id_cache(&s);
        }
    }
}

/// Build [`crate::project_layout::ProjectInfo`] rows from DDB META (no materialize).
pub fn list_s3_projects() -> Result<Vec<crate::project_layout::ProjectInfo>, String> {
    let b = bucket();
    let br = branch();
    let pairs = list_s3_slug_ids()?;
    Ok(pairs
        .into_iter()
        .map(|(name, id)| crate::project_layout::ProjectInfo {
            name,
            path: format!("s3://{b}/repos/{id}/{br}/"),
            // Origin existence is probed on open/commit, not on every project list
            // (each exists() is two S3 GETs via aws CLI).
            is_git: true,
            package_count: 0,
        })
        .collect())
}

/// List project slugs available in object store (DDB META).
pub fn list_s3_project_slugs() -> Result<Vec<String>, String> {
    Ok(list_s3_slug_ids()?.into_iter().map(|(s, _)| s).collect())
}

/// (slug, repo_id) from DDB META rows.
fn list_s3_slug_ids() -> Result<Vec<(String, String)>, String> {
    let table = ddb_table();
    let out = aws_base()
        .args([
            "dynamodb",
            "scan",
            "--table-name",
            &table,
            "--projection-expression",
            "PK,SK,#d",
            "--expression-attribute-names",
            r##"{"#d":"data"}"##,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws dynamodb scan: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws dynamodb scan failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ddb json: {e}"))?;
    let mut pairs = Vec::new();
    for item in v
        .get("Items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let sk = item.pointer("/SK/S").and_then(|s| s.as_str()).unwrap_or("");
        if sk != "META" {
            continue;
        }
        let pk = item.pointer("/PK/S").and_then(|s| s.as_str()).unwrap_or("");
        let Some(repo_id) = pk.strip_prefix("REPO#") else {
            continue;
        };
        let data = item
            .pointer("/data/S")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(data) {
            crate::git_origin::register_origin_from_repo_json(repo_id, &meta);
            if let Some(s) = meta.get("slug").and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    pairs.push((s.to_string(), repo_id.to_string()));
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs.dedup_by(|a, b| a.0 == b.0);
    Ok(pairs)
}

/// Import `repos/{id}/{branch}/` into git origin for catalog repos that have
/// no `git/{id}/` yet. Best-effort; logs and continues on individual failures.
pub fn backfill_git_origins() -> Vec<(String, String, Result<String, String>)> {
    if matches!(ide_source_mode(), IdeSourceMode::Local) {
        return Vec::new();
    }
    if !crate::git_origin::origin_enabled() {
        return Vec::new();
    }
    let Ok(pairs) = list_s3_slug_ids() else {
        return Vec::new();
    };
    let br = branch();
    let mut out = Vec::new();
    for (slug, repo_id) in pairs {
        let origin = crate::git_origin::GitOrigin::for_repo(&repo_id);
        if origin.exists() {
            out.push((slug, repo_id, Ok("already".into())));
            continue;
        }
        let r = origin.import_legacy_tree(&br).and_then(|sha| match sha {
            Some(s) => Ok(s),
            None => Err("no legacy tree".into()),
        });
        match &r {
            Ok(sha) => tracing::info!(%slug, %repo_id, %sha, "backfilled git origin"),
            Err(e) => tracing::warn!(%slug, %repo_id, error = %e, "git origin backfill skipped"),
        }
        out.push((slug, repo_id, r));
    }
    out
}

fn s3_prefix(repo_id: &str) -> String {
    format!("repos/{}/{}/", repo_id, branch())
}

/// aws s3 sync → local workdir (deletes local extras so S3 is source of truth).
pub fn materialize_repo(repo_id: &str, work: &Path) -> Result<(), String> {
    materialize_repo_with(repo_id, work, true)
}

/// Incremental sync (no `--delete`) — pull without wiping local-only files.
pub fn materialize_repo_incremental(repo_id: &str, work: &Path) -> Result<(), String> {
    materialize_repo_with(repo_id, work, false)
}

fn materialize_repo_with(repo_id: &str, work: &Path, delete: bool) -> Result<(), String> {
    if crate::git_origin::origin_enabled() {
        let origin = crate::git_origin::GitOrigin::for_repo(repo_id);
        let br = branch();
        let mode = if delete {
            crate::git_origin::CheckoutMode::ResetHard
        } else {
            crate::git_origin::CheckoutMode::FetchKeepDirty
        };
        if origin.exists() {
            return origin.checkout(work, &br, mode).map(|_| ());
        }
        if matches!(ide_source_mode(), IdeSourceMode::Local) {
            return origin.checkout(work, &br, mode).map(|_| ());
        }
        if let Ok(Some(_)) = origin.import_legacy_tree(&br) {
            return origin.checkout(work, &br, mode).map(|_| ());
        }
    }
    if matches!(ide_source_mode(), IdeSourceMode::Local) {
        return Err(format!(
            "local catalog has no git origin for {repo_id} — bind GitHub or recreate the project"
        ));
    }
    let b = bucket();
    let prefix = s3_prefix(repo_id);
    let src = format!("s3://{b}/{prefix}");
    std::fs::create_dir_all(work).map_err(|e| format!("mkdir {}: {e}", work.display()))?;
    let mut args = vec![
        "s3".into(),
        "sync".into(),
        src.clone(),
        work.to_string_lossy().into_owned(),
        "--exact-timestamps".into(),
    ];
    if delete {
        args.push("--delete".into());
    }
    let out = aws_base()
        .args(args.iter().map(|s| s.as_str()))
        .output()
        .map_err(|e| format!("aws s3 sync: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 sync {src} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let has = walkdir_has_veil(work);
    if !has {
        return Err(format!(
            "S3 prefix {src} has no .veil / veil.toml after sync"
        ));
    }
    Ok(())
}

fn walkdir_has_veil(root: &Path) -> bool {
    fn rec(p: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(p) else {
            return false;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    ".git" | "target" | "generated" | "node_modules" | "dist"
                ) {
                    continue;
                }
                if rec(&path) {
                    return true;
                }
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if ext == "veil" || path.file_name().and_then(|n| n.to_str()) == Some("veil.toml") {
                    return true;
                }
            }
        }
        false
    }
    rec(root)
}

fn put_file_s3(
    repo_id: &str,
    work: &Path,
    abs_path: &Path,
    draft_mode: bool,
    session_id: &str,
    subpath: Option<&str>,
) -> Result<Option<String>, String> {
    let rel = abs_path
        .strip_prefix(work)
        .map_err(|_| format!("{} not under workspace", abs_path.display()))?;
    let rel_s = rel.to_string_lossy().replace('\\', "/");
    // Re-prefix subpath so the S3 mirror key is repo-relative (hybrid model).
    let rel_s = match crate::git_origin::normalize_subpath(subpath) {
        Some(sub) => format!("{sub}/{rel_s}"),
        None => rel_s,
    };
    let key = if draft_mode && !session_id.is_empty() {
        format!("repos/{repo_id}/drafts/{session_id}/{rel_s}")
    } else {
        format!("repos/{}/{}/{rel_s}", repo_id, branch())
    };
    let dest = format!("s3://{}/{}", bucket(), key);
    let out = aws_base()
        .args(["s3", "cp", &abs_path.to_string_lossy(), &dest])
        .output()
        .map_err(|e| format!("aws s3 cp: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "aws s3 cp → {dest} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    tracing::info!(%key, "s3 write-through ok");
    // Best-effort etag
    let head = aws_base()
        .args([
            "s3api",
            "head-object",
            "--bucket",
            &bucket(),
            "--key",
            &key,
            "--output",
            "json",
        ])
        .output();
    let etag = head.ok().and_then(|o| {
        if !o.status.success() {
            return None;
        }
        serde_json::from_slice::<serde_json::Value>(&o.stdout)
            .ok()
            .and_then(|v| {
                v.get("ETag")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string())
            })
    });
    Ok(etag)
}

/// Open a project slug from S3 into a temp workspace + write-through provider.
pub fn open_s3_project(
    slug: &str,
    show_core_layers: bool,
) -> Result<Arc<S3WorkspaceProvider>, String> {
    let repo_id = resolve_repo_id(slug)?;
    let work = workspace_root(slug);
    // Always pull S3 main (with --delete) so merge promotions are visible.
    // Ignoring sync errors when warm left the IDE on scaffold after merge.
    materialize_repo(&repo_id, &work).or_else(|e| {
        if walkdir_has_veil(&work) {
            tracing::warn!(%slug, error = %e, "S3 sync failed; using existing workdir");
            Ok(())
        } else {
            Err(e)
        }
    })?;

    let paths = collect_project_files(&work, show_core_layers)
        .map_err(|e| format!("S3 workspace {slug} has no packages after materialize: {e}"))?;
    let entries: Vec<(PathBuf, String, bool)> = paths
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            let editable = is_source_editable(&path, &source);
            (path, source, editable)
        })
        .collect();
    if entries.is_empty() {
        return Err(format!("S3 workspace {slug} empty after materialize"));
    }
    let reg =
        LayerRegistry::for_veil_file(&entries[0].0).unwrap_or_else(|_| LayerRegistry::builtin());
    let inner = FilesystemProvider::with_files_in_project(entries, reg, Some(work.clone()));
    Ok(S3WorkspaceProvider::from_parts(
        Arc::new(inner),
        repo_id,
        work,
        slug.to_string(),
        false,
        String::new(),
    ))
}

/// Filesystem session with S3 write-through.
pub struct S3WorkspaceProvider {
    inner: Arc<FilesystemProvider>,
    repo_id: String,
    work: PathBuf,
    slug: String,
    draft_mode: bool,
    session_id: String,
    /// Repo-relative subpath prefix (hybrid model). `work` is the project root
    /// `<checkout>/<subpath>`; the S3 mirror key must re-prefix `<subpath>/`.
    subpath: Option<String>,
    /// The git checkout root (whole repo). For subpath projects this is the
    /// parent of `work`; materialize/pull must target this, not the subpath.
    checkout_root: PathBuf,
}

impl S3WorkspaceProvider {
    pub fn from_parts(
        inner: Arc<FilesystemProvider>,
        repo_id: String,
        work: PathBuf,
        slug: String,
        draft_mode: bool,
        session_id: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            checkout_root: work.clone(),
            repo_id,
            work,
            slug,
            draft_mode,
            session_id,
            subpath: None,
        })
    }

    /// Set the repo-relative subpath (hybrid model). Builder-style. This also
    /// re-roots `checkout_root` to the parent-of-subpath so materialize targets
    /// the whole repo checkout, while file serving stays under the subpath.
    pub fn with_subpath(self: Arc<Self>, subpath: Option<String>) -> Arc<Self> {
        let sub = crate::git_origin::normalize_subpath(subpath.as_deref());
        // Derive the checkout root by stripping the subpath tail from `work`.
        let checkout_root = match &sub {
            Some(s) => {
                let depth = s.split('/').count();
                let mut root = self.work.clone();
                for _ in 0..depth {
                    root = root.parent().map(|p| p.to_path_buf()).unwrap_or(root);
                }
                root
            }
            None => self.work.clone(),
        };
        Arc::new(Self {
            inner: self.inner.clone(),
            repo_id: self.repo_id.clone(),
            work: self.work.clone(),
            slug: self.slug.clone(),
            draft_mode: self.draft_mode,
            session_id: self.session_id.clone(),
            subpath: sub,
            checkout_root,
        })
    }

    pub fn slug(&self) -> &str {
        &self.slug
    }
    pub fn work_dir(&self) -> &Path {
        &self.work
    }
    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn draft_mode(&self) -> bool {
        self.draft_mode
    }

    /// Refresh the in-memory serve-set after a workdir write (ws_write / git).
    pub fn reload_from_disk(&self, hint: &str) -> Result<(), String> {
        self.inner.reload_from_disk(hint)
    }

    /// Re-sync from S3 (source of truth) into the workspace and rebuild cache.
    pub fn rematerialize(&self) -> Result<(), String> {
        materialize_repo(&self.repo_id, &self.checkout_root)
    }

    /// Soft pull (no --delete).
    pub fn pull_incremental(&self) -> Result<(), String> {
        materialize_repo_incremental(&self.repo_id, &self.checkout_root)
    }

    /// Relative path of `file` (or active file) under the workspace root.
    /// For a subpath project the returned path is **repo-relative** (prefixed
    /// with `<subpath>/`) so S3 baseline lookups hit the right key.
    async fn relative_source_path(&self, file: &str) -> String {
        let files = self.inner.list_files().await;
        let path = if file.is_empty() {
            files
                .iter()
                .find(|f| f.active)
                .or_else(|| files.first())
                .map(|f| PathBuf::from(&f.path))
        } else {
            files
                .iter()
                .find(|f| f.name == file || f.path == file)
                .map(|f| PathBuf::from(&f.path))
        };
        let rel = match path {
            Some(abs) => abs
                .strip_prefix(&self.work)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| {
                    abs.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "main.veil".into())
                }),
            None => {
                if file.is_empty() {
                    "main.veil".into()
                } else {
                    file.to_string()
                }
            }
        };
        match &self.subpath {
            Some(sub) => format!("{sub}/{rel}"),
            None => rel,
        }
    }

    fn session_base_branch(&self) -> Option<String> {
        if self.session_id.is_empty() {
            return None;
        }
        crate::session::SessionManager::global()
            .attach(&self.session_id)
            .ok()
            .and_then(|h| {
                let m = h.snapshot_meta();
                m.base_branch.or_else(|| {
                    if m.draft_mode {
                        Some("main".into())
                    } else {
                        None
                    }
                })
            })
    }
}

/// Fetch `repos/{repo_id}/{branch}/{rel}` text from S3 (best-effort).
fn s3_get_object_text(repo_id: &str, branch_name: &str, rel: &str) -> Option<String> {
    let key = format!(
        "repos/{}/{}/{}",
        repo_id,
        branch_name.trim_matches('/'),
        rel.trim_start_matches('/')
    );
    s3_cat_key(&key)
}

/// Fetch a path from a session commit snapshot prefix.
fn s3_get_commit_snapshot_text(
    repo_id: &str,
    session_id: &str,
    commit_short: &str,
    rel: &str,
) -> Option<String> {
    let key = format!(
        "repos/{}/commits/{}/{}/{}",
        repo_id,
        session_id,
        commit_short,
        rel.trim_start_matches('/')
    );
    s3_cat_key(&key)
}

fn s3_cat_key(key: &str) -> Option<String> {
    let uri = format!("s3://{}/{}", bucket(), key);
    let out = aws_base().args(["s3", "cp", &uri, "-"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[async_trait]
impl SourceProvider for S3WorkspaceProvider {
    async fn list_files(&self) -> Vec<FileInfo> {
        self.inner.list_files().await
    }

    async fn read_source(&self, file: &str) -> Result<String, String> {
        self.inner.read_source(file).await
    }

    async fn write_source(&self, file: &str, content: &str) -> Result<(), String> {
        // Write local materialization first (for layer registry / smoke)
        self.inner.write_source(file, content).await?;
        // Resolve absolute path of active/named file
        let files = self.inner.list_files().await;
        let path = if file.is_empty() {
            files
                .iter()
                .find(|f| f.active)
                .map(|f| PathBuf::from(&f.path))
        } else {
            files
                .iter()
                .find(|f| f.name == file || f.path == file)
                .map(|f| PathBuf::from(&f.path))
        };
        let Some(path) = path else {
            return Err("write-through: could not resolve file path".into());
        };
        // Fail closed: S3 must succeed before we claim durable write.
        put_file_s3(
            &self.repo_id,
            &self.work,
            &path,
            self.draft_mode,
            &self.session_id,
            self.subpath.as_deref(),
        )?;
        Ok(())
    }

    fn registry(&self) -> LayerRegistry {
        self.inner.registry()
    }

    fn is_editable(&self, file: &str) -> bool {
        self.inner.is_editable(file)
    }

    fn file_kind(&self, file: &str) -> FileKind {
        self.inner.file_kind(file)
    }

    fn set_active(&self, index: usize) -> Result<(), String> {
        self.inner.set_active(index)
    }

    async fn baseline_source(&self, file: &str) -> Result<Option<(String, String)>, String> {
        // S3 workspaces are not git checkouts — FilesystemProvider's `git show HEAD`
        // always fails and used to return None → PR Wizard treated every construct
        // as Added (162 false steps) while session_status correctly reported clean.
        let head = match self.inner.read_source(file).await {
            Ok(s) => s,
            Err(e) => return Err(e),
        };
        let rel = self.relative_source_path(file).await;

        // Feature/draft session: baseline is product base branch (usually main).
        if self.draft_mode {
            let base_br = self.session_base_branch().unwrap_or_else(|| branch());
            if let Some(text) = s3_get_object_text(&self.repo_id, &base_br, &rel) {
                return Ok(Some((format!("branch:{base_br}"), text)));
            }
            // Greenfield feature branch — empty base is honest (all adds).
            return Ok(None);
        }

        // Mainline session bound to a coding session.
        if !self.session_id.is_empty() {
            if let Ok(h) = crate::session::SessionManager::global().attach(&self.session_id) {
                // Clean working tree: structural review must be empty (agent is right).
                if !h.has_uncommitted() {
                    return Ok(Some(("session (clean)".into(), head)));
                }
                // Dirty: prefer last named commit snapshot as baseline.
                let meta = h.snapshot_meta();
                if let Some(ref cid) = meta.head_commit {
                    let short = &cid[..8.min(cid.len())];
                    if let Some(text) =
                        s3_get_commit_snapshot_text(&self.repo_id, &self.session_id, short, &rel)
                    {
                        return Ok(Some((format!("commit:{}", &cid[..8.min(cid.len())]), text)));
                    }
                }
            }
        }

        // Fall back: product main on S3 (may equal head after write-through).
        let base_br = branch();
        if let Some(text) = s3_get_object_text(&self.repo_id, &base_br, &rel) {
            return Ok(Some((format!("s3:{base_br}"), text)));
        }

        // Last resort: never invent a phantom full-package add walk when we have head.
        Ok(Some(("working tree".into(), head)))
    }

    async fn reload_from_disk(&self) -> Result<usize, String> {
        // Default reload = refresh local cache only (no full S3 rematerialize).
        // Use pull_incremental / rematerialize explicitly for remote pull/reset.
        let files = self.inner.list_files().await;
        for f in &files {
            let path = PathBuf::from(&f.path);
            if let Ok(src) = std::fs::read_to_string(&path) {
                let _ = self.inner.write_source(&f.name, &src).await;
            }
        }
        Ok(files.len())
    }

    async fn layer_dependents(&self, layer_name: &str) -> Vec<FileInfo> {
        self.inner.layer_dependents(layer_name).await
    }

    fn register_file(
        &self,
        path: PathBuf,
        source: String,
        editable: bool,
    ) -> Result<usize, String> {
        let idx = self
            .inner
            .register_file(path.clone(), source.clone(), editable)?;
        put_file_s3(
            &self.repo_id,
            &self.work,
            &path,
            self.draft_mode,
            &self.session_id,
            self.subpath.as_deref(),
        )?;
        let _ = source;
        Ok(idx)
    }

    fn project_root(&self) -> Option<PathBuf> {
        Some(self.work.clone())
    }
}
