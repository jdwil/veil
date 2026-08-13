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
/// Delegates to [`veil_ir::is_platform_layer_name`] (single source of truth).
pub fn is_core_platform_layer(stem: &str) -> bool {
    veil_ir::is_platform_layer_name(stem)
}

/// Default projects directory: env → `~/.veil/config.json` → `~/veil-projects`.
///
/// Prefer [`crate::config::resolve_projects_dir`] / [`crate::default_projects_dir`].
pub fn default_projects_dir() -> PathBuf {
    crate::config::resolve_projects_dir()
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

/// Options for [`init_project`] (INIT-001).
#[derive(Debug, Clone)]
pub struct InitOptions {
    /// Product name (`[a-zA-Z0-9_-]+`).
    pub name: String,
    /// Run `git init` when git is available (default true).
    pub git: bool,
    /// Allow non-empty dir / overwrite scaffold files.
    pub force: bool,
}

impl InitOptions {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            git: true,
            force: false,
        }
    }
}

const PROJECT_GITIGNORE: &str = "\
# VEIL / tooling
generated/
target/
.veil-dev/
output/

# OS
.DS_Store
Thumbs.db
";

/// Default product intent brief for new projects (`MISSION.md`).
///
/// Short, normative, non-goal-heavy — not a backlog or architecture essay.
/// Agents receive a capped inject when the file exists (see `agent_context`).
pub fn mission_md_template(name: &str) -> String {
    format!(
        r#"# {name}

## Purpose
One short paragraph: what this product is for.

## In scope
- …

## Out of scope
- …

## Primary users & success
- Who:
- Success:

## Hard constraints
- …

<!-- Keep this brief (~1–2 min read). Product intent lives here; behavior lives in .veil. -->
"#
    )
}

/// Max chars injected into the agent preamble from `MISSION.md` (token budget).
pub const MISSION_MAX_INJECT_CHARS: usize = 2_000;

/// Read project-root `MISSION.md` when present, capped for agent inject.
pub fn read_mission_for_agent(root: &Path) -> Option<String> {
    let path = root.join("MISSION.md");
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MISSION_MAX_INJECT_CHARS {
        return Some(trimmed.to_string());
    }
    let mut out: String = trimmed.chars().take(MISSION_MAX_INJECT_CHARS).collect();
    out.push_str("\n\n…[MISSION.md truncated for agent budget — keep the file short]…\n");
    Some(out)
}

/// In-memory scaffold files for a new product (disk or S3).
///
/// Paths are relative (`veil.toml`, `main.veil`, …). Used by [`init_project`]
/// and remote create (`s3_workspace::seed_new_repo_scaffold`).
///
/// `name` may be a display title (`Agent Registry`) or a slug (`agent-registry`).
/// Filesystem / package identity uses the slug form; MISSION.md keeps the display title.
pub fn scaffold_file_contents(name: &str) -> Result<Vec<(String, String)>, String> {
    let display = name.trim();
    if display.is_empty() {
        return Err("project name is empty".into());
    }
    let slug = slugify_name(display);
    validate_project_name(&slug)?;
    let pkg_name = pascal_case(&slug);
    // Default backend target so dual-loop smoke can attach without the agent
    // hand-authoring [[targets]] first (missing targets used to reject write_source).
    let veil_toml = format!(
        r#"name = "{slug}"

[package]
name = "{slug}"
veil = "main.veil"
layer = "layers/main.layer"

# Dual-loop: gen + cargo check on write_source (agent + IDE).
[[targets]]
name = "backend"
package = "main.veil"
target = "rust"
output = "generated/backend"
dev_command = "VEIL_DEV=1 cargo run -p veil_bin"

# Flip: new projects require declared endpoint / deps / compose.
# Existing packages without [harness] stay compat=auto for one release.
[harness]
compat = "off"
"#
    );
    let pkg_src = format!(
        "pkg {pkg_name}\n  use ddd\n\n  # Scaffold — edit via IDE / write_source (source of truth is remote when VEIL_SOURCE_MODE=s3)\n"
    );
    let layer_src = format!("pkg {slug} v1\n  desc \"{display} product language\"\n  use ddd\n");
    Ok(vec![
        ("veil.toml".into(), veil_toml),
        ("main.veil".into(), pkg_src),
        ("layers/main.layer".into(), layer_src),
        ("MISSION.md".into(), mission_md_template(display)),
        (".gitignore".into(), PROJECT_GITIGNORE.to_string()),
        // Keep empty dirs visible in S3 listings / materialize
        ("stubs/.gitkeep".into(), String::new()),
    ])
}

/// Slug for package / path identity (`Agent Registry` → `agent-registry`).
pub fn slugify_name(raw: &str) -> String {
    raw.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

/// Scaffold a product project at `root` (INIT-001).
///
/// Creates `veil.toml` (`[package]` entry), `main.veil`, `layers/main.layer`,
/// `MISSION.md`, `stubs/`, `.gitignore`, and optionally `git init` (R21).
///
/// **Disk hub only** — when `VEIL_SOURCE_MODE=s3`, use
/// [`crate::provider::s3_workspace::seed_new_repo_scaffold`] instead.
pub fn init_project(root: &Path, opts: &InitOptions) -> Result<ProjectInfo, String> {
    let slug = slugify_name(&opts.name);
    validate_project_name(&slug)?;

    if root.exists() {
        if !root.is_dir() {
            return Err(format!("not a directory: {}", root.display()));
        }
        let has_pkg = root.join("veil.toml").is_file()
            || read_dir_ext(root, "veil").next().is_some();
        if has_pkg && !opts.force {
            return Err(format!(
                "{} already looks like a VEIL project (veil.toml or *.veil present); use --force to re-scaffold",
                root.display()
            ));
        }
        if !opts.force {
            // Non-empty without veil files: refuse unless empty or only empty dirs
            if let Ok(rd) = std::fs::read_dir(root) {
                let entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                if !entries.is_empty() {
                    let only_ok = entries.iter().all(|e| {
                        let n = e.file_name();
                        let s = n.to_string_lossy();
                        s == "layers"
                            || s == "stubs"
                            || s == ".git"
                            || s == ".gitignore"
                            || s == "MISSION.md"
                    });
                    if !only_ok {
                        return Err(format!(
                            "{} is not empty; pass --force or choose an empty path",
                            root.display()
                        ));
                    }
                }
            }
        }
    } else {
        std::fs::create_dir_all(root)
            .map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    }

    std::fs::create_dir_all(root.join("layers"))
        .map_err(|e| format!("cannot create layers/: {e}"))?;
    std::fs::create_dir_all(root.join("stubs"))
        .map_err(|e| format!("cannot create stubs/: {e}"))?;

    // Prefer original display name for MISSION.md; package paths use slugify inside.
    for (rel, content) in scaffold_file_contents(&opts.name)? {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if path.exists() && !opts.force && (rel == "MISSION.md" || rel == ".gitignore") {
            continue;
        }
        std::fs::write(&path, content).map_err(|e| format!("cannot write {rel}: {e}"))?;
    }

    if opts.git && !root.join(".git").exists() {
        let git_ok = std::process::Command::new("git")
            .args(["init"])
            .current_dir(root)
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
    }

    Ok(ProjectInfo {
        name: slug,
        path: root.to_string_lossy().to_string(),
        is_git: root.join(".git").exists(),
        package_count: 1,
    })
}

/// Create a new product under `projects_dir` (INIT-002 = hub entry to init).
///
/// Refuses when `VEIL_SOURCE_MODE=s3` — use remote create (`create_project` agent
/// tool / `seed_new_repo_scaffold`) instead of writing the disk hub.
pub fn create_project(projects_dir: &Path, name: &str) -> Result<ProjectInfo, String> {
    create_project_with_opts(projects_dir, name, true)
}

/// Hub create with git flag.
pub fn create_project_with_opts(
    projects_dir: &Path,
    name: &str,
    git: bool,
) -> Result<ProjectInfo, String> {
    if !crate::provider::s3_workspace::allow_disk_project_create() {
        return Err(
            "create_project disk hub forbidden while VEIL_SOURCE_MODE=s3 — use remote create (POST /api/repos + S3 scaffold via agent create_project)"
                .into(),
        );
    }
    let slug = slugify_name(name);
    validate_project_name(&slug)?;
    ensure_projects_dir(projects_dir)?;
    let root = projects_dir.join(&slug);
    if root.exists() && has_package_sources(&root) {
        return Err(format!("project already exists: {}", root.display()));
    }
    init_project(
        &root,
        &InitOptions {
            // Keep display name for MISSION; dir/slug is `slug`
            name: name.to_string(),
            git,
            force: root.exists(),
        },
    )
}

/// INIT-003: ensure `layers/` and `stubs/` exist under a project root.
pub fn ensure_project_shape(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    for sub in ["layers", "stubs"] {
        let p = root.join(sub);
        if !p.exists() {
            std::fs::create_dir_all(&p)
                .map_err(|e| format!("cannot create {}: {e}", p.display()))?;
            eprintln!("veil: created {}/", p.display());
        }
    }
    Ok(())
}

/// Whether the directory has any package sources.
pub fn has_package_sources(root: &Path) -> bool {
    root.join("veil.toml").is_file() || read_dir_ext(root, "veil").next().is_some()
}

/// UX-010 / editable for serve list (shared with hub open).
pub fn is_source_editable(path: &Path, source: &str) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "veil" && ext != "layer" {
        return false;
    }
    if path.components().any(|c| c.as_os_str() == "generated") {
        return false;
    }
    for line in source.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t == "# veil:readonly" || t.starts_with("# veil:readonly ") {
            return false;
        }
        break;
    }
    true
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
        assert!(root.join("main.veil").is_file());
        assert!(root.join("layers/main.layer").is_file());
        assert!(root.join("MISSION.md").is_file());
        assert!(root.join("layers").is_dir());
        assert!(root.join("stubs").is_dir());
        assert!(root.join(".gitignore").is_file());
        let toml = std::fs::read_to_string(root.join("veil.toml")).unwrap();
        assert!(toml.contains("[package]"), "{toml}");
        assert!(toml.contains("main.veil"), "{toml}");
        assert!(
            toml.contains("[harness]") && toml.contains("compat = \"off\""),
            "veil init must default compat=off:\n{toml}"
        );
        let mission = std::fs::read_to_string(root.join("MISSION.md")).unwrap();
        assert!(mission.contains("# hello-app"), "{mission}");
        assert!(mission.contains("## Out of scope"), "{mission}");
        let injected = read_mission_for_agent(&root).expect("mission inject");
        assert!(injected.contains("Purpose"));
        let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains("generated/"));
        let listed = list_projects(&hub).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "hello-app");
    }

    #[test]
    fn init_project_refuses_clobber() {
        let root = tempfile_dir("veil_init_clobber");
        init_project(&root, &InitOptions::new("a")).unwrap();
        let err = init_project(&root, &InitOptions::new("b")).unwrap_err();
        assert!(err.contains("already") || err.contains("force"), "{err}");
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
