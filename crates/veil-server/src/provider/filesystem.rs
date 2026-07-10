//! Filesystem-backed source provider — reads/writes .veil files from disk.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use async_trait::async_trait;
use veil_ir::LayerRegistry;

use super::{FileInfo, SourceProvider};

/// Local filesystem provider for `veil-cli serve`.
pub struct FilesystemProvider {
    files: Vec<FileEntry>,
    active: Mutex<usize>,
}

struct FileEntry {
    path: PathBuf,
    name: String,
    source: Mutex<String>,
    editable: bool,
    /// Layers for this file's `use` lines (reloaded on write when content changes).
    registry: Mutex<LayerRegistry>,
}

impl FilesystemProvider {
    /// Create a provider for a single .veil file.
    pub fn new(path: PathBuf, source: String, registry: LayerRegistry, editable: bool) -> Self {
        let name = path.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        FilesystemProvider {
            files: vec![FileEntry {
                path,
                name,
                source: Mutex::new(source),
                editable,
                registry: Mutex::new(registry),
            }],
            active: Mutex::new(0),
        }
    }

    /// Create a provider for multiple files.
    ///
    /// Each file gets its own layer registry from `LayerRegistry::for_veil_file`
    /// so switching active file (e.g. to `dlx_core.veil`) loads ddd/di/sqlx etc.
    pub fn with_files(files: Vec<(PathBuf, String, bool)>, _shared: LayerRegistry) -> Self {
        let entries = files
            .into_iter()
            .map(|(path, source, editable)| {
                let name = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let registry = LayerRegistry::for_veil_file(&path).unwrap_or_else(|e| {
                    eprintln!(
                        "warning: layers for {}: {e} — using builtin only",
                        path.display()
                    );
                    LayerRegistry::builtin()
                });
                FileEntry {
                    path,
                    name,
                    source: Mutex::new(source),
                    editable,
                    registry: Mutex::new(registry),
                }
            })
            .collect();
        FilesystemProvider {
            files: entries,
            active: Mutex::new(0),
        }
    }

    /// Get/set the active file index.
    pub fn active_index(&self) -> usize {
        *self.active.lock().unwrap()
    }

    pub fn set_active(&self, idx: usize) -> Result<(), String> {
        if idx >= self.files.len() {
            return Err("invalid file index".to_string());
        }
        *self.active.lock().unwrap() = idx;
        Ok(())
    }

    /// Get the active file's name.
    pub fn active_name(&self) -> String {
        self.files[self.active_index()].name.clone()
    }
}

#[async_trait]
impl SourceProvider for FilesystemProvider {
    async fn list_files(&self) -> Vec<FileInfo> {
        let active_idx = self.active_index();
        self.files
            .iter()
            .enumerate()
            .map(|(i, entry)| FileInfo {
                index: i,
                name: entry.name.clone(),
                path: entry.path.to_string_lossy().to_string(),
                editable: entry.editable,
                active: i == active_idx,
            })
            .collect()
    }

    async fn read_source(&self, file: &str) -> Result<String, String> {
        // If file is empty or matches active, return active file
        let entry = if file.is_empty() {
            &self.files[self.active_index()]
        } else {
            self.files.iter().find(|e| e.name == file || e.path.to_string_lossy() == file)
                .ok_or_else(|| format!("file not found: {}", file))?
        };
        Ok(entry.source.lock().unwrap().clone())
    }

    async fn write_source(&self, file: &str, content: &str) -> Result<(), String> {
        let entry = if file.is_empty() {
            &self.files[self.active_index()]
        } else {
            self.files.iter().find(|e| e.name == file || e.path.to_string_lossy() == file)
                .ok_or_else(|| format!("file not found: {}", file))?
        };

        if !entry.editable {
            return Err("file is read-only".to_string());
        }

        // Write to disk
        std::fs::write(&entry.path, content)
            .map_err(|e| format!("failed to write: {}", e))?;

        // Update in-memory
        *entry.source.lock().unwrap() = content.to_string();
        // Refresh layers if `use` lines changed
        if let Ok(reg) = LayerRegistry::for_veil_file(&entry.path) {
            *entry.registry.lock().unwrap() = reg;
        }
        Ok(())
    }

    fn registry(&self) -> LayerRegistry {
        let idx = self.active_index();
        self.files[idx].registry.lock().unwrap().clone()
    }

    fn is_editable(&self, file: &str) -> bool {
        if file.is_empty() {
            return self.files[self.active_index()].editable;
        }
        self.files.iter()
            .find(|e| e.name == file || e.path.to_string_lossy() == file)
            .map(|e| e.editable)
            .unwrap_or(false)
    }

    fn set_active(&self, index: usize) -> Result<(), String> {
        FilesystemProvider::set_active(self, index)
    }

    async fn baseline_source(&self, file: &str) -> Result<Option<(String, String)>, String> {
        let entry = if file.is_empty() {
            &self.files[self.active_index()]
        } else {
            self.files
                .iter()
                .find(|e| e.name == file || e.path.to_string_lossy() == file)
                .ok_or_else(|| format!("file not found: {}", file))?
        };

        // Prefer path relative to a git root so `git show HEAD:path` works.
        let abs = entry
            .path
            .canonicalize()
            .unwrap_or_else(|_| entry.path.clone());
        let parent = abs.parent().unwrap_or(std::path::Path::new("."));

        let root_out = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(parent)
            .output()
            .map_err(|e| format!("git unavailable: {}", e))?;
        if !root_out.status.success() {
            return Ok(None);
        }
        let root = String::from_utf8_lossy(&root_out.stdout).trim().to_string();
        let root_path = PathBuf::from(&root);
        let rel = abs
            .strip_prefix(&root_path)
            .unwrap_or(&abs)
            .to_string_lossy()
            .to_string();

        let show = Command::new("git")
            .args(["show", &format!("HEAD:{}", rel)])
            .current_dir(&root_path)
            .output()
            .map_err(|e| format!("git show failed: {}", e))?;
        if !show.status.success() {
            // Untracked or missing at HEAD
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&show.stdout).to_string();
        Ok(Some(("git HEAD".into(), text)))
    }
}
