//! User config at `~/.veil/config.json` (and optional env overrides).
//!
//! See `docs/PROJECT_LAYOUT.md` and `docs/IDE_RUNTIME.md`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk user configuration for local VEIL / runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VeilConfig {
    /// Schema version for migrations.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Parent directory of product git repos (runtime projects hub).
    #[serde(default = "default_projects_dir_string")]
    pub projects_dir: String,
    /// Optional pin for core platform layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers_dir: Option<String>,
    /// Show core platform layers in IDE file pickers (language devs).
    #[serde(default)]
    pub show_core_layers: bool,
    /// True after first-run wizard completed (or config written).
    #[serde(default)]
    pub configured: bool,
}

fn default_version() -> u32 {
    1
}

fn default_projects_dir_string() -> String {
    fallback_projects_dir().to_string_lossy().to_string()
}

impl Default for VeilConfig {
    fn default() -> Self {
        Self {
            version: 1,
            projects_dir: default_projects_dir_string(),
            layers_dir: None,
            show_core_layers: false,
            configured: false,
        }
    }
}

impl VeilConfig {
    pub fn projects_dir_path(&self) -> PathBuf {
        expand_user_path(&self.projects_dir)
    }

    pub fn layers_dir_path(&self) -> Option<PathBuf> {
        self.layers_dir.as_ref().map(|s| expand_user_path(s))
    }
}

/// `~/.veil` (or `VEIL_DATA_DIR` when set — same root as local storage).
pub fn veil_home_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("VEIL_DATA_DIR") {
        return PathBuf::from(p);
    }
    home_dir()
        .map(|h| h.join(".veil"))
        .unwrap_or_else(|| PathBuf::from(".veil"))
}

/// Path to `config.json`.
pub fn config_path() -> PathBuf {
    veil_home_dir().join("config.json")
}

/// Fallback projects dir when nothing is configured: `~/veil-projects`.
pub fn fallback_projects_dir() -> PathBuf {
    home_dir()
        .map(|h| h.join("veil-projects"))
        .unwrap_or_else(|| PathBuf::from("veil-projects"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expand `~/…` and return absolute-ish path.
pub fn expand_user_path(raw: &str) -> PathBuf {
    let t = raw.trim();
    if t == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("."));
    }
    if let Some(rest) = t.strip_prefix("~/") {
        if let Some(h) = home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(t)
}

/// Load config from disk. Missing file → `Ok(None)`.
pub fn load_config() -> Result<Option<VeilConfig>, String> {
    let path = config_path();
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let cfg: VeilConfig = serde_json::from_str(&text)
        .map_err(|e| format!("invalid {}: {e}", path.display()))?;
    Ok(Some(cfg))
}

/// Load config or defaults (does not write).
pub fn load_config_or_default() -> VeilConfig {
    load_config().ok().flatten().unwrap_or_default()
}

/// Persist config (creates `~/.veil` as needed).
pub fn save_config(cfg: &VeilConfig) -> Result<(), String> {
    let home = veil_home_dir();
    fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
    let path = config_path();
    let text = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("serialize config: {e}"))?;
    fs::write(&path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(())
}

/// Resolve projects directory with precedence:
/// 1. `VEIL_PROJECTS_DIR` env (session override)
/// 2. `config.json` `projects_dir`
/// 3. `~/veil-projects`
pub fn resolve_projects_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("VEIL_PROJECTS_DIR") {
        return PathBuf::from(p);
    }
    load_config_or_default().projects_dir_path()
}

/// Whether first-run setup should run (no config file yet).
pub fn needs_first_run() -> bool {
    !config_path().is_file()
}

/// Apply first-run choices and write config.
pub fn complete_first_run(projects_dir: impl AsRef<Path>) -> Result<VeilConfig, String> {
    let projects_dir = projects_dir.as_ref();
    fs::create_dir_all(projects_dir)
        .map_err(|e| format!("cannot create projects dir {}: {e}", projects_dir.display()))?;
    let mut cfg = load_config_or_default();
    cfg.projects_dir = projects_dir.to_string_lossy().to_string();
    cfg.configured = true;
    cfg.version = 1;
    save_config(&cfg)?;
    Ok(cfg)
}

/// Interactive first-run on a TTY. Non-interactive: write defaults if missing.
///
/// Prompt text suggests `~/dev/veil-projects` when that parent exists, else default.
pub fn ensure_config_interactive() -> Result<VeilConfig, String> {
    if let Some(cfg) = load_config()? {
        return Ok(cfg);
    }

    let default_dir = {
        let dev = home_dir().map(|h| h.join("dev"));
        if dev.as_ref().map(|d| d.is_dir()).unwrap_or(false) {
            home_dir()
                .unwrap()
                .join("dev")
                .join("veil-projects")
        } else {
            fallback_projects_dir()
        }
    };

    let interactive = atty_stderr();
    if !interactive {
        eprintln!(
            "veil: writing default config (non-interactive) → {}",
            config_path().display()
        );
        return complete_first_run(&default_dir);
    }

    eprintln!("╔══════════════════════════════════════════════════════╗");
    eprintln!("║  VEIL first-time setup                               ║");
    eprintln!("╚══════════════════════════════════════════════════════╝");
    eprintln!();
    eprintln!("Where should product projects live?");
    eprintln!("  (each product is an independent git repo under this folder)");
    eprintln!();
    eprint!("  Projects directory [{}]: ", default_dir.display());
    let _ = std::io::Write::flush(&mut std::io::stderr());

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("stdin: {e}"))?;
    let chosen = line.trim();
    let dir = if chosen.is_empty() {
        default_dir
    } else {
        expand_user_path(chosen)
    };

    let cfg = complete_first_run(&dir)?;
    eprintln!();
    eprintln!("✓ Saved {}", config_path().display());
    eprintln!("  projects_dir = {}", cfg.projects_dir_path().display());
    eprintln!();
    Ok(cfg)
}

fn atty_stderr() -> bool {
    // Avoid extra deps: check isatty via libc isn't always available; use
    // stderr metadata heuristic — if we can't tell, assume interactive when
    // TERM is set.
    std::env::var_os("TERM").is_some() && std::env::var_os("CI").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde() {
        if home_dir().is_some() {
            let p = expand_user_path("~/foo/bar");
            assert!(p.ends_with("foo/bar") || p.to_string_lossy().contains("foo"));
        }
    }

    #[test]
    fn roundtrip_config_json() {
        let cfg = VeilConfig {
            version: 1,
            projects_dir: "/tmp/veil-projects-test".into(),
            layers_dir: None,
            show_core_layers: true,
            configured: true,
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: VeilConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(cfg, back);
    }
}
