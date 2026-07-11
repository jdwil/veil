//! Agent write safety (AGT-013) — path allowlist.

use crate::provider::FileInfo;

/// Allowed write roots from `VEIL_AGENT_ALLOWLIST` (comma-separated paths/prefixes).
/// Empty env → use loaded file paths as the default allowlist.
pub fn allowlist_from_env(loaded: &[FileInfo]) -> Vec<String> {
    if let Ok(raw) = std::env::var("VEIL_AGENT_ALLOWLIST") {
        let custom: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !custom.is_empty() {
            return custom;
        }
    }
    // Default: every loaded package path + basename
    let mut out = Vec::new();
    for f in loaded {
        if !f.path.is_empty() {
            out.push(f.path.clone());
        }
        if !f.name.is_empty() && !out.iter().any(|p| p == &f.name) {
            out.push(f.name.clone());
        }
    }
    out
}

/// Return Ok if `path` (or empty = active) may be written.
pub fn check_write_allowed(path: &str, allowlist: &[String], loaded: &[FileInfo]) -> Result<(), String> {
    if allowlist.is_empty() {
        // No files loaded and no env — deny writes (fail closed)
        return Err(
            "agent write denied: empty allowlist (load .veil files or set VEIL_AGENT_ALLOWLIST)"
                .into(),
        );
    }

    let candidates: Vec<String> = if path.is_empty() {
        loaded
            .iter()
            .filter(|f| f.active)
            .flat_map(|f| [f.path.clone(), f.name.clone()])
            .collect()
    } else {
        vec![path.to_string()]
    };

    if candidates.is_empty() && !path.is_empty() {
        // explicit path not in loaded list — still check allowlist prefixes
        if path_matches(path, allowlist) {
            return Ok(());
        }
        return Err(format!(
            "agent write denied: '{path}' is outside VEIL_AGENT_ALLOWLIST"
        ));
    }

    for c in &candidates {
        if path_matches(c, allowlist) {
            return Ok(());
        }
    }

    // Also allow empty path if any active file is allowlisted
    if path.is_empty() {
        for f in loaded.iter().filter(|f| f.active) {
            if path_matches(&f.path, allowlist) || path_matches(&f.name, allowlist) {
                return Ok(());
            }
        }
    }

    Err(format!(
        "agent write denied: path not in allowlist ({})",
        allowlist.join(", ")
    ))
}

fn path_matches(path: &str, allowlist: &[String]) -> bool {
    let path_norm = path.replace('\\', "/");
    for entry in allowlist {
        let e = entry.replace('\\', "/");
        if e == "*" || e == "**" {
            return true;
        }
        if path_norm == e {
            return true;
        }
        // basename match
        if path_norm.ends_with(&format!("/{e}")) || path_norm.ends_with(&e) && e.contains('/') {
            return true;
        }
        let base = path_norm.rsplit('/').next().unwrap_or(&path_norm);
        if base == e {
            return true;
        }
        // prefix directory
        if path_norm.starts_with(&e)
            || path_norm.starts_with(&format!("{e}/"))
            || e.ends_with('/') && path_norm.starts_with(e.trim_end_matches('/'))
        {
            return true;
        }
        // simple glob suffix *.veil
        if let Some(suf) = e.strip_prefix('*') {
            if path_norm.ends_with(suf) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fi(path: &str, name: &str, active: bool) -> FileInfo {
        FileInfo {
            index: 0,
            name: name.into(),
            path: path.into(),
            editable: true,
            active,
            kind: crate::provider::FileKind::Package,
        }
    }

    #[test]
    fn default_allowlist_uses_loaded_files() {
        let loaded = vec![fi("/proj/a.veil", "a.veil", true)];
        let al = allowlist_from_env(&loaded);
        assert!(al.iter().any(|p| p.contains("a.veil")));
        assert!(check_write_allowed("", &al, &loaded).is_ok());
    }

    #[test]
    fn denies_path_outside_allowlist() {
        let loaded = vec![fi("/proj/a.veil", "a.veil", true)];
        let al = vec!["/proj/a.veil".into()];
        assert!(check_write_allowed("/etc/passwd", &al, &loaded).is_err());
    }

    #[test]
    fn glob_suffix() {
        let al = vec!["*.veil".into()];
        assert!(path_matches("/x/y.veil", &al));
        assert!(!path_matches("/x/y.rs", &al));
    }
}
