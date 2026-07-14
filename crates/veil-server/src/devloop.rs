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
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// ─── Config (veil.toml) ────────────────────────────────────────────────────

/// Parsed `veil.toml` project config with dev targets.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub project: Option<ProjectMeta>,
    #[serde(default, rename = "targets")]
    pub targets: Vec<TargetConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectMeta {
    pub name: Option<String>,
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
}

impl DevLoop {
    pub fn new(project_root: PathBuf, veil_bin: PathBuf, targets: Vec<TargetConfig>) -> Self {
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
        }
    }

    /// Run `veil gen` for a target.
    pub fn generate(&mut self, target_name: &str) -> Result<(), String> {
        let target = self
            .targets
            .iter()
            .find(|t| t.name == target_name)
            .ok_or_else(|| format!("unknown target: {target_name}"))?
            .clone();

        if let Some(state) = self.states.get_mut(target_name) {
            state.status = TargetStatus::Generating;
        }

        let package_path = self.project_root.join(&target.package);
        let output_path = self.project_root.join(&target.output);

        // Ensure output dir exists
        let _ = std::fs::create_dir_all(&output_path);

        let result = Command::new(&self.veil_bin)
            .arg("gen")
            .arg(&package_path)
            .arg("-t")
            .arg(&target.target)
            .arg("-o")
            .arg(&output_path)
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
                    // Keep logs bounded
                    if state.logs.len() > 100 {
                        state.logs.drain(0..state.logs.len() - 100);
                    }
                }
                if output.status.success() {
                    if let Some(state) = self.states.get_mut(target_name) {
                        state.last_gen = Some(now_iso());
                        state.last_gen_instant = Some(Instant::now());
                        state.last_error = None;
                        // Restore to Running if process is alive, else Stopped
                        state.status = if self.processes.contains_key(target_name) {
                            TargetStatus::Running
                        } else {
                            TargetStatus::Stopped
                        };
                    }
                    Ok(())
                } else {
                    let err = format!("veil gen failed (exit {}): {stderr}", output.status);
                    if let Some(state) = self.states.get_mut(target_name) {
                        state.status = TargetStatus::Error;
                        state.last_error = Some(err.clone());
                    }
                    Err(err)
                }
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
                state.logs.push("[dev] stopped".into());
            }
        }
    }

    /// Start a target: gen first, then spawn dev server.
    pub fn start(&mut self, target_name: &str) -> Result<(), String> {
        self.generate(target_name)?;
        self.start_dev_server(target_name)?;
        Ok(())
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
                    // Or any .layer change (layers affect all targets)
                    || rel_str.ends_with(".layer")
            })
            .map(|t| t.name.clone())
            .collect();

        for name in affected {
            if self.processes.contains_key(&name) {
                // Only re-gen if the dev server is running
                let _ = self.generate(&name);
            }
        }
    }

    pub fn set_stop_tx(&mut self, tx: mpsc::Sender<()>) {
        self.stop_tx = Some(tx);
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
    let dev = DevLoop::new(project_root.to_path_buf(), veil_bin, config.targets);
    map.insert(project_name.to_string(), dev);
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

fn now_iso() -> String {
    // Simple timestamp without chrono dep
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", dur.as_secs())
}
