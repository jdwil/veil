//! Resolution-point ("search path") registration.
//!
//! Spec: registry-repo-structure-04-search-path-settings.md
//! Decision: Mind Palace `decision-registry-repo-structure`
//!           §"Resolution-Point / Search-Path Registration".
//!
//! A search path is a repo/dir the layer/stub/source resolver treats as a
//! resolution ROOT for `.layer` / `.stub` / package `.veil`. It is tier (a) of
//! the three-tier registry model: "a private repo registered in the runtime as
//! a resolution point — no registry at all".
//!
//! This is DISTINCT from `reference_dirs` ([`crate::reference_fs`]):
//! reference dirs are a READ-ONLY conversion source (never resolved-from);
//! search paths ARE resolved-from. Conflating them would let the resolver pull
//! from read-only conversion trees, which is wrong.
//!
//! The actual filesystem resolution happens in `veil-ir`
//! (`LayerRegistry::resolve_layer_content` and stub/library lookups), driven by
//! the `VEIL_SEARCH_PATHS` env var (veil-ir has no dependency on this crate's
//! config). This module owns config/env merging + usability reporting and
//! produces the env value the resolver consumes (see [`env_value`]).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::{SearchPathEntry, expand_user_path};

/// One registered resolution-point root, with usability reporting.
#[derive(Debug, Clone)]
pub struct SearchRoot {
    pub id: String,
    pub path: PathBuf,
    pub canon: PathBuf,
    pub exists: bool,
    pub skip_reason: Option<String>,
}

/// Env var the `veil-ir` resolver reads for extra resolution roots.
/// Colon/semicolon/newline separated, each entry optional `name=/abs/path`.
pub const SEARCH_PATHS_ENV: &str = "VEIL_SEARCH_PATHS";

/// Merge config `search_paths` with the `VEIL_SEARCH_PATHS` env overlay and
/// report each root's usability (mirrors `reference_fs::load_roots`).
pub fn load_roots() -> Vec<SearchRoot> {
    let cfg = crate::config::load_config_or_default();
    let env = std::env::var(SEARCH_PATHS_ENV).ok();
    assemble_roots(&cfg.search_paths, env.as_deref())
}

/// Public JSON for `/api/config` (mirrors `reference_fs::public_roots_json`).
pub fn public_roots_json() -> Value {
    roots_to_json(&load_roots())
}

/// Colon-separated `name=/abs/path` value for `VEIL_SEARCH_PATHS`, built from
/// the given config entries. Only USABLE roots (exist + no skip reason) are
/// emitted so the resolver never receives dead/forbidden roots. Absolute,
/// `~`-expanded paths so a child process (`veil` CLI) resolves the same tree.
pub fn env_value(entries: &[SearchPathEntry]) -> String {
    let roots = assemble_roots(entries, None);
    roots
        .iter()
        .filter(|r| r.exists && r.skip_reason.is_none())
        .map(|r| format!("{}={}", r.id, r.canon.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(":")
}

/// Set (or clear) `VEIL_SEARCH_PATHS` in this process so the in-process
/// resolver picks up config changes without a restart. Called after a
/// successful `/api/config` save.
///
/// # Safety
/// `set_var`/`remove_var` are `unsafe` on modern Rust (thread-safety of the
/// environment). ProductHost mutates config on a single request handler; the
/// resolver reads the env synchronously during `veil check`/codegen.
pub fn export_env(entries: &[SearchPathEntry]) {
    let value = env_value(entries);
    unsafe {
        if value.is_empty() {
            std::env::remove_var(SEARCH_PATHS_ENV);
        } else {
            std::env::set_var(SEARCH_PATHS_ENV, value);
        }
    }
}

/// Build usability-reported roots from config entries + optional env overlay.
/// Env entries override config entries with the same id (env wins), matching
/// `reference_fs` semantics.
pub fn assemble_roots(config: &[SearchPathEntry], env: Option<&str>) -> Vec<SearchRoot> {
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
        let skip_reason = usability_reason(&canon, &expanded, exists);
        out.push(SearchRoot {
            id: unique_id(&want_id, &mut used_ids),
            path: expanded,
            canon,
            exists,
            skip_reason,
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

/// Report why a root is unusable (skip reason), or None if usable.
///
/// Refuses filesystem root, `$HOME`, `/tmp`, temp dir, and host staging trees —
/// the resolver MUST NOT scan `$HOME`/`/tmp` (Spec: deterministic, no ambient
/// scanning). A non-existent or non-directory path is reported (not silently
/// dropped) so the UI can show it.
fn usability_reason(canon: &Path, expanded: &Path, exists: bool) -> Option<String> {
    let home = expand_user_path("~");
    if canon == Path::new("/") {
        return Some("refusing filesystem root `/`".into());
    }
    if exists && (canon == home || canon == Path::new("/tmp") || canon == std::env::temp_dir()) {
        return Some(format!("refusing overly broad root {}", canon.display()));
    }
    for c in canon.components() {
        if let std::path::Component::Normal(name) = c
            && matches!(
                name.to_str(),
                Some("veil-ws" | "veil-s3-ws" | "veil-acp-cwd")
            )
        {
            return Some(
                "host staging trees ($TMP/veil-ws, veil-s3-ws, veil-acp-cwd) cannot be search paths"
                    .into(),
            );
        }
    }
    if !exists {
        return Some(format!("{} does not exist or is not a directory", expanded.display()));
    }
    None
}

fn usable_roots(roots: &[SearchRoot]) -> Vec<&SearchRoot> {
    roots
        .iter()
        .filter(|r| r.exists && r.skip_reason.is_none())
        .collect()
}

fn roots_to_json(roots: &[SearchRoot]) -> Value {
    json!({
        "ok": true,
        "resolves_from": true,
        "roots": roots.iter().map(|r| json!({
            "id": r.id,
            "path": r.path.to_string_lossy(),
            "exists": r.exists,
            "usable": r.exists && r.skip_reason.is_none(),
            "skip_reason": r.skip_reason,
        })).collect::<Vec<_>>(),
        "hint": if usable_roots(roots).is_empty() {
            "No usable resolution points. Set VEIL_SEARCH_PATHS=/path/to/repo (colon-separated, optional name=/path) or Config → Search paths. Registered repos supply .layer / .stub / library .veil to consumer projects; local/project resolution still wins."
        } else {
            "Registered resolution points. A `use <name>` in a consumer project resolves against these roots AFTER local/project and [dependencies], BEFORE any remote registry. Distinct from Reference trees (which are read-only, never resolved-from)."
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SearchPathEntry;
    use std::fs;

    fn temp_repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "veil-search-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("layers")).unwrap();
        fs::write(dir.join("layers/ddd.layer"), "layer ddd {}\n").unwrap();
        dir
    }

    #[test]
    fn env_and_config_merge_env_wins() {
        let a = temp_repo("merge-a");
        let b = temp_repo("merge-b");
        let roots = assemble_roots(
            &[SearchPathEntry::Named {
                name: "libs".into(),
                path: a.to_string_lossy().into(),
            }],
            Some(&format!("libs={}", b.display())),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, "libs");
        assert_eq!(roots[0].canon, b.canonicalize().unwrap());
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn missing_dir_is_reported_not_dropped() {
        let roots = assemble_roots(
            &[SearchPathEntry::Path("/no/such/veil/repo".into())],
            None,
        );
        assert_eq!(roots.len(), 1);
        assert!(!roots[0].exists);
        assert!(
            roots[0]
                .skip_reason
                .as_deref()
                .unwrap_or("")
                .contains("does not exist")
        );
    }

    #[test]
    fn refuses_home_tmp_root_and_staging() {
        let home = expand_user_path("~");
        let roots = assemble_roots(
            &[
                SearchPathEntry::Path(home.to_string_lossy().into()),
                SearchPathEntry::Path("/".into()),
                SearchPathEntry::Path("/tmp".into()),
                SearchPathEntry::Named {
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
        assert!(roots.iter().any(|r| r
            .skip_reason
            .as_deref()
            .unwrap_or("")
            .contains("filesystem root")));
        assert!(roots.iter().any(|r| r
            .skip_reason
            .as_deref()
            .unwrap_or("")
            .contains("staging")));
    }

    #[test]
    fn env_value_only_usable_absolute() {
        let a = temp_repo("envval");
        let val = env_value(&[
            SearchPathEntry::Named {
                name: "libs".into(),
                path: a.to_string_lossy().into(),
            },
            SearchPathEntry::Path("/no/such/repo".into()),
        ]);
        // Only the existing root, absolute + canonicalized, id=libs.
        assert!(val.starts_with("libs="), "{val}");
        assert!(val.contains(&a.canonicalize().unwrap().to_string_lossy().to_string()));
        assert!(!val.contains("/no/such/repo"), "{val}");
        let _ = fs::remove_dir_all(&a);
    }

    #[test]
    fn public_json_shape() {
        let a = temp_repo("json");
        let roots = assemble_roots(
            &[SearchPathEntry::Named {
                name: "libs".into(),
                path: a.to_string_lossy().into(),
            }],
            None,
        );
        let v = roots_to_json(&roots);
        assert_eq!(v["ok"], true);
        assert_eq!(v["resolves_from"], true);
        assert_eq!(v["roots"][0]["id"], "libs");
        assert_eq!(v["roots"][0]["usable"], true);
        let _ = fs::remove_dir_all(&a);
    }
}
