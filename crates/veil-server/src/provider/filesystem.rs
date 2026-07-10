//! Filesystem-backed source provider — reads/writes .veil files from disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use veil_ir::LayerRegistry;

use super::{FileInfo, SourceProvider};

/// Local filesystem provider for `veil-cli serve`.
pub struct FilesystemProvider {
    files: Vec<FileEntry>,
    registry: LayerRegistry,
    active: Mutex<usize>,
}

struct FileEntry {
    path: PathBuf,
    name: String,
    source: Mutex<String>,
    editable: bool,
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
            }],
            registry,
            active: Mutex::new(0),
        }
    }

    /// Create a provider for multiple files.
    pub fn with_files(files: Vec<(PathBuf, String, bool)>, registry: LayerRegistry) -> Self {
        let entries = files.into_iter().map(|(path, source, editable)| {
            let name = path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            FileEntry { path, name, source: Mutex::new(source), editable }
        }).collect();
        FilesystemProvider {
            files: entries,
            registry,
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
        self.files.iter().enumerate().map(|(i, entry)| FileInfo {
            name: entry.name.clone(),
            path: entry.path.to_string_lossy().to_string(),
            editable: entry.editable,
        }).collect()
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
        Ok(())
    }

    fn registry(&self) -> &LayerRegistry {
        &self.registry
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
}
