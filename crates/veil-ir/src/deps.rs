//! Product dependencies (R20) — declared in `veil.toml` for hub + cloud resolve.
//!
//! ```toml
//! [dependencies]
//! designkit = { project = "dlx-designkit" }
//! application = { path = "../application" }
//! # future: mylib = { git = "https://…", rev = "main" }
//! ```
//!
//! Resolved roots are added to adapt package search and layer search so
//! `use designkit` works without relying only on ambient sibling discovery.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One product dependency keyed by **use name** (layer / package stem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductDep {
    /// Name used in `use <name>` / `adapt <name>` (map key in toml).
    pub use_name: String,
    /// Hub project directory name under `projects_dir` (e.g. `dlx-designkit`).
    pub project: Option<String>,
    /// Explicit path (absolute or relative to the depending project root).
    pub path: Option<PathBuf>,
    /// Optional git URL (materialized into cache when path/project missing).
    pub git: Option<String>,
    /// Git rev/branch/tag when `git` is set.
    pub rev: Option<String>,
}

/// Flexible toml value for a single dependency entry.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DepToml {
    /// `designkit = "../dlx-designkit"`
    Path(String),
    /// `designkit = { project = "dlx-designkit", path = "…", git = "…", rev = "…" }`
    Table {
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        git: Option<String>,
        #[serde(default)]
        rev: Option<String>,
        /// Optional override of use-name (defaults to table key).
        #[serde(default, rename = "use")]
        use_name: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct VeilTomlFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    package: Option<PackageToml>,
    #[serde(default)]
    dependencies: BTreeMap<String, DepToml>,
    /// Product codegen policy overrides (INV-001). Applied after layers load.
    #[serde(default)]
    codegen: Option<CodegenToml>,
}

/// `[codegen]` section in `veil.toml` — product knobs over layer policies.
///
/// Absent keys leave layer policy alone. Empty string or `"none"` clears an
/// optional field (e.g. disable bus strip prefix without forking ddd.layer).
///
/// ```toml
/// [codegen]
/// bus_strip_prefix = "Handle"
/// auth_service_trait = "AuthService"
/// http_path_prefix = "/api/v1/"
/// http_list_prefix = "List"
/// http_get_prefix = "Get"
/// http_create_prefix = "Create"
/// http_update_prefix = "Update"
/// http_delete_prefix = "Delete"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodegenToml {
    #[serde(default)]
    pub bus_strip_prefix: Option<String>,
    #[serde(default)]
    pub auth_service_trait: Option<String>,
    #[serde(default)]
    pub http_path_prefix: Option<String>,
    #[serde(default)]
    pub http_list_prefix: Option<String>,
    #[serde(default)]
    pub http_get_prefix: Option<String>,
    #[serde(default)]
    pub http_create_prefix: Option<String>,
    #[serde(default)]
    pub http_update_prefix: Option<String>,
    #[serde(default)]
    pub http_delete_prefix: Option<String>,
}

impl CodegenToml {
    /// True when at least one override key was present in toml.
    pub fn is_empty(&self) -> bool {
        self.bus_strip_prefix.is_none()
            && self.auth_service_trait.is_none()
            && self.http_path_prefix.is_none()
            && self.http_list_prefix.is_none()
            && self.http_get_prefix.is_none()
            && self.http_create_prefix.is_none()
            && self.http_update_prefix.is_none()
            && self.http_delete_prefix.is_none()
    }

    /// Normalize a optional string field: empty / `-` / `none` → clear (None).
    pub fn normalize_opt(s: &Option<String>) -> Option<Option<String>> {
        match s {
            None => None, // key absent — do not override
            Some(v) => {
                let t = v.trim();
                if t.is_empty() || t == "-" || t.eq_ignore_ascii_case("none") {
                    Some(None) // explicit clear
                } else {
                    Some(Some(t.to_string()))
                }
            }
        }
    }
}

/// Load `[codegen]` from a product root’s `veil.toml` (None if missing/empty).
pub fn load_codegen_overrides(project_root: &Path) -> Result<Option<CodegenToml>, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read {}: {e}", toml_path.display()))?;
    let parsed: VeilTomlFile =
        toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))?;
    Ok(parsed.codegen.filter(|c| !c.is_empty()))
}

/// Walk from a `.veil` path to project root and load `[codegen]` if present.
pub fn load_codegen_overrides_for(veil_path: &Path) -> Option<CodegenToml> {
    let root = find_project_root(veil_path)?;
    load_codegen_overrides(&root).ok().flatten()
}

/// `[package]` entry in veil.toml (R21).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PackageToml {
    /// Language/`use` name (defaults to top-level `name`).
    #[serde(default)]
    pub name: Option<String>,
    /// Primary package source relative to project root (default: `main.veil` if present).
    #[serde(default)]
    pub veil: Option<String>,
    /// Primary layer relative to project root (default: `layers/main.layer` if present).
    #[serde(default)]
    pub layer: Option<String>,
}

/// Resolved primary package entry for a product root (R21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    /// Project directory name / top-level `name` in veil.toml.
    pub project_name: Option<String>,
    /// `use` / language name (`package.name` or project name).
    pub use_name: String,
    /// Relative path to primary .veil (e.g. `main.veil`).
    pub veil: PathBuf,
    /// Relative path to primary .layer (e.g. `layers/main.layer`).
    pub layer: PathBuf,
}

impl PackageEntry {
    pub fn provides_use(&self, name: &str) -> bool {
        self.use_name == name
            || self
                .project_name
                .as_ref()
                .map(|p| p == name)
                .unwrap_or(false)
    }

    pub fn veil_abs(&self, root: &Path) -> PathBuf {
        root.join(&self.veil)
    }

    pub fn layer_abs(&self, root: &Path) -> PathBuf {
        root.join(&self.layer)
    }
}

/// Load `[package]` + defaults for a product root. Returns None if no veil.toml.
pub fn load_package_entry(project_root: &Path) -> Option<PackageEntry> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(&toml_path).ok()?;
    let parsed: VeilTomlFile = toml::from_str(&content).ok()?;
    let project_name = parsed.name.clone();
    let pkg = parsed.package.unwrap_or_default();
    let use_name = pkg
        .name
        .or_else(|| project_name.clone())
        .unwrap_or_else(|| {
            project_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("app")
                .to_string()
        });

    let veil = if let Some(v) = pkg.veil {
        PathBuf::from(v)
    } else if project_root.join("main.veil").is_file() {
        PathBuf::from("main.veil")
    } else if project_root.join(format!("{use_name}.veil")).is_file() {
        PathBuf::from(format!("{use_name}.veil"))
    } else {
        // Prefer main.veil as the written convention even if not yet created
        PathBuf::from("main.veil")
    };

    let layer = if let Some(l) = pkg.layer {
        PathBuf::from(l)
    } else if project_root.join("layers/main.layer").is_file() {
        PathBuf::from("layers/main.layer")
    } else if project_root.join(format!("layers/{use_name}.layer")).is_file() {
        PathBuf::from(format!("layers/{use_name}.layer"))
    } else if project_root.join("main.layer").is_file() {
        PathBuf::from("main.layer")
    } else {
        PathBuf::from("layers/main.layer")
    };

    Some(PackageEntry {
        project_name,
        use_name,
        veil,
        layer,
    })
}

/// Whether this product root provides the given `use` name (entry or legacy files).
pub fn product_provides_use(root: &Path, use_name: &str) -> bool {
    if let Some(entry) = load_package_entry(root) {
        if entry.provides_use(use_name) {
            return true;
        }
    }
    // Legacy: folder or file stem matches
    root.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == use_name)
        .unwrap_or(false)
        || root.join(format!("{use_name}.veil")).is_file()
        || root.join("layers").join(format!("{use_name}.layer")).is_file()
}

/// Absolute path to primary package source for `use_name` inside a product root.
pub fn package_source_in_root(root: &Path, use_name: &str) -> Option<PathBuf> {
    if let Some(entry) = load_package_entry(root) {
        if entry.provides_use(use_name) {
            let p = entry.veil_abs(root);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // main.veil when package declares this use name via pkg line (peek)
    let main = root.join("main.veil");
    if main.is_file() && package_file_use_name(&main).as_deref() == Some(use_name) {
        return Some(main);
    }
    let legacy = root.join(format!("{use_name}.veil"));
    if legacy.is_file() {
        return Some(legacy);
    }
    None
}

/// Absolute path to primary layer for `use_name` inside a product root.
///
/// Never returns another product’s `main.layer` for a different use name
/// (that caused infinite load_layer recursion when `use ddd` resolved to a
/// product main.layer that itself `use ddd`).
pub fn layer_source_in_root(root: &Path, use_name: &str) -> Option<PathBuf> {
    if let Some(entry) = load_package_entry(root) {
        if entry.provides_use(use_name) {
            let p = entry.layer_abs(root);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // Named layer only — do not fall back to main.layer unless entry matched above
    for rel in [
        format!("layers/{use_name}.layer"),
        format!("{use_name}.layer"),
    ] {
        let p = root.join(&rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Peek `pkg Name` from a .veil file.
fn package_file_use_name(veil_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(veil_path).ok()?;
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("pkg ") {
            let name = rest.split_whitespace().next()?.to_string();
            return Some(name);
        }
    }
    None
}

/// Walk parents of `start` looking for a directory that contains `veil.toml`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent().map(|p| p.to_path_buf())
    } else {
        Some(start.to_path_buf())
    };
    while let Some(dir) = cur {
        if dir.join("veil.toml").is_file() {
            return Some(dir);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Projects hub directory for resolving `{ project = "…" }` deps.
///
/// Order: `VEIL_PROJECTS_DIR` → parent of project root (hub) → project root.
/// Canonicalizes `project_root` when possible so relative paths like
/// `main.veil` (parent `""`) still resolve hub as the parent of the real
/// product directory under CWD.
pub fn projects_hub(project_root: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("VEIL_PROJECTS_DIR") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| {
            // Relative / empty path: resolve against CWD
            if project_root.as_os_str().is_empty() || project_root == Path::new(".") {
                std::env::current_dir().unwrap_or_else(|_| project_root.to_path_buf())
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(project_root))
                    .ok()
                    .and_then(|p| p.canonicalize().ok())
                    .unwrap_or_else(|| project_root.to_path_buf())
            }
        });
    root.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(root)
}

/// Cache dir for git-materialized deps: `$hub/.veil-deps/`.
pub fn deps_cache_dir(hub: &Path) -> PathBuf {
    hub.join(".veil-deps")
}

/// Parse `[dependencies]` from a project’s `veil.toml` (empty if missing).
pub fn load_product_deps(project_root: &Path) -> Result<Vec<ProductDep>, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read {}: {e}", toml_path.display()))?;
    let parsed: VeilTomlFile =
        toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))?;
    let mut out = Vec::new();
    for (key, val) in parsed.dependencies {
        let dep = match val {
            DepToml::Path(p) => ProductDep {
                use_name: key,
                project: None,
                path: Some(PathBuf::from(p)),
                git: None,
                rev: None,
            },
            DepToml::Table {
                project,
                path,
                git,
                rev,
                use_name,
            } => ProductDep {
                use_name: use_name.unwrap_or(key),
                project,
                path: path.map(PathBuf::from),
                git,
                rev,
            },
        };
        out.push(dep);
    }
    out.sort_by(|a, b| a.use_name.cmp(&b.use_name));
    Ok(out)
}

fn looks_like_product_root(dir: &Path) -> bool {
    dir.join("veil.toml").is_file()
        || dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| {
                // application/main.veil or designkit.veil at root of dlx-designkit
                dir.join(format!("{name}.veil")).is_file()
            })
            .unwrap_or(false)
        || dir
            .read_dir()
            .ok()
            .map(|rd| {
                rd.filter_map(|e| e.ok()).any(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        == Some("veil")
                })
            })
            .unwrap_or(false)
}

/// Resolve one dep to an on-disk product root (materialize git if needed).
pub fn resolve_dep_root(
    project_root: &Path,
    dep: &ProductDep,
    hub: &Path,
) -> Result<PathBuf, String> {
    // 1. Explicit path
    if let Some(ref p) = dep.path {
        let resolved = if p.is_absolute() {
            p.clone()
        } else {
            project_root.join(p)
        };
        let resolved = resolved
            .canonicalize()
            .unwrap_or(resolved);
        if looks_like_product_root(&resolved) || resolved.is_dir() {
            return Ok(resolved);
        }
        return Err(format!(
            "dependency '{}': path {} not found or not a VEIL product",
            dep.use_name,
            resolved.display()
        ));
    }

    // 2. Hub project id
    if let Some(ref proj) = dep.project {
        let candidate = hub.join(proj);
        if looks_like_product_root(&candidate) || candidate.is_dir() {
            return Ok(candidate
                .canonicalize()
                .unwrap_or(candidate));
        }
        // Fall through to git if also specified
        if dep.git.is_none() {
            return Err(format!(
                "dependency '{}': project '{}' not found under hub {} — \
                 clone it there or set path = \"…\" in veil.toml [dependencies]",
                dep.use_name,
                proj,
                hub.display()
            ));
        }
    }

    // 3. Git materialize
    if let Some(ref url) = dep.git {
        return materialize_git_dep(dep, url, hub);
    }

    Err(format!(
        "dependency '{}': need path, project, or git in veil.toml [dependencies]",
        dep.use_name
    ))
}

fn materialize_git_dep(dep: &ProductDep, url: &str, hub: &Path) -> Result<PathBuf, String> {
    let cache = deps_cache_dir(hub);
    std::fs::create_dir_all(&cache)
        .map_err(|e| format!("cannot create deps cache {}: {e}", cache.display()))?;
    let dest = cache.join(&dep.use_name);
    if looks_like_product_root(&dest) {
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    if dest.exists() {
        // Incomplete prior clone — remove and retry
        let _ = std::fs::remove_dir_all(&dest);
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("clone");
    if let Some(ref rev) = dep.rev {
        cmd.args(["--branch", rev]);
    }
    cmd.args(["--depth", "1", url]);
    cmd.arg(&dest);
    let out = cmd
        .output()
        .map_err(|e| format!("dependency '{}': git clone failed to start: {e}", dep.use_name))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "dependency '{}': git clone {} failed: {stderr}",
            dep.use_name, url
        ));
    }
    Ok(dest.canonicalize().unwrap_or(dest))
}

/// Resolve all declared deps to product roots (errors are collected).
pub fn resolve_dependency_roots(project_root: &Path) -> Result<Vec<PathBuf>, String> {
    let deps = load_product_deps(project_root)?;
    if deps.is_empty() {
        return Ok(Vec::new());
    }
    let hub = projects_hub(project_root);
    let mut roots = Vec::new();
    let mut errors = Vec::new();
    for dep in &deps {
        match resolve_dep_root(project_root, dep, &hub) {
            Ok(r) => {
                if !roots.contains(&r) {
                    roots.push(r);
                }
            }
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() && roots.is_empty() {
        return Err(errors.join("; "));
    }
    // Soft-warn partial failures via stderr (gen continues with what resolved)
    for e in errors {
        eprintln!("veil: {e}");
    }
    Ok(roots)
}

/// Resolve dependency roots for any path under a product (file or dir).
pub fn resolve_dependency_roots_for(path: &Path) -> Vec<PathBuf> {
    let Some(root) = find_project_root(path) else {
        return Vec::new();
    };
    match resolve_dependency_roots(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("veil: dependencies: {e}");
            Vec::new()
        }
    }
}

/// Adapt package search paths: defaults + declared product deps.
pub fn adapt_search_paths_for_file(leaf_path: &Path) -> Vec<PathBuf> {
    let extra = resolve_dependency_roots_for(leaf_path);
    crate::adapt::default_adapt_search_paths(leaf_path, &extra)
}

/// Human-readable hint when a use/adapt target is missing.
pub fn missing_package_hint(use_name: &str, project_root: Option<&Path>) -> String {
    let mut msg = format!(
        "package '{use_name}' not found for use/adapt.\n\
         Searched: project dir, hub siblings, and [dependencies] roots."
    );
    if let Some(root) = project_root {
        let hub = projects_hub(root);
        msg.push_str(&format!(
            "\n\nDeclare it in {}:\n\n\
             [dependencies]\n\
             {use_name} = {{ project = \"{use_name}\" }}\n\
             # or: {use_name} = {{ path = \"../other-product\" }}\n\
             # or: {use_name} = {{ git = \"https://…\", rev = \"main\" }}\n\n\
             Hub for project= is {} (VEIL_PROJECTS_DIR or parent of project).",
            root.join("veil.toml").display(),
            hub.display()
        ));
    } else {
        msg.push_str(&format!(
            "\n\nAdd to veil.toml:\n[dependencies]\n{use_name} = {{ project = \"…\" }}"
        ));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_path_and_project_deps() {
        let dir = tempfile_dir();
        let mut f = std::fs::File::create(dir.join("veil.toml")).unwrap();
        writeln!(
            f,
            r#"
name = "app"
[dependencies]
designkit = {{ project = "dlx-designkit" }}
application = "../application"
mylib = {{ path = "/tmp/mylib", use = "lib" }}
"#
        )
        .unwrap();
        let deps = load_product_deps(&dir).unwrap();
        assert_eq!(deps.len(), 3);
        let dk = deps.iter().find(|d| d.use_name == "designkit").unwrap();
        assert_eq!(dk.project.as_deref(), Some("dlx-designkit"));
        let eng = deps.iter().find(|d| d.use_name == "application").unwrap();
        assert_eq!(
            eng.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            Some("../application".into())
        );
        let lib = deps.iter().find(|d| d.use_name == "lib").unwrap();
        assert!(lib.path.is_some());
    }

    #[test]
    fn parse_codegen_overrides() {
        let dir = tempfile_dir();
        std::fs::write(
            dir.join("veil.toml"),
            r#"
name = "app"
[codegen]
bus_strip_prefix = "Cmd"
auth_service_trait = "AuthService"
http_path_prefix = "/api/v1/"
http_list_prefix = "List"
"#,
        )
        .unwrap();
        let o = load_codegen_overrides(&dir).unwrap().expect("codegen");
        assert_eq!(o.bus_strip_prefix.as_deref(), Some("Cmd"));
        assert_eq!(o.auth_service_trait.as_deref(), Some("AuthService"));
        assert_eq!(o.http_path_prefix.as_deref(), Some("/api/v1/"));
        assert_eq!(o.http_list_prefix.as_deref(), Some("List"));
        assert!(o.http_get_prefix.is_none());
    }

    #[test]
    fn codegen_none_string_normalizes_to_clear() {
        assert_eq!(
            CodegenToml::normalize_opt(&Some("none".into())),
            Some(None)
        );
        assert_eq!(CodegenToml::normalize_opt(&Some("".into())), Some(None));
        assert_eq!(
            CodegenToml::normalize_opt(&Some("Handle".into())),
            Some(Some("Handle".into()))
        );
        assert_eq!(CodegenToml::normalize_opt(&None), None);
    }

    #[test]
    fn resolve_path_dep() {
        let hub = tempfile_dir();
        let eng = hub.join("application");
        std::fs::create_dir_all(&eng).unwrap();
        std::fs::write(eng.join("veil.toml"), "name = \"application\"\n").unwrap();
        std::fs::write(eng.join("main.veil"), "pkg application\n  use ddd\n").unwrap();

        let app = hub.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("veil.toml"),
            "[dependencies]\napplication = { path = \"../application\" }\n",
        )
        .unwrap();

        let roots = resolve_dependency_roots(&app).unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].ends_with("application") || roots[0].file_name().unwrap() == "application");
    }

    #[test]
    fn resolve_project_dep_via_hub() {
        let hub = tempfile_dir();
        let eng = hub.join("application");
        std::fs::create_dir_all(eng.join("layers")).unwrap();
        std::fs::write(eng.join("veil.toml"), "name = \"application\"\n").unwrap();
        std::fs::write(
            eng.join("layers").join("main.layer"),
            "pkg application v1\n  use ddd\n",
        )
        .unwrap();

        let app = hub.join("wear_test");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("veil.toml"),
            "[dependencies]\napplication = { project = \"application\" }\n",
        )
        .unwrap();

        // Hub is parent of app
        let roots = resolve_dependency_roots(&app).unwrap();
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn package_entry_main_veil_and_layer() {
        let root = tempfile_dir();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(
            root.join("veil.toml"),
            r#"
name = "dlx-designkit"
[package]
name = "designkit"
veil = "main.veil"
layer = "layers/main.layer"
"#,
        )
        .unwrap();
        std::fs::write(root.join("main.veil"), "pkg designkit\n  use ddd\n").unwrap();
        std::fs::write(
            root.join("layers/main.layer"),
            "pkg designkit v1\n  use sveltekit5\n",
        )
        .unwrap();

        let entry = load_package_entry(&root).unwrap();
        assert_eq!(entry.use_name, "designkit");
        assert_eq!(entry.veil, PathBuf::from("main.veil"));
        assert_eq!(entry.layer, PathBuf::from("layers/main.layer"));
        assert!(entry.provides_use("designkit"));
        assert!(package_source_in_root(&root, "designkit")
            .unwrap()
            .ends_with("main.veil"));
        assert!(layer_source_in_root(&root, "designkit")
            .unwrap()
            .ends_with("main.layer"));
    }

    #[test]
    fn find_package_via_main_veil_in_search_path() {
        let root = tempfile_dir();
        std::fs::create_dir_all(root.join("layers")).unwrap();
        std::fs::write(
            root.join("veil.toml"),
            "name = \"application\"\n[package]\nname = \"application\"\n",
        )
        .unwrap();
        // Defaults prefer main.veil when present
        std::fs::write(root.join("main.veil"), "pkg application\n  use ddd\n").unwrap();
        std::fs::write(
            root.join("layers/main.layer"),
            "pkg application v1\n  use ddd\n",
        )
        .unwrap();

        let found = crate::adapt::find_package_source("application", &[root.clone()]);
        assert!(found.unwrap().ends_with("main.veil"));
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "veil-deps-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}








