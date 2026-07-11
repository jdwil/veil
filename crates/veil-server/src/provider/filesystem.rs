//! Filesystem-backed source provider — packages (`.veil`) and layers (`.layer`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use async_trait::async_trait;
use veil_ir::LayerRegistry;

use super::{FileInfo, FileKind, SourceProvider};

/// Local filesystem provider for `veil-cli serve`.
pub struct FilesystemProvider {
    files: Mutex<Vec<FileEntry>>,
    active: Mutex<usize>,
    /// Project root directory (IDE session is always one project).
    project_root: Option<PathBuf>,
}

struct FileEntry {
    path: PathBuf,
    name: String,
    kind: FileKind,
    source: Mutex<String>,
    editable: bool,
    /// Layers for this file's `use` lines (reloaded on write when content changes).
    registry: Mutex<LayerRegistry>,
}

fn registry_for_entry(path: &std::path::Path, source: &str) -> LayerRegistry {
    match FileKind::from_path(path) {
        FileKind::Layer => {
            let mut reg = LayerRegistry::builtin();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "layer".into());
            let dir = path.parent().unwrap_or(std::path::Path::new("."));
            // Resolve `use` deps first
            for line in source.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("use ") {
                    let dep = rest.split_whitespace().next().unwrap_or("");
                    if !dep.is_empty() {
                        let _ = reg.load_layer(dep, dir);
                    }
                }
            }
            if let Err(e) = reg.load_content(&name, source) {
                eprintln!("warning: layer registry for {}: {e}", path.display());
            }
            reg
        }
        FileKind::Stub => LayerRegistry::builtin(),
        FileKind::Package => LayerRegistry::for_veil_file(path).unwrap_or_else(|e| {
            eprintln!(
                "warning: layers for {}: {e} — using builtin only",
                path.display()
            );
            LayerRegistry::builtin()
        }),
    }
}

impl FilesystemProvider {
    /// Create a provider for a single file.
    pub fn new(path: PathBuf, source: String, registry: LayerRegistry, editable: bool) -> Self {
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let kind = FileKind::from_path(&path);
        let project_root = path.parent().map(|p| p.to_path_buf());
        FilesystemProvider {
            files: Mutex::new(vec![FileEntry {
                path,
                name,
                kind,
                source: Mutex::new(source),
                editable,
                registry: Mutex::new(registry),
            }]),
            active: Mutex::new(0),
            project_root,
        }
    }

    /// Create a provider for multiple files (packages + layers).
    pub fn with_files(files: Vec<(PathBuf, String, bool)>, _shared: LayerRegistry) -> Self {
        Self::with_files_in_project(files, _shared, None)
    }

    /// Multi-file provider with an explicit project root (for `/api/project`).
    pub fn with_files_in_project(
        files: Vec<(PathBuf, String, bool)>,
        _shared: LayerRegistry,
        project_root: Option<PathBuf>,
    ) -> Self {
        let entries: Vec<FileEntry> = files
            .into_iter()
            .map(|(path, source, editable)| {
                let name = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let kind = FileKind::from_path(&path);
                let registry = registry_for_entry(&path, &source);
                FileEntry {
                    path,
                    name,
                    kind,
                    source: Mutex::new(source),
                    editable,
                    registry: Mutex::new(registry),
                }
            })
            .collect();
        let project_root = project_root.or_else(|| {
            entries
                .first()
                .and_then(|e| e.path.parent().map(|p| p.to_path_buf()))
        });
        FilesystemProvider {
            files: Mutex::new(entries),
            active: Mutex::new(0),
            project_root,
        }
    }

    /// Project root for the active IDE session, if known.
    pub fn project_root(&self) -> Option<PathBuf> {
        self.project_root.clone()
    }

    pub fn active_index(&self) -> usize {
        *self.active.lock().unwrap()
    }

    pub fn set_active(&self, idx: usize) -> Result<(), String> {
        let n = self.files.lock().unwrap().len();
        if idx >= n {
            return Err("invalid file index".to_string());
        }
        *self.active.lock().unwrap() = idx;
        Ok(())
    }

    pub fn active_name(&self) -> String {
        let files = self.files.lock().unwrap();
        files[self.active_index()].name.clone()
    }

    fn entry_index(&self, file: &str) -> Result<usize, String> {
        if file.is_empty() {
            return Ok(self.active_index());
        }
        let files = self.files.lock().unwrap();
        files
            .iter()
            .position(|e| e.name == file || e.path.to_string_lossy() == file)
            .ok_or_else(|| format!("file not found: {file}"))
    }

    /// After a layer write, rebuild registries for packages that use it (DSL-004).
    fn reload_dependents_of_layer(&self, layer_name: &str) {
        let files = self.files.lock().unwrap();
        for entry in files.iter() {
            if entry.kind != FileKind::Package {
                continue;
            }
            let src = entry.source.lock().unwrap().clone();
            let uses_layer = src.lines().any(|line| {
                let t = line.trim();
                t.strip_prefix("use ")
                    .map(|rest| rest.split_whitespace().next() == Some(layer_name))
                    .unwrap_or(false)
            });
            if uses_layer {
                if let Ok(reg) = LayerRegistry::for_veil_file(&entry.path) {
                    *entry.registry.lock().unwrap() = reg;
                }
            }
        }
    }
}

#[async_trait]
impl SourceProvider for FilesystemProvider {
    async fn list_files(&self) -> Vec<FileInfo> {
        let active_idx = self.active_index();
        let files = self.files.lock().unwrap();
        files
            .iter()
            .enumerate()
            .map(|(i, entry)| FileInfo {
                index: i,
                name: entry.name.clone(),
                path: entry.path.to_string_lossy().to_string(),
                editable: entry.editable,
                active: i == active_idx,
                kind: entry.kind,
            })
            .collect()
    }

    async fn read_source(&self, file: &str) -> Result<String, String> {
        let idx = self.entry_index(file)?;
        let files = self.files.lock().unwrap();
        Ok(files[idx].source.lock().unwrap().clone())
    }

    async fn write_source(&self, file: &str, content: &str) -> Result<(), String> {
        let idx = self.entry_index(file)?;
        let (path, kind, editable, layer_stem) = {
            let files = self.files.lock().unwrap();
            let entry = &files[idx];
            if !entry.editable {
                return Err("file is read-only".to_string());
            }
            (
                entry.path.clone(),
                entry.kind,
                entry.editable,
                entry
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            )
        };
        let _ = editable;

        std::fs::write(&path, content).map_err(|e| format!("failed to write: {e}"))?;

        {
            let files = self.files.lock().unwrap();
            let entry = &files[idx];
            *entry.source.lock().unwrap() = content.to_string();
            *entry.registry.lock().unwrap() = registry_for_entry(&path, content);
        }

        if kind == FileKind::Layer {
            self.reload_dependents_of_layer(&layer_stem);
            // Also try pkg name from content
            if let Some(pkg_line) = content.lines().find(|l| l.trim_start().starts_with("pkg ")) {
                let name = pkg_line
                    .trim()
                    .strip_prefix("pkg ")
                    .unwrap_or("")
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if !name.is_empty() && name != layer_stem {
                    self.reload_dependents_of_layer(name);
                }
            }
        }

        crate::revision::bus().publish(
            content.len(),
            &path.to_string_lossy(),
            match kind {
                FileKind::Layer => "write_layer",
                _ => "write_source",
            },
        );
        Ok(())
    }

    fn registry(&self) -> LayerRegistry {
        let idx = self.active_index();
        let files = self.files.lock().unwrap();
        files[idx].registry.lock().unwrap().clone()
    }

    fn is_editable(&self, file: &str) -> bool {
        let idx = match self.entry_index(file) {
            Ok(i) => i,
            Err(_) => return false,
        };
        let files = self.files.lock().unwrap();
        files[idx].editable
    }

    fn file_kind(&self, file: &str) -> FileKind {
        let idx = match self.entry_index(file) {
            Ok(i) => i,
            Err(_) => return FileKind::Package,
        };
        let files = self.files.lock().unwrap();
        files[idx].kind
    }

    fn set_active(&self, index: usize) -> Result<(), String> {
        FilesystemProvider::set_active(self, index)
    }

    async fn baseline_source(&self, file: &str) -> Result<Option<(String, String)>, String> {
        let idx = self.entry_index(file)?;
        let path = {
            let files = self.files.lock().unwrap();
            files[idx].path.clone()
        };

        let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
        let parent = abs.parent().unwrap_or(std::path::Path::new("."));

        let root_out = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(parent)
            .output()
            .map_err(|e| format!("git unavailable: {e}"))?;
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
            .args(["show", &format!("HEAD:{rel}")])
            .current_dir(&root_path)
            .output()
            .map_err(|e| format!("git show failed: {e}"))?;
        if !show.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&show.stdout).to_string();
        Ok(Some(("git HEAD".into(), text)))
    }

    async fn reload_from_disk(&self) -> Result<usize, String> {
        let mut n = 0usize;
        let mut changed = false;
        let paths: Vec<(usize, PathBuf, FileKind)> = {
            let files = self.files.lock().unwrap();
            files
                .iter()
                .enumerate()
                .map(|(i, e)| (i, e.path.clone(), e.kind))
                .collect()
        };
        for (i, path, kind) in paths {
            let disk = std::fs::read_to_string(&path)
                .map_err(|e| format!("reload {}: {e}", path.display()))?;
            let files = self.files.lock().unwrap();
            let entry = &files[i];
            let mut guard = entry.source.lock().unwrap();
            if *guard != disk {
                *guard = disk.clone();
                drop(guard);
                *entry.registry.lock().unwrap() = registry_for_entry(&path, &disk);
                changed = true;
                n += 1;
                if kind == FileKind::Layer {
                    drop(files);
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.reload_dependents_of_layer(&stem);
                }
            }
        }
        if changed {
            let files = self.files.lock().unwrap();
            let active = &files[self.active_index()];
            let bytes = active.source.lock().unwrap().len();
            crate::revision::bus().publish(
                bytes,
                &active.path.to_string_lossy(),
                "reload_from_disk",
            );
        }
        Ok(n)
    }

    async fn layer_dependents(&self, layer_name: &str) -> Vec<FileInfo> {
        let active_idx = self.active_index();
        let files = self.files.lock().unwrap();
        files
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == FileKind::Package)
            .filter(|(_, e)| {
                let src = e.source.lock().unwrap();
                src.lines().any(|line| {
                    line.trim()
                        .strip_prefix("use ")
                        .map(|rest| rest.split_whitespace().next() == Some(layer_name))
                        .unwrap_or(false)
                })
            })
            .map(|(i, e)| FileInfo {
                index: i,
                name: e.name.clone(),
                path: e.path.to_string_lossy().to_string(),
                editable: e.editable,
                active: i == active_idx,
                kind: e.kind,
            })
            .collect()
    }

    fn register_file(
        &self,
        path: PathBuf,
        source: String,
        editable: bool,
    ) -> Result<usize, String> {
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        let kind = FileKind::from_path(&path);
        let registry = registry_for_entry(&path, &source);
        let mut files = self.files.lock().unwrap();
        let idx = files.len();
        files.push(FileEntry {
            path,
            name,
            kind,
            source: Mutex::new(source),
            editable,
            registry: Mutex::new(registry),
        });
        Ok(idx)
    }

    fn project_root(&self) -> Option<PathBuf> {
        FilesystemProvider::project_root(self)
    }
}
