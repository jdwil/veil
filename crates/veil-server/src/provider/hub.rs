//! Multi-project hub: lazy per-product [`FilesystemProvider`] sessions (MP-002).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::filesystem::FilesystemProvider;
use crate::project_layout::{
    collect_project_files, create_project, ensure_project_shape, is_project_root,
    is_source_editable, list_projects, ProjectInfo,
};
use crate::provider::{FileInfo, FileKind, SourceProvider};
use async_trait::async_trait;
use veil_ir::LayerRegistry;

/// Request-scoped project name for multi-project providers.
tokio::task_local! {
    pub static CURRENT_PROJECT: String;
}

/// Lazy sessions keyed by product directory name under `projects_dir`.
pub struct ProjectsHub {
    projects_dir: PathBuf,
    show_core_layers: bool,
    sessions: Mutex<HashMap<String, Arc<FilesystemProvider>>>,
}

impl ProjectsHub {
    pub fn new(projects_dir: PathBuf, show_core_layers: bool) -> Self {
        Self {
            projects_dir,
            show_core_layers,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    pub fn list(&self) -> Result<Vec<ProjectInfo>, String> {
        list_projects(&self.projects_dir)
    }

    pub fn create(&self, name: &str) -> Result<ProjectInfo, String> {
        let info = create_project(&self.projects_dir, name)?;
        // Drop any stale session
        self.sessions.lock().unwrap().remove(name);
        Ok(info)
    }

    /// Open or return cached session for a product name.
    pub fn open(&self, name: &str) -> Result<Arc<FilesystemProvider>, String> {
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err("invalid project name".into());
        }
        {
            let map = self.sessions.lock().unwrap();
            if let Some(p) = map.get(name) {
                return Ok(p.clone());
            }
        }
        let root = self.projects_dir.join(name);
        if !root.is_dir() {
            return Err(format!("project not found: {name}"));
        }
        if !is_project_root(&root) {
            return Err(format!("not a VEIL project: {}", root.display()));
        }
        let _ = ensure_project_shape(&root);
        let paths = collect_project_files(&root, self.show_core_layers)?;
        let entries: Vec<(PathBuf, String, bool)> = paths
            .into_iter()
            .map(|path| {
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_default();
                let editable = is_source_editable(&path, &source);
                (path, source, editable)
            })
            .collect();
        if entries.is_empty() {
            return Err(format!("no packages in project {name}"));
        }
        let reg = LayerRegistry::for_veil_file(&entries[0].0).unwrap_or_else(|_| {
            LayerRegistry::builtin()
        });
        let provider =
            FilesystemProvider::with_files_in_project(entries, reg, Some(root));
        let arc = Arc::new(provider);
        self.sessions
            .lock()
            .unwrap()
            .insert(name.to_string(), arc.clone());
        Ok(arc)
    }

    pub fn invalidate(&self, name: &str) {
        self.sessions.lock().unwrap().remove(name);
    }
}

/// SourceProvider that routes to the session named in [`CURRENT_PROJECT`].
pub struct MultiProjectProvider {
    hub: Arc<ProjectsHub>,
}

impl MultiProjectProvider {
    pub fn new(hub: ProjectsHub) -> Self {
        Self {
            hub: Arc::new(hub),
        }
    }

    pub fn hub(&self) -> &Arc<ProjectsHub> {
        &self.hub
    }

    fn session(&self) -> Result<Arc<FilesystemProvider>, String> {
        let name = CURRENT_PROJECT
            .try_with(|n| n.clone())
            .map_err(|_| {
                "project scope missing — use /api/p/{project}/… routes".to_string()
            })?;
        self.hub.open(&name)
    }
}

#[async_trait]
impl SourceProvider for MultiProjectProvider {
    async fn list_files(&self) -> Vec<FileInfo> {
        match self.session() {
            Ok(p) => p.list_files().await,
            Err(_) => Vec::new(),
        }
    }

    async fn read_source(&self, file: &str) -> Result<String, String> {
        self.session()?.read_source(file).await
    }

    async fn write_source(&self, file: &str, content: &str) -> Result<(), String> {
        self.session()?.write_source(file, content).await
    }

    fn registry(&self) -> LayerRegistry {
        self.session()
            .map(|p| p.registry())
            .unwrap_or_else(|_| LayerRegistry::builtin())
    }

    fn is_editable(&self, file: &str) -> bool {
        self.session()
            .map(|p| p.is_editable(file))
            .unwrap_or(false)
    }

    fn file_kind(&self, file: &str) -> FileKind {
        self.session()
            .map(|p| p.file_kind(file))
            .unwrap_or(FileKind::Package)
    }

    fn set_active(&self, index: usize) -> Result<(), String> {
        self.session()?.set_active(index)
    }

    async fn baseline_source(&self, file: &str) -> Result<Option<(String, String)>, String> {
        self.session()?.baseline_source(file).await
    }

    async fn reload_from_disk(&self) -> Result<usize, String> {
        self.session()?.reload_from_disk().await
    }

    async fn layer_dependents(&self, layer_name: &str) -> Vec<FileInfo> {
        match self.session() {
            Ok(p) => p.layer_dependents(layer_name).await,
            Err(_) => Vec::new(),
        }
    }

    fn register_file(
        &self,
        path: PathBuf,
        source: String,
        editable: bool,
    ) -> Result<usize, String> {
        self.session()?.register_file(path, source, editable)
    }

    fn project_root(&self) -> Option<PathBuf> {
        self.session().ok().and_then(|p| p.project_root())
    }
}

