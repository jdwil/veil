//! Read-only access to operator-chosen local trees (conversion source).
//!
//! Product VEIL is still edited only via `write_source` / `create_file`.
//! ACP `fs/*` and `terminal/*` stay refused. These MCP tools are the
//! allowlisted way to *read* existing code the operator listed.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

use crate::config::{ReferenceDirEntry, expand_user_path};
use crate::session::resolve_under_root;

const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "generated",
    "node_modules",
    "dist",
    "__pycache__",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    ".tox",
    "vendor",
    ".idea",
    ".vscode",
    ".veil-ws",
    "build",
    "out",
    ".cargo",
    ".gradle",
    ".svn",
    ".hg",
];

const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "pdf", "zip", "gz", "tgz", "bz2", "xz",
    "7z", "rar", "wasm", "so", "dylib", "a", "o", "class", "jar", "exe", "dll", "woff", "woff2",
    "ttf", "otf", "eot", "mp3", "mp4", "webm", "ogg", "wav", "bin", "dat",
];

const LIST_DEFAULT: usize = 400;
const LIST_HARD: usize = 2000;
const READ_DEFAULT: usize = 200_000;
const READ_HARD: usize = 1_000_000;
const GREP_DEFAULT: usize = 40;
const GREP_HARD: usize = 200;
const GREP_FILE_MAX: usize = 1_000_000;
const GREP_WALK_MAX: usize = 4000;

#[derive(Debug, Clone)]
pub struct ReferenceRoot {
    pub id: String,
    pub path: PathBuf,
    pub canon: PathBuf,
    pub exists: bool,
    pub skip_reason: Option<String>,
}

pub fn is_reference_tool(name: &str) -> bool {
    matches!(
        name,
        "reference_roots" | "reference_list" | "reference_read" | "reference_grep"
    )
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "reference_roots",
            "description": "List operator-chosen local directories the agent may READ (conversion source). Empty until VEIL_REFERENCE_DIRS or Config → Reference trees is set. READ ONLY — never write these trees. Convert into the bound VEIL product with write_source / create_file.",
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        }),
        json!({
            "name": "reference_list",
            "description": "List files under a configured reference root (read-only). Paths relative to that root. Skips .git/target/node_modules/generated. Use before converting local code into VEIL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": { "type": "string", "description": "Root id from reference_roots, or an absolute path under a configured root" },
                    "path": { "type": "string", "description": "Relative subdirectory (default '')" },
                    "max": { "type": "integer", "description": "Max entries (default 400, cap 2000)" }
                },
                "required": []
            }
        }),
        json!({
            "name": "reference_read",
            "description": "Read a text file under a configured reference root. READ ONLY. Do not write_source this path — copy ideas into the VEIL product with write_source / create_file. Binary files are refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": { "type": "string", "description": "Root id, or omit if path is absolute under a configured root" },
                    "path": { "type": "string", "description": "Path relative to root, or absolute path under a configured root" },
                    "max_bytes": { "type": "integer", "description": "Max bytes (default 200000, cap 1000000)" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "reference_grep",
            "description": "Regex search under a configured reference root (read-only). Not for product VEIL (use ws_grep / read_source) and not for SDK stubs (use stub_search).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": { "type": "string", "description": "Root id from reference_roots" },
                    "pattern": { "type": "string", "description": "Rust regex" },
                    "path": { "type": "string", "description": "Optional relative subdirectory or glob filter" },
                    "max_matches": { "type": "integer", "description": "Max hits (default 40, cap 200)" }
                },
                "required": ["pattern"]
            }
        }),
    ]
}

pub fn dispatch(tool_name: &str, arguments: &Value) -> Result<String, String> {
    dispatch_with(&load_roots(), tool_name, arguments)
}

pub fn load_roots() -> Vec<ReferenceRoot> {
    let cfg = crate::config::load_config_or_default();
    let env = std::env::var("VEIL_REFERENCE_DIRS").ok();
    assemble_roots(&cfg.reference_dirs, env.as_deref())
}

pub fn public_roots_json() -> Value {
    roots_to_json(&load_roots())
}

pub fn assemble_roots(config: &[ReferenceDirEntry], env: Option<&str>) -> Vec<ReferenceRoot> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for e in config {
        entries.push(e.to_pair());
    }
    if let Some(raw) = env {
        for part in split_env(raw) {
            if let Some(pair) = parse_named_path(&part) {
                if let Some(existing) = entries.iter_mut().find(|(id, _)| id == &pair.0) {
                    existing.1 = pair.1;
                } else {
                    entries.push(pair);
                }
            }
        }
    }
    let mut used_ids: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for (want_id, raw_path) in entries {
        let expanded = expand_user_path(&raw_path);
        let exists = expanded.is_dir();
        let canon = if exists {
            expanded.canonicalize().unwrap_or_else(|_| expanded.clone())
        } else {
            expanded.clone()
        };
        if let Some(reason) = forbidden_reason(&canon, exists) {
            out.push(ReferenceRoot {
                id: unique_id(&want_id, &mut used_ids),
                path: expanded,
                canon,
                exists,
                skip_reason: Some(reason),
            });
            continue;
        }
        let id = unique_id(&want_id, &mut used_ids);
        out.push(ReferenceRoot {
            id,
            path: expanded,
            canon,
            exists,
            skip_reason: None,
        });
    }
    out
}

fn split_env(raw: &str) -> Vec<String> {
    raw.split([':', ';', '\n'])
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_named_path(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((name, path)) = s.split_once('=') {
        let name = name.trim();
        let path = path.trim();
        if is_root_id(name) && !path.is_empty() {
            return Some((name.to_string(), path.to_string()));
        }
    }
    let path = s.to_string();
    let id = Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .map(sanitize_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "root".into());
    Some((id, path))
}

fn is_root_id(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && s.len() <= 64
}

fn sanitize_id(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn unique_id(want: &str, used: &mut Vec<String>) -> String {
    let base = if is_root_id(want) {
        want.to_string()
    } else {
        let s = sanitize_id(want);
        if s.is_empty() { "root".into() } else { s }
    };
    let mut id = base.clone();
    let mut n = 2;
    while used.iter().any(|u| u == &id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    used.push(id.clone());
    id
}

fn forbidden_reason(canon: &Path, exists: bool) -> Option<String> {
    let home = expand_user_path("~");
    if canon == Path::new("/") {
        return Some("refusing filesystem root `/`".into());
    }
    if exists && (canon == home || canon == Path::new("/tmp") || canon == std::env::temp_dir()) {
        return Some(format!("refusing overly broad root {}", canon.display()));
    }
    for c in canon.components() {
        if let std::path::Component::Normal(name) = c {
            if matches!(
                name.to_str(),
                Some("veil-ws" | "veil-s3-ws" | "veil-acp-cwd")
            ) {
                return Some(
                    "host staging trees ($TMP/veil-ws, veil-s3-ws, veil-acp-cwd) cannot be reference roots"
                        .into(),
                );
            }
        }
    }
    None
}

fn usable_roots(roots: &[ReferenceRoot]) -> Vec<&ReferenceRoot> {
    roots
        .iter()
        .filter(|r| r.exists && r.skip_reason.is_none())
        .collect()
}

fn roots_to_json(roots: &[ReferenceRoot]) -> Value {
    json!({
        "ok": true,
        "read_only": true,
        "roots": roots.iter().map(|r| json!({
            "id": r.id,
            "path": r.path.to_string_lossy(),
            "exists": r.exists,
            "usable": r.exists && r.skip_reason.is_none(),
            "skip_reason": r.skip_reason,
        })).collect::<Vec<_>>(),
        "hint": if usable_roots(roots).is_empty() {
            "No usable reference directories. Set VEIL_REFERENCE_DIRS=/path/to/existing/code (colon-separated, optional name=/path) or Config → Reference trees. Dropping a file on AgentDock also works for one-off docs. READ ONLY — convert into VEIL with write_source."
        } else {
            "READ ONLY. Use reference_list / reference_read / reference_grep on these roots, then write_source into the bound VEIL product. Never edit these trees."
        }
    })
}

fn arg_str(arguments: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = arguments.get(*k).and_then(|v| v.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn clamp(n: Option<usize>, default: usize, hard: usize) -> usize {
    n.unwrap_or(default).clamp(1, hard)
}

fn no_roots_payload() -> String {
    json!({
        "ok": false,
        "read_only": true,
        "error": "no_roots",
        "roots": [],
        "hint": "No reference directories configured. Set VEIL_REFERENCE_DIRS or Config → Reference trees (one path per line, optional name=/abs/path). Then call reference_roots. Do not grep $HOME, /tmp, or $TMP/veil-ws."
    })
    .to_string()
}

fn find_root<'a>(roots: &'a [ReferenceRoot], key: &str) -> Result<&'a ReferenceRoot, String> {
    let usable = usable_roots(roots);
    if let Some(r) = usable.iter().find(|r| r.id == key) {
        return Ok(*r);
    }
    let expanded = expand_user_path(key);
    let canon = expanded.canonicalize().unwrap_or(expanded);
    if let Some(r) = usable.iter().find(|r| r.canon == canon || r.path == canon) {
        return Ok(*r);
    }
    if let Some(r) = usable.iter().find(|r| canon.starts_with(&r.canon)) {
        return Ok(*r);
    }
    if let Some(r) = roots.iter().find(|r| r.id == key) {
        if let Some(reason) = &r.skip_reason {
            return Err(format!("reference root `{}` is not usable: {reason}", r.id));
        }
        return Err(format!("reference root `{}` does not exist on disk", r.id));
    }
    Err(format!(
        "unknown reference root `{key}`. Call reference_roots first."
    ))
}

/// Resolve (root, absolute path, relative path) for a tool call.
fn locate(
    roots: &[ReferenceRoot],
    root_key: Option<&str>,
    path: Option<&str>,
) -> Result<(PathBuf, PathBuf, String), String> {
    let usable = usable_roots(roots);
    if usable.is_empty() {
        return Err("no_roots".into());
    }
    let path = path.unwrap_or("").trim();
    if path.starts_with('/') || path.starts_with('~') {
        let expanded = expand_user_path(path);
        let abs = if expanded.exists() {
            expanded
                .canonicalize()
                .map_err(|e| format!("canonicalize: {e}"))?
        } else {
            expanded
        };
        let root = if let Some(k) = root_key {
            find_root(roots, k)?
        } else {
            usable
                .iter()
                .find(|r| abs.starts_with(&r.canon))
                .copied()
                .ok_or_else(|| {
                    "path is not under a configured reference root. Call reference_roots."
                        .to_string()
                })?
        };
        if !abs.starts_with(&root.canon) && abs != root.canon {
            return Err(format!(
                "path escapes reference root `{}` ({})",
                root.id,
                root.path.display()
            ));
        }
        let rel = abs
            .strip_prefix(&root.canon)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        return Ok((root.canon.clone(), abs, rel));
    }
    let root = if let Some(k) = root_key {
        find_root(roots, k)?
    } else if usable.len() == 1 {
        usable[0]
    } else {
        return Err(
            "pass root (id from reference_roots) when more than one reference directory is configured"
                .into(),
        );
    };
    let abs = resolve_under_root(&root.canon, path)
        .map_err(|e| format!("path not under reference root `{}`: {e}", root.id))?;
    let rel = if path.is_empty() {
        String::new()
    } else {
        abs.strip_prefix(&root.canon)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string())
    };
    Ok((root.canon.clone(), abs, rel))
}

pub fn dispatch_with(
    roots: &[ReferenceRoot],
    tool_name: &str,
    arguments: &Value,
) -> Result<String, String> {
    match tool_name {
        "reference_roots" => Ok(roots_to_json(roots).to_string()),
        "reference_list" => {
            if usable_roots(roots).is_empty() {
                return Ok(no_roots_payload());
            }
            let root = arg_str(arguments, &["root", "id"]);
            let path = arg_str(arguments, &["path", "dir"]);
            let max = clamp(
                arguments
                    .get("max")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize),
                LIST_DEFAULT,
                LIST_HARD,
            );
            match locate(roots, root.as_deref(), path.as_deref()) {
                Ok((root_canon, abs, rel)) => {
                    if !abs.is_dir() {
                        return Ok(json!({
                            "ok": false,
                            "error": "not_a_directory",
                            "path": rel,
                            "hint": "reference_list needs a directory"
                        })
                        .to_string());
                    }
                    let files = list_under(&root_canon, &abs, max)?;
                    Ok(json!({
                        "ok": true,
                        "read_only": true,
                        "root": root_canon.to_string_lossy(),
                        "path": rel,
                        "files": files,
                        "truncated": files.len() >= max,
                        "hint": "READ ONLY. Convert into the bound VEIL product with write_source / create_file."
                    })
                    .to_string())
                }
                Err(e) if e == "no_roots" => Ok(no_roots_payload()),
                Err(e) => Ok(json!({ "ok": false, "error": e, "read_only": true }).to_string()),
            }
        }
        "reference_read" => {
            if usable_roots(roots).is_empty() {
                return Ok(no_roots_payload());
            }
            let root = arg_str(arguments, &["root", "id"]);
            let path = arg_str(arguments, &["path", "file"])
                .ok_or_else(|| "reference_read requires path".to_string())?;
            let max = clamp(
                arguments
                    .get("max_bytes")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize),
                READ_DEFAULT,
                READ_HARD,
            );
            match locate(roots, root.as_deref(), Some(&path)) {
                Ok((_, abs, rel)) => match read_text(&abs, max) {
                    Ok((content, truncated, bytes)) => Ok(json!({
                        "ok": true,
                        "read_only": true,
                        "path": rel,
                        "bytes": bytes,
                        "truncated": truncated,
                        "content": content,
                        "hint": "READ ONLY snapshot. Author VEIL with write_source; do not edit this file."
                    })
                    .to_string()),
                    Err(e) => Ok(json!({
                        "ok": false,
                        "error": e,
                        "path": rel,
                        "read_only": true
                    })
                    .to_string()),
                },
                Err(e) if e == "no_roots" => Ok(no_roots_payload()),
                Err(e) => Ok(json!({ "ok": false, "error": e, "read_only": true }).to_string()),
            }
        }
        "reference_grep" => {
            if usable_roots(roots).is_empty() {
                return Ok(no_roots_payload());
            }
            let pattern = arg_str(arguments, &["pattern", "query", "regex"])
                .ok_or_else(|| "reference_grep requires pattern".to_string())?;
            let root = arg_str(arguments, &["root", "id"]);
            let path = arg_str(arguments, &["path", "glob"]);
            let max = clamp(
                arguments
                    .get("max_matches")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize),
                GREP_DEFAULT,
                GREP_HARD,
            );
            let re = Regex::new(&pattern).map_err(|e| format!("invalid regex: {e}"))?;
            match locate(
                roots,
                root.as_deref(),
                path.as_deref().filter(|p| !p.contains('*')),
            ) {
                Ok((root_canon, start, _rel)) => {
                    let glob = path.as_deref().filter(|p| p.contains('*'));
                    let start = if start.is_dir() {
                        start
                    } else {
                        root_canon.clone()
                    };
                    let files = list_under(&root_canon, &start, GREP_WALK_MAX)?;
                    let mut hits = Vec::new();
                    for rel in files {
                        if rel.ends_with('/') {
                            continue;
                        }
                        if let Some(g) = glob {
                            if !simple_glob_match(g, &rel) {
                                continue;
                            }
                        }
                        let abs = match resolve_under_root(&root_canon, &rel) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        if is_probably_binary_path(&abs) {
                            continue;
                        }
                        let Ok(meta) = fs::metadata(&abs) else {
                            continue;
                        };
                        if meta.len() as usize > GREP_FILE_MAX {
                            continue;
                        }
                        let Ok((text, _, _)) = read_text(&abs, GREP_FILE_MAX) else {
                            continue;
                        };
                        for (i, line) in text.lines().enumerate() {
                            if re.is_match(line) {
                                hits.push(json!({
                                    "path": rel,
                                    "line": i + 1,
                                    "text": line.chars().take(400).collect::<String>(),
                                }));
                                if hits.len() >= max {
                                    return Ok(json!({
                                        "ok": true,
                                        "read_only": true,
                                        "pattern": pattern,
                                        "hits": hits,
                                        "truncated": true,
                                        "hint": "READ ONLY. Convert matches into VEIL with write_source."
                                    })
                                    .to_string());
                                }
                            }
                        }
                    }
                    Ok(json!({
                        "ok": true,
                        "read_only": true,
                        "pattern": pattern,
                        "hits": hits,
                        "truncated": false,
                        "hint": "READ ONLY. Convert matches into VEIL with write_source."
                    })
                    .to_string())
                }
                Err(e) if e == "no_roots" => Ok(no_roots_payload()),
                Err(e) => Ok(json!({ "ok": false, "error": e, "read_only": true }).to_string()),
            }
        }
        other => Err(format!("unknown reference tool: {other}")),
    }
}

fn list_under(root: &Path, start: &Path, max: usize) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    fn walk(base: &Path, cur: &Path, out: &mut Vec<String>, max: usize) -> Result<(), String> {
        if out.len() >= max {
            return Ok(());
        }
        let rd = match fs::read_dir(cur) {
            Ok(rd) => rd,
            Err(e) => return Err(e.to_string()),
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if out.len() >= max {
                break;
            }
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if SKIP_DIRS.contains(&name) {
                continue;
            }
            if name.starts_with('.') && p.is_dir() {
                continue;
            }
            let Ok(canon) = p.canonicalize() else {
                continue;
            };
            if !canon.starts_with(base) && canon != *base {
                continue;
            }
            let rel = canon
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
    walk(root, start, &mut out, max)?;
    Ok(out)
}

fn is_probably_binary_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| BINARY_EXT.iter().any(|b| e.eq_ignore_ascii_case(b)))
        .unwrap_or(false)
}

fn read_text(abs: &Path, max_bytes: usize) -> Result<(String, bool, usize), String> {
    if !abs.is_file() {
        return Err(format!("not a file: {}", abs.display()));
    }
    if is_probably_binary_path(abs) {
        return Err("binary file skipped".into());
    }
    let data = fs::read(abs).map_err(|e| format!("read: {e}"))?;
    if data.iter().take(8192).any(|&b| b == 0) {
        return Err("binary file skipped (nul bytes)".into());
    }
    let truncated = data.len() > max_bytes;
    let slice = if truncated { &data[..max_bytes] } else { &data };
    let text = std::str::from_utf8(slice)
        .map_err(|_| "not a UTF-8 text file".to_string())?
        .to_string();
    Ok((text, truncated, data.len()))
}

fn simple_glob_match(pat: &str, path: &str) -> bool {
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
    use crate::config::ReferenceDirEntry;
    use serde_json::json;
    use std::fs;

    fn temp_tree(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "veil-ref-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("src/app.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        fs::write(dir.join("README.md"), "# sample\n").unwrap();
        fs::write(dir.join("node_modules/pkg/index.js"), "secret()\n").unwrap();
        fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        dir
    }

    fn roots_for(dir: &Path, id: &str) -> Vec<ReferenceRoot> {
        assemble_roots(
            &[ReferenceDirEntry::Named {
                name: id.into(),
                path: dir.to_string_lossy().into(),
            }],
            None,
        )
    }

    #[test]
    fn env_named_and_plain_paths() {
        let dir = temp_tree("env");
        let roots = assemble_roots(
            &[],
            Some(&format!(
                "legacy={}:other={}",
                dir.display(),
                dir.join("src").display()
            )),
        );
        assert!(
            roots
                .iter()
                .any(|r| r.id == "legacy" && r.exists && r.skip_reason.is_none())
        );
        assert!(roots.iter().any(|r| r.id == "other"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_overrides_same_id() {
        let a = temp_tree("ov-a");
        let b = temp_tree("ov-b");
        let roots = assemble_roots(
            &[ReferenceDirEntry::Named {
                name: "app".into(),
                path: a.to_string_lossy().into(),
            }],
            Some(&format!("app={}", b.display())),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, "app");
        assert_eq!(roots[0].canon, b.canonicalize().unwrap());
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn refuses_home_tmp_and_host_staging() {
        let home = expand_user_path("~");
        let roots = assemble_roots(
            &[
                ReferenceDirEntry::Path(home.to_string_lossy().into()),
                ReferenceDirEntry::Path("/".into()),
                ReferenceDirEntry::Path("/tmp".into()),
                ReferenceDirEntry::Named {
                    name: "ws".into(),
                    path: std::env::temp_dir()
                        .join("veil-ws/secret")
                        .to_string_lossy()
                        .into(),
                },
            ],
            None,
        );
        assert!(usable_roots(&roots).is_empty(), "{roots:?}");
        assert!(roots.iter().any(|r| {
            r.skip_reason
                .as_deref()
                .unwrap_or("")
                .contains("filesystem root")
        }));
        assert!(roots.iter().any(|r| {
            r.skip_reason
                .as_deref()
                .unwrap_or("")
                .contains("overly broad")
        }));
        assert!(
            roots
                .iter()
                .any(|r| r.skip_reason.as_deref().unwrap_or("").contains("staging"))
        );
    }

    #[test]
    fn list_skips_git_and_node_modules_and_blocks_escape() {
        let dir = temp_tree("list");
        let roots = roots_for(&dir, "sample");
        let out = dispatch_with(&roots, "reference_list", &json!({ "root": "sample" })).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        let files: Vec<String> = v["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
        assert!(files.iter().any(|f| f == "src/app.rs"), "{files:?}");
        assert!(files.iter().any(|f| f == "README.md"), "{files:?}");
        assert!(
            !files.iter().any(|f| f.contains("node_modules")),
            "{files:?}"
        );
        assert!(!files.iter().any(|f| f.contains(".git")), "{files:?}");

        let esc = dispatch_with(
            &roots,
            "reference_read",
            &json!({ "root": "sample", "path": "../outside.txt" }),
        )
        .unwrap();
        let ev: Value = serde_json::from_str(&esc).unwrap();
        assert_eq!(ev["ok"], false, "{esc}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_and_grep() {
        let dir = temp_tree("rg");
        let roots = roots_for(&dir, "sample");
        let read = dispatch_with(
            &roots,
            "reference_read",
            &json!({ "root": "sample", "path": "src/app.rs" }),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["content"].as_str().unwrap().contains("println"));
        assert_eq!(v["read_only"], true);

        let grep = dispatch_with(
            &roots,
            "reference_grep",
            &json!({ "root": "sample", "pattern": "println!" }),
        )
        .unwrap();
        let g: Value = serde_json::from_str(&grep).unwrap();
        assert_eq!(g["ok"], true);
        assert!(
            g["hits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["path"] == "src/app.rs")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn absolute_path_must_be_under_root() {
        let dir = temp_tree("abs");
        let other = temp_tree("abs-other");
        let roots = roots_for(&dir, "sample");
        let ok = dispatch_with(
            &roots,
            "reference_read",
            &json!({ "path": dir.join("README.md").to_string_lossy() }),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(v["ok"], true, "{ok}");

        let bad = dispatch_with(
            &roots,
            "reference_read",
            &json!({ "path": other.join("README.md").to_string_lossy() }),
        )
        .unwrap();
        let b: Value = serde_json::from_str(&bad).unwrap();
        assert_eq!(b["ok"], false, "{bad}");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&other);
    }

    #[test]
    fn empty_roots_is_ok_false_not_a_crash() {
        let out = dispatch_with(&[], "reference_list", &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "no_roots");
        assert!(v["hint"].as_str().unwrap().contains("VEIL_REFERENCE_DIRS"));
    }

    #[test]
    fn tool_definitions_are_read_only() {
        let defs = tool_definitions();
        let names: Vec<_> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(|n| n.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                "reference_roots",
                "reference_list",
                "reference_read",
                "reference_grep"
            ]
        );
        assert!(!names.iter().any(|n| n.contains("write")));
        for d in &defs {
            let desc = d.get("description").and_then(|s| s.as_str()).unwrap_or("");
            assert!(desc.to_ascii_lowercase().contains("read"), "{desc}");
        }
    }

    #[test]
    fn parse_named_and_plain() {
        let p = parse_named_path("legacy=/home/jd/src/app").unwrap();
        assert_eq!(p.0, "legacy");
        assert_eq!(p.1, "/home/jd/src/app");
        let q = parse_named_path("/home/jd/src/my-app").unwrap();
        assert_eq!(q.0, "my-app");
        assert_eq!(q.1, "/home/jd/src/my-app");
    }
}
