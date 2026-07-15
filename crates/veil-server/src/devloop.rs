//! Dev-loop orchestrator — file-watch, auto-gen, dev server management.
//!
//! Configured via `veil.toml` `[[targets]]` in the project root.
//! Launched from the IDE via `/api/p/{project}/dev/start`.
//!
//! Flow:
//! 1. Parse `veil.toml` [[targets]] for project
//! 2. Watch `.veil` and `.layer` files in project root
//! 3. On change: run `veil gen <package> -t <target> -o <output>`
//! 4. Spawn dev server process (`dev_command`) in output dir
//! 5. Report status via API + SSE

use std::collections::HashMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Process-wide dev-loop map so agent writes can smoke-test without every
/// handler carrying an Extension. Set once from `build_router` / multi.
static GLOBAL_DEV_LOOPS: OnceLock<SharedDevLoops> = OnceLock::new();

/// Register the shared DevLoop map (idempotent — first wins).
pub fn set_global_dev_loops(loops: SharedDevLoops) {
    let _ = GLOBAL_DEV_LOOPS.set(loops);
}

/// Shared map for smoke gates from agent / MCP write paths.
pub fn global_dev_loops() -> Option<&'static SharedDevLoops> {
    GLOBAL_DEV_LOOPS.get()
}

/// Entry point for agent / MCP after a source write.
/// Ensures a DevLoop exists, then gen+check affected Rust targets; rolls back on fail.
pub fn smoke_agent_write(
    project_root: &Path,
    active_file_path: &str,
    project_name: Option<&str>,
) -> Result<(), String> {
    if !smoke_enabled() {
        return Ok(());
    }
    let Some(loops) = global_dev_loops() else {
        // Server built without registering loops — skip rather than fail open loudly.
        tracing::warn!("smoke_agent_write: no global DevLoop map; skip smoke");
        return Ok(());
    };
    let name = project_name
        .map(|s| s.to_string())
        .unwrap_or_else(|| crate::project_layout::project_display_name(project_root));
    get_or_create_dev_loop(loops, &name, project_root)?;

    // Rel path of the file that changed (for target matching).
    let rel = {
        let p = Path::new(active_file_path);
        if let Ok(r) = p.strip_prefix(project_root) {
            r.to_string_lossy().replace('\\', "/")
        } else if let Some(fname) = p.file_name().and_then(|f| f.to_str()) {
            fname.to_string()
        } else {
            active_file_path.to_string()
        }
    };

    let mut map = loops
        .lock()
        .map_err(|e| format!("devloop lock: {e}"))?;
    let dev = map
        .get_mut(&name)
        .ok_or_else(|| format!("devloop missing for {name}"))?;
    dev.smoke_after_source_change(&rel)
}

/// When false (`VEIL_AGENT_SMOKE=0` / `false` / `off`), skip cargo check + rollback.
pub fn smoke_enabled() -> bool {
    match std::env::var("VEIL_AGENT_SMOKE") {
        Ok(v) => {
            let t = v.trim();
            !(t == "0" || t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("off"))
        }
        Err(_) => true, // default ON — agent must not break the backend
    }
}

/// When false (`VEIL_AGENT_AUTO_RESTART=0`), do not restart owned processes after smoke (ACS-004).
/// Default ON so dual-loop picks up new gen after successful smoke.
pub fn auto_restart_enabled() -> bool {
    match std::env::var("VEIL_AGENT_AUTO_RESTART") {
        Ok(v) => {
            let t = v.trim();
            !(t == "0" || t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

// ─── Config (veil.toml) ────────────────────────────────────────────────────

/// Parsed `veil.toml` project config with dev targets.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub project: Option<ProjectMeta>,
    #[serde(default, rename = "targets")]
    pub targets: Vec<TargetConfig>,
    /// Dev-only configuration: extra packages wired into the local harness.
    #[serde(default)]
    pub dev: Option<DevConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectMeta {
    pub name: Option<String>,
}

/// Dev section: packages to include in the local harness binary.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DevConfig {
    /// Paths to additional .veil packages (absolute or relative to project root).
    #[serde(default)]
    pub packages: Vec<String>,
}

/// A single dev target: which package to gen, what target lang, where output
/// goes, and what dev command to spawn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Display name for the IDE (e.g. "frontend", "backend")
    pub name: String,
    /// Source `.veil` file (relative to project root)
    pub package: String,
    /// Codegen target: "rust", "typescript", "svelte", etc.
    pub target: String,
    /// Output directory (relative to project root)
    pub output: String,
    /// Dev server command to spawn in the output dir
    #[serde(default)]
    pub dev_command: Option<String>,
    /// Port the dev server listens on (for status display)
    #[serde(default)]
    pub dev_port: Option<u16>,
}

/// Parse `veil.toml` from a project root.
pub fn parse_project_config(project_root: &Path) -> Result<ProjectConfig, String> {
    let toml_path = project_root.join("veil.toml");
    if !toml_path.is_file() {
        return Ok(ProjectConfig::default());
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("cannot read veil.toml: {e}"))?;
    toml::from_str(&content).map_err(|e| format!("veil.toml parse error: {e}"))
}

// ─── Process Manager ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetStatus {
    Stopped,
    Starting,
    Running,
    Error,
    Generating,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetState {
    pub name: String,
    pub status: TargetStatus,
    pub config: TargetConfig,
    /// Recent log lines (ring buffer).
    pub logs: Vec<String>,
    /// Last error message.
    pub last_error: Option<String>,
    /// Last successful gen timestamp.
    pub last_gen: Option<String>,
    /// Last gen instant for debounce.
    #[serde(skip)]
    pub last_gen_instant: Option<Instant>,
    /// True when the dev server was detected externally (not spawned by us).
    /// We can monitor it but cannot stop it.
    #[serde(default)]
    pub attached: bool,
}

struct ManagedProcess {
    child: Child,
    started_at: Instant,
}

/// The orchestrator state for one project.
pub struct DevLoop {
    project_root: PathBuf,
    veil_bin: PathBuf,
    targets: Vec<TargetConfig>,
    states: HashMap<String, TargetState>,
    processes: HashMap<String, ManagedProcess>,
    /// Channel to signal the file watcher to stop.
    stop_tx: Option<mpsc::Sender<()>>,
    /// Dev packages from `[dev].packages` — wired into Rust harness.
    dev_packages: Option<Vec<String>>,
    /// Last package/layer sources that passed smoke (path relative to project root).
    /// Used to roll back disk when gen/check fails after a bad write.
    last_good_sources: HashMap<String, String>,
    /// When true, file-watcher should not re-enter generate (agent smoke holds the lock path).
    smoke_in_progress: bool,
}

impl DevLoop {
    pub fn new(project_root: PathBuf, veil_bin: PathBuf, targets: Vec<TargetConfig>, dev_packages: Option<Vec<String>>) -> Self {
        let states = targets
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    TargetState {
                        name: t.name.clone(),
                        status: TargetStatus::Stopped,
                        config: t.clone(),
                        logs: Vec::new(),
                        last_error: None,
                        last_gen: None,
                        last_gen_instant: None,
                        attached: false,
                    },
                )
            })
            .collect();
        Self {
            project_root,
            veil_bin,
            targets,
            states,
            processes: HashMap::new(),
            stop_tx: None,
            dev_packages,
            last_good_sources: HashMap::new(),
            smoke_in_progress: false,
        }
    }

    /// Snapshot primary package (+ layers that matter) after a successful smoke.
    fn remember_good_sources(&mut self) {
        let mut paths: Vec<PathBuf> = Vec::new();
        for t in &self.targets {
            paths.push(self.project_root.join(&t.package));
        }
        // Project-root layers
        if let Ok(rd) = std::fs::read_dir(&self.project_root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("layer") {
                    paths.push(p);
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(self.project_root.join("layers")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("layer") {
                    paths.push(p);
                }
            }
        }
        for p in paths {
            if let Ok(src) = std::fs::read_to_string(&p) {
                if let Ok(rel) = p.strip_prefix(&self.project_root) {
                    self.last_good_sources
                        .insert(rel.to_string_lossy().replace('\\', "/"), src);
                }
            }
        }
    }

    /// Restore package/layer files from last successful smoke.
    fn restore_good_sources(&self) -> Result<(), String> {
        for (rel, content) in &self.last_good_sources {
            let path = self.project_root.join(rel);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, content)
                .map_err(|e| format!("restore {}: {e}", path.display()))?;
        }
        Ok(())
    }

    /// Gen + `cargo check` for a Rust target. On failure, restore the previous
    /// generated tree so a running `cargo run -p veil_bin` is not left on broken code.
    pub fn generate_checked(&mut self, target_name: &str) -> Result<(), String> {
        self.generate_checked_for(target_name, None)
    }

    /// Like [`generate_checked`], but when `changed_rel` is set, prefer checking
    /// the package crate matching that file (so multi-package workspaces with an
    /// unrelated broken sibling don't block edits to a healthy package).
    pub fn generate_checked_for(
        &mut self,
        target_name: &str,
        changed_rel: Option<&str>,
    ) -> Result<(), String> {
        let target = self
            .targets
            .iter()
            .find(|t| t.name == target_name)
            .cloned()
            .ok_or_else(|| format!("unknown target: {target_name}"))?;

        let output_path = self.project_root.join(&target.output);
        let backup = if target.target == "rust" && output_path.is_dir() {
            Some(backup_tree(&output_path)?)
        } else {
            None
        };

        let gen_result = self.generate(target_name);
        if let Err(e) = gen_result {
            if let Some(ref bak) = backup {
                let _ = restore_tree(bak, &output_path);
                let _ = std::fs::remove_dir_all(bak);
            }
            return Err(e);
        }

        if target.target == "rust" {
            if !smoke_enabled() {
                self.remember_good_sources();
                if let Some(bak) = backup {
                    let _ = std::fs::remove_dir_all(bak);
                }
                return Ok(());
            }
            let pkgs = check_packages_for_change(&output_path, changed_rel);
            if !self.check_build_pkgs(&output_path, target_name, &pkgs) {
                let err = self
                    .states
                    .get(target_name)
                    .and_then(|s| s.last_error.clone())
                    .unwrap_or_else(|| "cargo check failed".into());
                if let Some(ref bak) = backup {
                    if let Err(re) = restore_tree(bak, &output_path) {
                        tracing::error!(%re, "failed to restore gen tree after smoke fail");
                    }
                    let _ = std::fs::remove_dir_all(bak);
                }
                if let Some(state) = self.states.get_mut(target_name) {
                    if self.processes.contains_key(target_name) || state.attached {
                        state.status = TargetStatus::Running;
                    }
                    state.logs.push(
                        "[smoke] WRITE/GEN REJECTED — cargo check failed; restored previous generated backend"
                            .into(),
                    );
                }
                let scope = if pkgs.is_empty() {
                    "workspace".into()
                } else {
                    pkgs.join(", ")
                };
                return Err(format!(
                    "SMOKE TEST FAILED — change rejected; previous generated backend restored.\n\
                     Checked: {scope}\n\
                     Fix the VEIL source so generated Rust passes `cargo check`.\n\n{err}"
                ));
            }
        }

        self.remember_good_sources();
        if let Some(bak) = backup {
            let _ = std::fs::remove_dir_all(bak);
        }

        // ACS-004: reload owned cargo run so routes match new gen (not attached).
        // Only stop + start_dev_server — do NOT call start() (would re-enter generate_checked).
        if auto_restart_enabled()
            && target.target == "rust"
            && self.processes.contains_key(target_name)
            && !self
                .states
                .get(target_name)
                .map(|s| s.attached)
                .unwrap_or(false)
        {
            if let Some(state) = self.states.get_mut(target_name) {
                state
                    .logs
                    .push("[dev] restart after smoke (VEIL_AGENT_AUTO_RESTART)".into());
            }
            self.stop_dev_server(target_name);
            if let Err(e) = self.start_dev_server(target_name) {
                if let Some(state) = self.states.get_mut(target_name) {
                    state
                        .logs
                        .push(format!("[dev] restart after smoke failed: {e}"));
                }
                tracing::warn!(target = %target_name, %e, "auto-restart after smoke failed");
            }
        } else if auto_restart_enabled()
            && target.target == "rust"
            && self
                .states
                .get(target_name)
                .map(|s| s.attached)
                .unwrap_or(false)
        {
            if let Some(state) = self.states.get_mut(target_name) {
                state.logs.push(
                    "[dev] smoke OK but target is attached (external) — restart manually or use dev_restart"
                        .into(),
                );
            }
        }

        Ok(())
    }

    /// After an agent (or API) write to a package/layer: gen every affected Rust
    /// target and cargo-check. On failure, restore last-good sources + gen tree.
    pub fn smoke_after_source_change(&mut self, changed_rel: &str) -> Result<(), String> {
        if !smoke_enabled() {
            return Ok(());
        }
        self.smoke_in_progress = true;
        let result = self.smoke_after_source_change_inner(changed_rel);
        self.smoke_in_progress = false;
        result
    }

    fn smoke_after_source_change_inner(&mut self, changed_rel: &str) -> Result<(), String> {
        let rel = changed_rel.replace('\\', "/");
        let affected: Vec<String> = self
            .targets
            .iter()
            .filter(|t| {
                if t.target != "rust" {
                    return false;
                }
                rel == t.package
                    || rel.ends_with(&t.package)
                    || rel.ends_with(".layer")
                    || self.dev_packages.as_ref().map(|pkgs| {
                        pkgs.iter().any(|p| {
                            if Path::new(p).is_absolute() {
                                rel == *p || Path::new(p).ends_with(&rel)
                            } else {
                                rel == *p || rel.ends_with(p)
                            }
                        })
                    }).unwrap_or(false)
            })
            .map(|t| t.name.clone())
            .collect();

        if affected.is_empty() {
            // No rust backend target — nothing to smoke.
            return Ok(());
        }

        let mut errors = Vec::new();
        for name in &affected {
            if let Err(e) = self.generate_checked_for(name, Some(changed_rel)) {
                errors.push(format!("[{name}] {e}"));
            }
        }
        if errors.is_empty() {
            return Ok(());
        }

        // Roll back sources to last good snapshot, then re-gen so disk matches.
        if !self.last_good_sources.is_empty() {
            if let Err(e) = self.restore_good_sources() {
                errors.push(format!("source restore: {e}"));
            } else {
                for name in &affected {
                    // Force re-gen from restored sources (gen tree may already be restored).
                    let _ = self.generate(name);
                }
            }
        }
        Err(errors.join("\n\n"))
    }

    /// Run `veil gen` for a target. When `[dev].packages` is configured,
    /// also gens the dev packages into the same output dir (with `--dev` semantics).
    pub fn generate(&mut self, target_name: &str) -> Result<(), String> {
        // Re-read veil.toml so commenting/uncommenting [dev].packages takes effect
        // without restarting `veil serve`.
        if let Ok(cfg) = parse_project_config(&self.project_root) {
            if !cfg.targets.is_empty() {
                self.targets = cfg.targets;
            }
            self.dev_packages = cfg
                .dev
                .as_ref()
                .map(|d| d.packages.clone())
                .filter(|v| !v.is_empty());
        }

        let target = self
            .targets
            .iter()
            .find(|t| t.name == target_name)
            .ok_or_else(|| format!("unknown target: {target_name}"))?
            .clone();

        if let Some(state) = self.states.get_mut(target_name) {
            state.status = TargetStatus::Generating;
            state.config = target.clone();
        }

        let output_path = self.project_root.join(&target.output);
        let _ = std::fs::create_dir_all(&output_path);

        let package_path = self.project_root.join(&target.package);
        let multi = target.target == "rust" && self.dev_packages.is_some();

        // Multi-package: --no-prune on each package gen so the second package
        // does not delete the first's context crates. Single-package: prune.
        self.run_gen(
            &package_path,
            &target.target,
            &output_path,
            target_name,
            multi, // no_prune when multi
        )?;

        // Dev packages (only for Rust targets — TypeScript frontends don't need them)
        if target.target == "rust" {
            let dev_pkgs = self.dev_packages.clone();
            if let Some(ref pkgs) = dev_pkgs {
                for pkg in pkgs {
                    let pkg_path = if std::path::Path::new(pkg).is_absolute() {
                        std::path::PathBuf::from(pkg)
                    } else {
                        self.project_root.join(pkg)
                    };
                    self.run_gen(
                        &pkg_path,
                        &target.target,
                        &output_path,
                        target_name,
                        true, // no_prune
                    )?;
                }
                // Combined harness first (source of truth for veil_bin path deps)
                let mut all_pkg_paths = vec![package_path.clone()];
                for pkg in pkgs {
                    let p = if std::path::Path::new(pkg).is_absolute() {
                        std::path::PathBuf::from(pkg)
                    } else {
                        self.project_root.join(pkg)
                    };
                    all_pkg_paths.push(p);
                }
                let mut cmd = Command::new(&self.veil_bin);
                cmd.arg("gen-harness");
                for p in &all_pkg_paths {
                    cmd.arg(p);
                }
                cmd.arg("-o").arg(&output_path);
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
                match cmd.output() {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        if let Some(state) = self.states.get_mut(target_name) {
                            if !stdout.is_empty() {
                                state.logs.push(format!("[harness] {stdout}"));
                            }
                            if !stderr.is_empty() {
                                state.logs.push(format!("[harness] {stderr}"));
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(state) = self.states.get_mut(target_name) {
                            state.logs.push(format!("[harness] gen-harness failed: {e}"));
                        }
                    }
                }
                // Drop crates not path-dep'd by veil_bin (leftovers from older multi gens)
                if let Err(e) = prune_unreferenced_crates(&output_path) {
                    if let Some(state) = self.states.get_mut(target_name) {
                        state.logs.push(format!("[gen] prune warning: {e}"));
                    }
                }
                // Merge workspace members + ensure workspace.dependencies cover
                // every workspace=true dep (including aws-config companions).
                if let Err(e) = merge_workspace_members(&output_path) {
                    if let Some(state) = self.states.get_mut(target_name) {
                        state.logs.push(format!("[gen] workspace merge warning: {e}"));
                    }
                }
            }
        }

        if let Some(state) = self.states.get_mut(target_name) {
            state.last_gen = Some(now_iso());
            state.last_gen_instant = Some(Instant::now());
            state.last_error = None;
            state.status = if self.processes.contains_key(target_name) {
                TargetStatus::Running
            } else {
                TargetStatus::Stopped
            };
        }

        Ok(())
    }

    /// Run `cargo check` in the output dir to verify generated code compiles.
    /// Returns true if check passes. Logs errors to the target's log buffer.
    fn check_build(&mut self, output_path: &Path, target_name: &str) -> bool {
        self.check_build_pkgs(output_path, target_name, &[])
    }

    /// `cargo check` optionally limited to `-p` packages (empty = whole workspace).
    fn check_build_pkgs(
        &mut self,
        output_path: &Path,
        target_name: &str,
        packages: &[String],
    ) -> bool {
        let mut cmd = Command::new("cargo");
        cmd.arg("check");
        for p in packages {
            cmd.arg("-p").arg(p);
        }
        cmd.current_dir(output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let result = cmd.output();

        match result {
            Ok(output) => {
                if output.status.success() {
                    if let Some(state) = self.states.get_mut(target_name) {
                        let scope = if packages.is_empty() {
                            "workspace".into()
                        } else {
                            packages.join(", ")
                        };
                        state
                            .logs
                            .push(format!("[check] ✓ build OK ({scope})"));
                        state.last_error = None;
                    }
                    true
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    // Extract just the error lines (not warnings) for concise display
                    let errors: String = stderr
                        .lines()
                        .filter(|l| l.contains("error[") || l.contains("error:"))
                        .take(15)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Some(state) = self.states.get_mut(target_name) {
                        state.status = TargetStatus::Error;
                        state.last_error = Some(format!("build check failed:\n{}", errors));
                        state.logs.push(format!("[check] ✗ build failed:\n{}", errors));
                        if state.logs.len() > 100 {
                            state.logs.drain(0..state.logs.len() - 100);
                        }
                    }
                    false
                }
            }
            Err(e) => {
                if let Some(state) = self.states.get_mut(target_name) {
                    state.logs.push(format!("[check] failed to run cargo check: {e}"));
                }
                false
            }
        }
    }

    /// Run a single `veil gen` invocation.
    /// When `no_prune` is true, pass `--no-prune` so multi-package gens keep sibling crates.
    fn run_gen(
        &mut self,
        package_path: &Path,
        target_lang: &str,
        output_path: &Path,
        target_name: &str,
        no_prune: bool,
    ) -> Result<(), String> {
        let mut cmd = Command::new(&self.veil_bin);
        cmd.arg("gen")
            .arg(package_path)
            .arg("-t")
            .arg(target_lang)
            .arg("-o")
            .arg(output_path);
        if no_prune {
            cmd.arg("--no-prune");
        }
        let result = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if let Some(state) = self.states.get_mut(target_name) {
                    if !stdout.is_empty() {
                        state.logs.push(format!("[gen] {stdout}"));
                    }
                    if !stderr.is_empty() {
                        state.logs.push(format!("[gen] {stderr}"));
                    }
                    if state.logs.len() > 100 {
                        state.logs.drain(0..state.logs.len() - 100);
                    }
                }
                if !output.status.success() {
                    let pkg_name = package_path.file_name().unwrap_or_default().to_string_lossy();
                    let err = format!("veil gen failed for {pkg_name} (exit {}): {stderr}", output.status);
                    if let Some(state) = self.states.get_mut(target_name) {
                        state.status = TargetStatus::Error;
                        state.last_error = Some(err.clone());
                    }
                    return Err(err);
                }
                Ok(())
            }
            Err(e) => {
                let err = format!("failed to run veil gen: {e}");
                if let Some(state) = self.states.get_mut(target_name) {
                    state.status = TargetStatus::Error;
                    state.last_error = Some(err.clone());
                }
                Err(err)
            }
        }
    }

    /// Spawn the dev server for a target.
    pub fn start_dev_server(&mut self, target_name: &str) -> Result<(), String> {
        let target = self
            .targets
            .iter()
            .find(|t| t.name == target_name)
            .ok_or_else(|| format!("unknown target: {target_name}"))?
            .clone();

        let dev_command = target
            .dev_command
            .as_deref()
            .ok_or_else(|| format!("no dev_command configured for target '{target_name}'"))?;

        // Kill existing process if any
        self.stop_dev_server(target_name);

        let output_path = self.project_root.join(&target.output);
        let _ = std::fs::create_dir_all(&output_path);

        // Split command into program + args
        let parts: Vec<&str> = dev_command.split_whitespace().collect();
        if parts.is_empty() {
            return Err("empty dev_command".into());
        }

        if let Some(state) = self.states.get_mut(target_name) {
            state.status = TargetStatus::Starting;
            state.logs.push(format!("[dev] starting: {dev_command}"));
        }

        // Run through shell to support && and pipes
        let child = Command::new("sh")
            .arg("-c")
            .arg(dev_command)
            .current_dir(&output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                let err = format!("failed to spawn '{dev_command}': {e}");
                if let Some(state) = self.states.get_mut(target_name) {
                    state.status = TargetStatus::Error;
                    state.last_error = Some(err.clone());
                }
                err
            })?;

        self.processes.insert(
            target_name.to_string(),
            ManagedProcess {
                child,
                started_at: Instant::now(),
            },
        );

        if let Some(state) = self.states.get_mut(target_name) {
            state.status = TargetStatus::Running;
            state.attached = false;
            state.last_error = None;
        }

        Ok(())
    }

    /// Stop the dev server for a target.
    pub fn stop_dev_server(&mut self, target_name: &str) {
        if let Some(mut proc) = self.processes.remove(target_name) {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
            if let Some(state) = self.states.get_mut(target_name) {
                state.status = TargetStatus::Stopped;
                state.attached = false;
                state.logs.push("[dev] stopped".into());
            }
        } else if let Some(state) = self.states.get_mut(target_name) {
            // Attached (external) server — attempt to kill the process on the port.
            if state.attached {
                let port = self
                    .targets
                    .iter()
                    .find(|t| t.name == target_name)
                    .and_then(|t| t.dev_port);
                if let Some(port) = port {
                    kill_process_on_port(port);
                    state.logs.push(format!("[dev] killed process on port {port}"));
                }
                state.status = TargetStatus::Stopped;
                state.attached = false;
            }
        }
    }

    /// Start a target: gen first, then spawn dev server.
    /// If the target is already attached (external server detected on port),
    /// only run gen (the watcher will keep it updated).
    pub fn start(&mut self, target_name: &str) -> Result<(), String> {
        let already_attached = self
            .states
            .get(target_name)
            .map(|s| s.attached && s.status == TargetStatus::Running)
            .unwrap_or(false);

        // Gen + cargo check; restore previous gen tree if check fails.
        self.generate_checked(target_name)?;

        if already_attached {
            if let Some(state) = self.states.get_mut(target_name) {
                state.status = TargetStatus::Running;
                state.logs.push("[dev] server already running (attached); gen updated".into());
            }
            Ok(())
        } else {
            self.start_dev_server(target_name)
        }
    }

    /// Start all targets.
    pub fn start_all(&mut self) -> Vec<(String, Result<(), String>)> {
        let names: Vec<String> = self.targets.iter().map(|t| t.name.clone()).collect();
        names
            .iter()
            .map(|n| (n.clone(), self.start(n)))
            .collect()
    }

    /// Stop all targets.
    pub fn stop_all(&mut self) {
        let names: Vec<String> = self.processes.keys().cloned().collect();
        for name in names {
            self.stop_dev_server(&name);
        }
        // Signal file watcher to stop
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.try_send(());
        }
    }

    /// Get status of all targets.
    pub fn status(&self) -> Vec<&TargetState> {
        self.states.values().collect()
    }

    /// Get status of one target.
    pub fn target_status(&self, name: &str) -> Option<&TargetState> {
        self.states.get(name)
    }

    /// Check if any processes have died and update status.
    /// Also re-probes attached (external) targets to detect if they stopped.
    pub fn poll_health(&mut self) {
        let mut dead: Vec<String> = Vec::new();
        for (name, proc) in &mut self.processes {
            match proc.child.try_wait() {
                Ok(Some(status)) => {
                    dead.push(name.clone());
                    if let Some(state) = self.states.get_mut(name) {
                        if status.success() {
                            state.status = TargetStatus::Stopped;
                            state.logs.push("[dev] exited normally".into());
                        } else {
                            state.status = TargetStatus::Error;
                            let msg = format!("[dev] exited with {status}");
                            state.last_error = Some(msg.clone());
                            state.logs.push(msg);
                        }
                    }
                }
                Ok(None) => {} // still running
                Err(e) => {
                    dead.push(name.clone());
                    if let Some(state) = self.states.get_mut(name) {
                        state.status = TargetStatus::Error;
                        state.last_error = Some(format!("wait error: {e}"));
                    }
                }
            }
        }
        for name in dead {
            self.processes.remove(&name);
        }

        // Re-probe attached targets: if port is no longer listening, mark stopped.
        // Also discover servers that came up since last check (stopped → attached).
        for target in &self.targets {
            let Some(state) = self.states.get(&target.name) else { continue };
            let Some(port) = target.dev_port else { continue };

            if state.attached && state.status == TargetStatus::Running {
                // Already attached — check it's still alive.
                if !probe_port(port) {
                    if let Some(state) = self.states.get_mut(&target.name) {
                        state.status = TargetStatus::Stopped;
                        state.attached = false;
                        state.logs.push(format!(
                            "[reattach] port {port} no longer responding — server stopped"
                        ));
                    }
                }
            } else if state.status == TargetStatus::Stopped
                && !self.processes.contains_key(&target.name)
            {
                // Target is stopped and we don't own a process — probe for external server.
                if probe_port(port) {
                    if let Some(state) = self.states.get_mut(&target.name) {
                        state.status = TargetStatus::Running;
                        state.attached = true;
                        state.logs.push(format!(
                            "[reattach] detected running server on port {port}"
                        ));
                    }
                }
            }
        }
    }

    /// Generate for all targets that watch the given file.
    pub fn on_file_changed(&mut self, changed_path: &Path) {
        let rel = changed_path
            .strip_prefix(&self.project_root)
            .unwrap_or(changed_path);
        let rel_str = rel.to_string_lossy();

        // Ignore paths inside output directories
        for target in &self.targets {
            if rel_str.starts_with(&target.output) {
                return;
            }
        }
        // Ignore hidden directories (.kiro, .git, etc.)
        if rel.components().any(|c| {
            c.as_os_str()
                .to_str()
                .map(|s| s.starts_with('.'))
                .unwrap_or(false)
        }) {
            return;
        }

        // Debounce: skip if last gen was less than 2 seconds ago
        let now = std::time::Instant::now();
        let too_recent = self.states.values().any(|s| {
            s.last_gen_instant
                .map(|t| now.duration_since(t).as_secs() < 2)
                .unwrap_or(false)
        });
        if too_recent {
            return;
        }

        // Find targets whose package matches the changed file
        let affected: Vec<String> = self
            .targets
            .iter()
            .filter(|t| {
                // Direct match on the target's package file
                rel_str == t.package
                    // Or match against dev packages (changes to a dev package re-gen the backend)
                    || self.dev_packages.as_ref().map(|pkgs| {
                        pkgs.iter().any(|p| {
                            if std::path::Path::new(p).is_absolute() {
                                changed_path.to_string_lossy() == *p
                            } else {
                                rel_str == *p
                            }
                        })
                    }).unwrap_or(false)
                    // Or any .layer change (layers affect all targets)
                    || rel_str.ends_with(".layer")
            })
            .map(|t| t.name.clone())
            .collect();

        if self.smoke_in_progress {
            // Agent smoke path owns gen+check; avoid double gen races.
            return;
        }

        for name in affected {
            if self.processes.contains_key(&name)
                || self.states.get(&name).map(|s| s.attached).unwrap_or(false)
            {
                // Gen + cargo check; on failure restore previous gen (and last-good
                // sources) so the running backend is not left on broken code.
                if let Err(e) = self.generate_checked(&name) {
                    if let Some(state) = self.states.get_mut(&name) {
                        state.logs.push(format!("[smoke] rejected change: {e}"));
                        if state.logs.len() > 100 {
                            state.logs.drain(0..state.logs.len() - 100);
                        }
                    }
                    // Roll package sources back if we have a last-good snapshot.
                    if !self.last_good_sources.is_empty() {
                        if let Ok(()) = self.restore_good_sources() {
                            let _ = self.generate(&name);
                        }
                    }
                }
            }
        }
    }

    pub fn set_stop_tx(&mut self, tx: mpsc::Sender<()>) {
        self.stop_tx = Some(tx);
    }

    /// Check if a file watcher stop channel is active (watcher is running).
    pub fn stop_tx_active(&self) -> bool {
        self.stop_tx.is_some()
    }

    /// Probe configured `dev_port`s to detect servers that are still running
    /// from a previous IDE session. Mark them as `Running` + `attached` so the
    /// toolbar shows them correctly and the file watcher can be started for
    /// codegen. Returns the number of re-attached targets.
    pub fn probe_existing_servers(&mut self) -> usize {
        let mut count = 0;
        for target in &self.targets {
            let Some(port) = target.dev_port else { continue };
            if probe_port(port) {
                if let Some(state) = self.states.get_mut(&target.name) {
                    state.status = TargetStatus::Running;
                    state.attached = true;
                    state.logs.push(format!(
                        "[reattach] detected running server on port {port}"
                    ));
                    count += 1;
                }
            }
        }
        count
    }

    /// True if any target is running (owned or attached).
    pub fn any_running(&self) -> bool {
        self.states.values().any(|s| s.status == TargetStatus::Running)
    }
}

impl Drop for DevLoop {
    fn drop(&mut self) {
        self.stop_all();
    }
}

// ─── Shared State ──────────────────────────────────────────────────────────

/// Thread-safe shared dev-loop state, keyed by project name.
pub type SharedDevLoops = Arc<Mutex<HashMap<String, DevLoop>>>;

/// Get or create a DevLoop for a project.
/// On first creation, probes configured ports to detect already-running dev
/// servers (e.g. from a previous IDE session). If any are found, marks them
/// as attached and starts the file watcher for live codegen.
pub fn get_or_create_dev_loop(
    loops: &SharedDevLoops,
    project_name: &str,
    project_root: &Path,
) -> Result<(), String> {
    let mut map = loops.lock().map_err(|e| format!("lock: {e}"))?;
    if map.contains_key(project_name) {
        return Ok(());
    }
    let config = parse_project_config(project_root)?;
    if config.targets.is_empty() {
        return Err(format!(
            "no [[targets]] in {}/veil.toml — add target config first",
            project_root.display()
        ));
    }
    let veil_bin = resolve_veil_bin();
    let dev_packages = config.dev.as_ref().map(|d| d.packages.clone()).filter(|v| !v.is_empty());
    let mut dev = DevLoop::new(project_root.to_path_buf(), veil_bin, config.targets, dev_packages);

    // Probe configured ports to detect running servers from a previous session.
    let attached = dev.probe_existing_servers();
    if attached > 0 {
        tracing::info!(
            project = project_name,
            attached,
            "re-attached to {attached} running dev server(s)"
        );
    }

    let should_start_watcher = dev.any_running();
    map.insert(project_name.to_string(), dev);
    drop(map);

    // Auto-start file watcher so that codegen runs on .veil/.layer changes
    // even for attached (externally-running) targets.
    if should_start_watcher {
        let _ = start_file_watcher(
            loops.clone(),
            project_name.to_string(),
            project_root.to_path_buf(),
        );
    }

    Ok(())
}

/// Binary used for `veil gen` inside the dev loop.
/// Prefer `VEIL_BIN`, then this process (usually `veil serve`), then PATH `veil`.
fn resolve_veil_bin() -> PathBuf {
    if let Ok(p) = std::env::var("VEIL_BIN") {
        let pb = PathBuf::from(&p);
        if pb.is_file() || p.contains('/') || p.contains('\\') {
            return pb;
        }
        // Bare name from env — still try as PATH command
        return pb;
    }
    if let Ok(exe) = std::env::current_exe() {
        if exe.is_file() {
            return exe;
        }
    }
    PathBuf::from("veil")
}

// ─── File Watcher ──────────────────────────────────────────────────────────

/// Start a file watcher for a project's .veil and .layer files.
/// Returns a stop channel sender.
pub fn start_file_watcher(
    loops: SharedDevLoops,
    project_name: String,
    project_root: PathBuf,
) -> Result<mpsc::Sender<()>, String> {
    use notify::{Event, RecursiveMode, Watcher};

    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            let _ = notify_tx.send(event);
        }
    })
    .map_err(|e| format!("watcher init: {e}"))?;

    watcher
        .watch(&project_root, RecursiveMode::Recursive)
        .map_err(|e| format!("watch: {e}"))?;

    let name = project_name.clone();
    let loops_for_watcher = loops.clone();
    let project_root_for_watcher = project_root.clone();
    tokio::task::spawn_blocking(move || {
        let _watcher = watcher; // keep alive
        // Collect output dirs to exclude from watch events.
        let exclude_dirs: Vec<PathBuf> = loops_for_watcher
            .lock()
            .ok()
            .and_then(|map| {
                map.get(&name).map(|dev| {
                    dev.targets
                        .iter()
                        .map(|t| project_root_for_watcher.join(&t.output))
                        .collect()
                })
            })
            .unwrap_or_default();

        loop {
            // Check stop signal (non-blocking)
            if stop_rx.try_recv().is_ok() {
                break;
            }
            // Wait for file events (with timeout so we can check stop)
            match notify_rx.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(event) => {
                    // Filter: only .veil/.layer files NOT inside output dirs
                    let dominated = event.paths.iter().any(|p| {
                        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext != "veil" && ext != "layer" {
                            return false;
                        }
                        // Exclude paths under any output directory
                        !exclude_dirs.iter().any(|d| p.starts_with(d))
                    });
                    if dominated {
                        if let Ok(mut map) = loops_for_watcher.lock() {
                            if let Some(dev) = map.get_mut(&name) {
                                for path in &event.paths {
                                    dev.on_file_changed(path);
                                }
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    // Store the stop_tx in the DevLoop
    if let Ok(mut map) = loops.lock() {
        if let Some(dev) = map.get_mut(&project_name) {
            dev.set_stop_tx(stop_tx.clone());
        }
    }

    Ok(stop_tx)
}

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Pick `-p` crates for smoke after a source change.
/// Prefer a context crate whose name matches the .veil stem (e.g. wear_test.veil
/// → wear_test) so a broken multi-package sibling (iaaa) doesn't block edits.
fn check_packages_for_change(output_path: &Path, changed_rel: Option<&str>) -> Vec<String> {
    let crates_dir = output_path.join("crates");
    let mut members: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&crates_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == "veil_shared" || name == "veil_bin" {
                continue;
            }
            if e.path().join("Cargo.toml").is_file() {
                members.push(name);
            }
        }
    }
    members.sort();
    let Some(rel) = changed_rel else {
        return members;
    };
    let rel = rel.replace('\\', "/");
    if rel.ends_with(".layer") {
        // Layers affect all packages — check every context crate.
        return members;
    }
    let stem = Path::new(&rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if !stem.is_empty() {
        if members.iter().any(|m| m == &stem) {
            return vec![stem];
        }
        // e.g. dlx_core.veil → crate iaaa: fall back to all context crates
        // but still avoid requiring veil_bin if we can check members.
    }
    members
}

/// Copy a directory tree to a temp backup path (for smoke-test rollback).
fn backup_tree(src: &Path) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dest = std::env::temp_dir().join(format!(
        "veil-gen-bak-{}-{}",
        stamp,
        src.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("out")
    ));
    copy_dir_all(src, &dest)?;
    Ok(dest)
}

fn restore_tree(backup: &Path, dest: &Path) -> Result<(), String> {
    if !backup.is_dir() {
        return Ok(());
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| format!("clear dest: {e}"))?;
    }
    copy_dir_all(backup, dest)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// After multi-package gen-harness, drop `crates/*` that veil_bin does not
/// path-depend on (plus always keep veil_shared / veil_bin).
fn prune_unreferenced_crates(output_path: &Path) -> Result<(), String> {
    let crates_dir = output_path.join("crates");
    if !crates_dir.is_dir() {
        return Ok(());
    }
    let bin_cargo = crates_dir.join("veil_bin").join("Cargo.toml");
    let mut keep: std::collections::HashSet<String> = ["veil_shared", "veil_bin"]
        .into_iter()
        .map(String::from)
        .collect();
    if let Ok(bin) = std::fs::read_to_string(&bin_cargo) {
        // `name = { path = "../name" }`
        for line in bin.lines() {
            let t = line.trim();
            if let Some(rest) = t.split_once('=') {
                let key = rest.0.trim();
                let val = rest.1.trim();
                if val.contains("path") && val.contains("..") {
                    keep.insert(key.to_string());
                }
            }
        }
    }
    for entry in std::fs::read_dir(&crates_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() || keep.contains(&name) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => tracing::info!(crate = %name, "pruned unreferenced gen crate"),
            Err(e) => {
                return Err(format!("could not prune {}: {e}", path.display()));
            }
        }
    }
    Ok(())
}

/// After multi-package gen, scan `crates/` for all subdirs and ensure the
/// workspace Cargo.toml lists them all as members. Handles the case where
/// the second `veil gen` overwrote the Cargo.toml from the first.
/// Also ensures `[workspace.dependencies]` covers every `workspace = true`
/// dep referenced by member crates **including veil_bin** (e.g. aws-config).
fn merge_workspace_members(output_path: &Path) -> Result<(), String> {
    let crates_dir = output_path.join("crates");
    if !crates_dir.is_dir() {
        return Ok(());
    }
    // Discover all crate directories
    let mut members: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() && path.join("Cargo.toml").is_file() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            members.push(name);
        }
    }
    if members.is_empty() {
        return Ok(());
    }
    // Sort with veil_shared first, veil_bin second, then alpha
    members.sort_by(|a, b| {
        let order = |s: &str| -> u8 {
            if s == "veil_shared" { 0 }
            else if s == "veil_bin" { 1 }
            else { 2 }
        };
        order(a).cmp(&order(b)).then(a.cmp(b))
    });

    // Read existing Cargo.toml and patch the members list
    let cargo_path = output_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_path).map_err(|e| e.to_string())?;

    // Build new members block
    let members_str = members
        .iter()
        .map(|m| format!("    \"crates/{}\"", m))
        .collect::<Vec<_>>()
        .join(",\n");

    // Replace the members = [...] block
    let new_content = if let Some(start) = content.find("members = [") {
        if let Some(end) = content[start..].find(']') {
            let before = &content[..start];
            let after = &content[start + end + 1..];
            format!("{}members = [\n{}\n]{}", before, members_str, after)
        } else {
            content
        }
    } else {
        content
    };

    std::fs::write(&cargo_path, new_content).map_err(|e| e.to_string())?;

    // Ensure veil_bin path-depends on all context crates (gen-harness usually
    // already wrote these; fill any gap if a crate exists without a dep line).
    let bin_cargo = output_path.join("crates/veil_bin/Cargo.toml");
    if bin_cargo.is_file() {
        let bin_content = std::fs::read_to_string(&bin_cargo).map_err(|e| e.to_string())?;
        let mut new_bin = bin_content.clone();
        for m in &members {
            if m == "veil_shared" || m == "veil_bin" {
                continue;
            }
            let dep_line = format!("{} = {{ path = \"../{}\" }}", m, m);
            // Match key as a dependency name, not a substring of another key.
            let key_pat = format!("\n{} = ", m);
            let key_start = format!("{} = ", m);
            if !new_bin.contains(&key_pat) && !new_bin.lines().any(|l| l.trim_start().starts_with(&key_start)) {
                if let Some(deps_pos) = new_bin.find("[dependencies]") {
                    let insert_pos = new_bin[deps_pos..]
                        .find('\n')
                        .map(|p| deps_pos + p + 1)
                        .unwrap_or(new_bin.len());
                    let after_deps = &new_bin[insert_pos..];
                    let section_end = after_deps.find("\n[").unwrap_or(after_deps.len());
                    let insert_at = insert_pos + section_end;
                    new_bin.insert_str(insert_at, &format!("{}\n", dep_line));
                }
            }
        }
        if new_bin != bin_content {
            std::fs::write(&bin_cargo, new_bin).map_err(|e| e.to_string())?;
        }
    }

    // Merge workspace.dependencies: scan **all** member crates (including veil_bin)
    // for workspace=true deps and ensure they exist at the workspace root.
    let ws_content = std::fs::read_to_string(&cargo_path).map_err(|e| e.to_string())?;
    let mut needed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for m in &members {
        let crate_cargo = crates_dir.join(m).join("Cargo.toml");
        if let Ok(c) = std::fs::read_to_string(&crate_cargo) {
            for line in c.lines() {
                let t = line.trim();
                // `foo.workspace = true` or `foo = { workspace = true }`
                if let Some(name) = t.strip_suffix(".workspace = true") {
                    let name = name.trim();
                    if !name.is_empty() {
                        needed.insert(name.to_string());
                    }
                } else if t.contains("workspace = true") || t.contains("workspace=true") {
                    if let Some((key, _)) = t.split_once('=') {
                        let key = key.trim();
                        if !key.is_empty() && !key.starts_with('[') {
                            needed.insert(key.to_string());
                        }
                    }
                }
            }
        }
    }

    // Known defaults for common SDK stubs (gen-harness may also patch from stubs).
    let default_for = |name: &str| -> Option<String> {
        match name {
            "aws-sdk-dynamodb" => Some("aws-sdk-dynamodb = \"1.117.0\"".into()),
            "aws-config" => Some("aws-config = \"1\"".into()),
            "sqlx" => Some(
                "sqlx = { version = \"0.8\", features = [\"runtime-tokio-rustls\", \"postgres\"] }"
                    .into(),
            ),
            "tokio" | "async-trait" | "thiserror" | "serde" | "uuid" | "chrono" | "tracing"
            | "serde_json" => None, // always present in gen template
            _ => None,
        }
    };

    let mut extra_ws_deps: Vec<String> = Vec::new();
    for name in &needed {
        // Already declared in workspace root?
        if ws_content.lines().any(|l| {
            let t = l.trim();
            t.starts_with(&format!("{name} =")) || t.starts_with(&format!("{name}="))
        }) {
            continue;
        }
        if let Some(line) = default_for(name) {
            extra_ws_deps.push(line);
        }
    }

    if !extra_ws_deps.is_empty() {
        let mut patched = ws_content;
        for dep in &extra_ws_deps {
            let dep_name = dep.split('=').next().unwrap_or("").trim();
            if dep_name.is_empty() || patched.contains(&format!("{dep_name} =")) {
                continue;
            }
            if let Some(pos) = patched.find("[workspace.dependencies]") {
                if let Some(nl) = patched[pos..].find('\n') {
                    let insert = pos + nl + 1;
                    let after = &patched[insert..];
                    let end = after.find("\n[").unwrap_or(after.len());
                    let insert_at = insert + end;
                    patched.insert_str(insert_at, &format!("{}\n", dep));
                }
            }
        }
        std::fs::write(&cargo_path, patched).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Check if a TCP port is currently accepting connections (something is listening).
fn probe_port(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// Kill whatever process is listening on the given TCP port.
/// Uses `fuser -k` on Linux; falls back to lsof+kill on macOS.
fn kill_process_on_port(port: u16) {
    // Try fuser first (Linux, common)
    let result = Command::new("fuser")
        .arg("-k")
        .arg(format!("{port}/tcp"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Ok(s) = result {
        if s.success() {
            return;
        }
    }
    // Fallback: lsof + kill (macOS / systems without fuser)
    if let Ok(output) = Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}")])
        .output()
    {
        let pids = String::from_utf8_lossy(&output.stdout);
        for pid in pids.split_whitespace() {
            if let Ok(p) = pid.trim().parse::<u32>() {
                let _ = Command::new("kill")
                    .arg(p.to_string())
                    .status();
            }
        }
    }
}

fn now_iso() -> String {
    // Simple timestamp without chrono dep
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", dur.as_secs())
}
