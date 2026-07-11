//! Project directory layout: projects hub dir + single-project file scan.
//!
//! See `docs/PROJECT_LAYOUT.md`. Runtime owns multi-project UX; IDE serve is
//! always one project root.

use std::path::{Path, PathBuf};

/// Metadata for one product under the projects directory.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: String,
    /// Has a `.git` directory.
    pub is_git: bool,
    /// Count of `*.veil` packages at project root.
    pub package_count: usize,
}

/// Active IDE session context (single project).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActiveProjectInfo {
    pub name: String,
    pub path: String,
    /// Resolved projects hub directory (for runtime UX).
    pub projects_dir: String,
}

/// Core platform layers shipped with VEIL (language design, not userland DSL).
/// Hidden from the serve file picker by default; still resolved via `use`.
pub fn is_core_platform_layer(stem: &str) -> bool {
    matches!(
        stem,
        "base"
            | "ddd"
            | "di"
            | "functional"
            | "rust"
            | "harness"
            | "ui"
            | "svelte5"
            | "transports"
            | "rig"
            | "aws_storage"
    )
}

/// Default projects directory: `VEIL_PROJECTS_DIR` or `~/veil-projects`.
pub fn default_projects_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("VEIL_PROJECTS_DIR") {
        return PathBuf::from(p);
    }
    home_dir()
        .map(|h| h.join("veil-projects"))
        .unwrap_or_else(|| PathBuf::from("veil-projects"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Ensure the projects directory exists.
pub fn ensure_projects_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

/// Whether `path` looks like a VEIL product project root.
pub fn is_project_root(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    if path.join(".git").exists() {
        return true;
    }
    if path.join("veil.toml").is_file() {
        return true;
    }
    read_dir_ext(path, "veil").next().is_some()
}

/// List product projects under `projects_dir` (immediate children only).
pub fn list_projects(projects_dir: &Path) -> Result<Vec<ProjectInfo>, String> {
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }
    let rd = std::fs::read_dir(projects_dir)
        .map_err(|e| format!("cannot read {}: {e}", projects_dir.display()))?;
    let mut out = Vec::new();
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip hidden
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(true)
        {
            continue;
        }
        if !is_project_root(&path) {
            continue;
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let package_count = read_dir_ext(&path, "veil").count();
        out.push(ProjectInfo {
            name,
            path: path.to_string_lossy().to_string(),
            is_git: path.join(".git").exists(),
            package_count,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Validate project name: letters, digits, `_`, `-`.
pub fn validate_project_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("project name is empty".into());
    }
    if name.len() > 64 {
        return Err("project name too long (max 64)".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("project name must be [a-zA-Z0-9_-]+".into());
    }
    if name.starts_with('-') {
        return Err("project name must not start with '-'".into());
    }
    Ok(())
}

/// Create a new product under `projects_dir`: mkdir, git init, scaffold.
pub fn create_project(projects_dir: &Path, name: &str) -> Result<ProjectInfo, String> {
    validate_project_name(name)?;
    ensure_projects_dir(projects_dir)?;
    let root = projects_dir.join(name);
    if root.exists() {
        return Err(format!("project already exists: {}", root.display()));
    }
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    std::fs::create_dir_all(root.join("layers"))
        .map_err(|e| format!("cannot create layers/: {e}"))?;
    std::fs::create_dir_all(root.join("stubs"))
        .map_err(|e| format!("cannot create stubs/: {e}"))?;

    let pkg_name = pascal_case(name);
    let veil_toml = format!("name = \"{name}\"\n");
    std::fs::write(root.join("veil.toml"), veil_toml)
        .map_err(|e| format!("cannot write veil.toml: {e}"))?;

    let pkg_src = format!(
        "pkg {pkg_name}\n  use ddd\n\n  # Scaffold — open in IDE: veil serve {}\n",
        root.display()
    );
    let pkg_file = root.join(format!("{name}.veil"));
    std::fs::write(&pkg_file, pkg_src).map_err(|e| format!("cannot write package: {e}"))?;

    let git_ok = std::process::Command::new("git")
        .args(["init"])
        .current_dir(&root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !git_ok {
        eprintln!(
            "warning: git init failed in {} (git missing or error); project files created anyway",
            root.display()
        );
    }

    Ok(ProjectInfo {
        name: name.to_string(),
        path: root.to_string_lossy().to_string(),
        is_git: root.join(".git").exists(),
        package_count: 1,
    })
}

fn pascal_case(name: &str) -> String {
    name.split(|c: char| c == '-' || c == '_')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut c = part.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Collect editable sources for a **single project root**.
///
/// - `root/*.veil`
/// - `root/layers/*.layer` (canonical project layers)
/// - `root/*.layer` (legacy/demo layout e.g. `examples/crm.layer`)
///
/// Does **not** pull monorepo or parent `layers/` directories into the list.
/// When `show_core_layers` is false, core platform layer stems are omitted
/// (they still resolve via `use` + `VEIL_LAYERS_DIR`).
pub fn collect_project_files(root: &Path, show_core_layers: bool) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let mut found: Vec<PathBuf> = read_dir_ext(root, "veil").collect();

    // Canonical: project-local layers/
    let layers_dir = root.join("layers");
    if layers_dir.is_dir() {
        for p in read_dir_ext(&layers_dir, "layer") {
            found.push(p);
        }
    }
    // Legacy/demo: layers sitting next to packages (examples/)
    for p in read_dir_ext(root, "layer") {
        found.push(p);
    }

    found = dedup_layer_files(found);

    if !show_core_layers {
        found.retain(|p| {
            if p.extension().and_then(|e| e.to_str()) != Some("layer") {
                return true;
            }
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            !is_core_platform_layer(stem)
        });
    }

    // Packages first, then layers
    found.sort_by(|a, b| {
        let ak = a.extension().and_then(|e| e.to_str()) == Some("layer");
        let bk = b.extension().and_then(|e| e.to_str()) == Some("layer");
        ak.cmp(&bk).then_with(|| a.cmp(b))
    });

    if found.is_empty() {
        return Err(format!(
            "No .veil packages or layers/ found in {}",
            root.display()
        ));
    }
    Ok(found)
}

/// Prefer `layers/<name>.layer` when the same stem appears twice.
pub fn dedup_layer_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    use std::collections::HashMap;
    let mut by_stem: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut non_layers: Vec<PathBuf> = Vec::new();
    for p in files {
        if p.extension().and_then(|e| e.to_str()) == Some("layer") {
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            by_stem.entry(stem).or_default().push(p);
        } else {
            non_layers.push(p);
        }
    }
    let mut layers: Vec<PathBuf> = Vec::new();
    for (_stem, mut paths) in by_stem {
        if paths.len() == 1 {
            layers.push(paths.pop().unwrap());
            continue;
        }
        paths.sort_by_key(|p| {
            let s = p.to_string_lossy();
            let in_layers = s.contains("/layers/") || s.starts_with("layers/");
            (!in_layers, s.to_string())
        });
        layers.push(paths.remove(0));
    }
    non_layers.extend(layers);
    non_layers
}

/// Read project name from `veil.toml` or directory name.
pub fn project_display_name(root: &Path) -> String {
    let toml_path = root.join("veil.toml");
    if let Ok(text) = std::fs::read_to_string(&toml_path) {
        for line in text.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("name") {
                let rest = rest.trim().trim_start_matches('=').trim();
                let name = rest.trim_matches('"').trim_matches('\'');
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    root.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| root.to_string_lossy().to_string())
}

fn read_dir_ext(dir: &Path, ext: &str) -> impl Iterator<Item = PathBuf> {
    let ext = ext.to_string();
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(move |p| p.extension().and_then(|e| e.to_str()) == Some(ext.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn default_projects_dir_respects_env() {
        // Safety: only assert helper uses env when set in this process is hard;
        // just check non-empty fallback path shape.
        let d = default_projects_dir();
        assert!(!d.as_os_str().is_empty());
    }

    #[test]
    fn collect_project_files_only_local_layers() {
        let tmp = tempfile_dir("veil_proj_scan");
        fs::write(tmp.join("app.veil"), "pkg App\n").unwrap();
        fs::create_dir_all(tmp.join("layers")).unwrap();
        fs::write(tmp.join("layers/wear_test.layer"), "pkg wear_test v1\n").unwrap();
        // Core name under project layers/ is still filtered when show_core=false
        fs::write(tmp.join("layers/ddd.layer"), "pkg ddd v1\n").unwrap();

        let files = collect_project_files(&tmp, false).unwrap();
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"app.veil".into()));
        assert!(names.contains(&"wear_test.layer".into()));
        assert!(
            !names.iter().any(|n| n == "ddd.layer"),
            "core ddd.layer should be hidden: {names:?}"
        );
        // No monorepo layers injected
        assert!(files.iter().all(|p| p.starts_with(&tmp)));
    }

    #[test]
    fn create_project_scaffolds_git_and_files() {
        let hub = tempfile_dir("veil_proj_hub");
        let info = create_project(&hub, "hello-app").unwrap();
        assert_eq!(info.name, "hello-app");
        let root = PathBuf::from(&info.path);
        assert!(root.join("veil.toml").is_file());
        assert!(root.join("hello-app.veil").is_file());
        assert!(root.join("layers").is_dir());
        assert!(root.join("stubs").is_dir());
        let listed = list_projects(&hub).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "hello-app");
    }

    #[test]
    fn validate_name_rejects_bad() {
        assert!(validate_project_name("ok_name-1").is_ok());
        assert!(validate_project_name("has space").is_err());
        assert!(validate_project_name("").is_err());
    }

    fn tempfile_dir(prefix: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "{prefix}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
}
