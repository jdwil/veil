//! CAP-001: resolve `link` declarations into Cargo dependency lines.
//!
//! Security model:
//! - Absolute paths are rejected.
//! - Allowlisted monorepo crates may omit `path` (default relative path used).
//! - Non-allowlisted crates require an explicit relative `path`.

use veil_ir::ast::LinkDecl;

/// Resolved Cargo dependency for codegen.
#[derive(Debug, Clone)]
pub struct ResolvedLink {
    /// Cargo package name (`veil-server`).
    pub cargo_name: String,
    /// Rust crate identifier for `use` (`veil_server`).
    pub rust_name: String,
    /// Path relative to the generated workspace root.
    pub path: String,
    pub features: Vec<String>,
}

/// Monorepo crates that may be linked without an explicit path.
/// Path is relative to a typical generated workspace two levels under the monorepo
/// (e.g. `runtime/generated` → `../../crates/…`). Override with `path "..."`.
const ALLOWLIST: &[(&str, &str, &str)] = &[
    // (accepted name, cargo package name, default path from gen workspace root)
    ("veil_server", "veil-server", "../../crates/veil-server"),
    ("veil-server", "veil-server", "../../crates/veil-server"),
    ("veil_local", "veil-local", "../../crates/veil-local"),
    ("veil-local", "veil-local", "../../crates/veil-local"),
    ("veil_parser", "veil-parser", "../../crates/veil-parser"),
    ("veil-parser", "veil-parser", "../../crates/veil-parser"),
    ("veil_ir", "veil-ir", "../../crates/veil-ir"),
    ("veil-ir", "veil-ir", "../../crates/veil-ir"),
    ("veil_codegen", "veil-codegen", "../../crates/veil-codegen"),
    ("veil-codegen", "veil-codegen", "../../crates/veil-codegen"),
];

/// Resolve a single link declaration. Returns `Err` with a human message on
/// security / validation failure.
pub fn resolve_link(link: &LinkDecl) -> Result<ResolvedLink, String> {
    let rust_name = to_rust_crate_name(&link.name);
    let cargo_name = allowlist_cargo_name(&link.name)
        .unwrap_or_else(|| to_cargo_package_name(&link.name));

    let path = match &link.path {
        Some(p) => {
            validate_path(p)?;
            p.clone()
        }
        None => {
            if let Some((_, _, default_path)) = ALLOWLIST
                .iter()
                .find(|(n, _, _)| *n == link.name.as_str())
            {
                default_path.to_string()
            } else {
                return Err(format!(
                    "link `{}`: not allowlisted — provide an explicit relative path \
                     (e.g. `link {} path \"../my-crate\"`). Allowlisted: {}",
                    link.name,
                    link.name,
                    allowlist_names_display()
                ));
            }
        }
    };

    Ok(ResolvedLink {
        cargo_name,
        rust_name,
        path,
        features: link.features.clone(),
    })
}

/// Resolve all links; collect errors if any.
pub fn resolve_links(links: &[LinkDecl]) -> Result<Vec<ResolvedLink>, Vec<String>> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for link in links {
        match resolve_link(link) {
            Ok(r) => out.push(r),
            Err(e) => errs.push(e),
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

/// Emit a `[workspace.dependencies]` (or crate-level) path dependency line.
pub fn cargo_dep_line(link: &ResolvedLink) -> String {
    if link.features.is_empty() {
        format!(
            "{} = {{ path = \"{}\" }}\n",
            link.cargo_name, link.path
        )
    } else {
        let feats: Vec<String> = link
            .features
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect();
        format!(
            "{} = {{ path = \"{}\", features = [{}] }}\n",
            link.cargo_name,
            link.path,
            feats.join(", ")
        )
    }
}

/// Emit a crate-level `name.workspace = true` dependency line.
pub fn cargo_workspace_dep_line(link: &ResolvedLink) -> String {
    format!("{}.workspace = true\n", link.cargo_name)
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("link path must not be empty".into());
    }
    if path.contains('\0') {
        return Err("link path must not contain NUL".into());
    }
    // Absolute paths (unix or windows drive).
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(format!(
            "link path `{path}` must be relative (absolute paths are not allowed)"
        ));
    }
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        // C:\... or C:/...
        return Err(format!(
            "link path `{path}` must be relative (absolute paths are not allowed)"
        ));
    }
    Ok(())
}

fn allowlist_cargo_name(name: &str) -> Option<String> {
    ALLOWLIST
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, cargo, _)| (*cargo).to_string())
}

fn allowlist_names_display() -> String {
    let mut names: Vec<&str> = ALLOWLIST.iter().map(|(n, _, _)| *n).collect();
    names.sort();
    names.dedup();
    names.join(", ")
}

fn to_rust_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn to_cargo_package_name(name: &str) -> String {
    name.replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::span::Span;

    fn link(name: &str, path: Option<&str>, features: &[&str]) -> LinkDecl {
        LinkDecl {
            name: name.into(),
            path: path.map(str::to_string),
            features: features.iter().map(|s| s.to_string()).collect(),
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn allowlisted_without_path() {
        let r = resolve_link(&link("veil_server", None, &[])).unwrap();
        assert_eq!(r.cargo_name, "veil-server");
        assert_eq!(r.rust_name, "veil_server");
        assert_eq!(r.path, "../../crates/veil-server");
    }

    #[test]
    fn allowlisted_hyphen_name() {
        let r = resolve_link(&link("veil-local", None, &[])).unwrap();
        assert_eq!(r.cargo_name, "veil-local");
        assert_eq!(r.rust_name, "veil_local");
    }

    #[test]
    fn explicit_path_override() {
        let r = resolve_link(&link(
            "veil_server",
            Some("../../../crates/veil-server"),
            &["full"],
        ))
        .unwrap();
        assert_eq!(r.path, "../../../crates/veil-server");
        assert_eq!(r.features, vec!["full".to_string()]);
        let line = cargo_dep_line(&r);
        assert!(line.contains("features = [\"full\"]"));
        assert!(line.contains("path = \"../../../crates/veil-server\""));
    }

    #[test]
    fn non_allowlisted_requires_path() {
        let err = resolve_link(&link("my_crate", None, &[])).unwrap_err();
        assert!(err.contains("not allowlisted"), "{err}");
    }

    #[test]
    fn non_allowlisted_with_path_ok() {
        let r = resolve_link(&link("my_crate", Some("../vendor/my-crate"), &[])).unwrap();
        assert_eq!(r.cargo_name, "my-crate");
        assert_eq!(r.rust_name, "my_crate");
        assert_eq!(r.path, "../vendor/my-crate");
    }

    #[test]
    fn rejects_absolute_path() {
        let err = resolve_link(&link("x", Some("/etc/passwd"), &[])).unwrap_err();
        assert!(err.contains("relative"), "{err}");
    }
}
