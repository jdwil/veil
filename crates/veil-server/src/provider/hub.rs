//! Multi-project hub: lazy per-product sessions (MP-002).
//!
//! Backends:
//! - **disk** — [`FilesystemProvider`] under `VEIL_PROJECTS_DIR`
//! - **s3** / **prefer_s3** — materialize `s3://$BUCKET/repos/{id}/{branch}/` + write-through
//!   ([`super::s3_workspace`]); source of truth is DDB META + S3 (production-like / ECS).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::filesystem::FilesystemProvider;
use super::s3_workspace::{
    ide_source_mode, list_s3_projects, open_s3_project, IdeSourceMode,
};
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

/// HTTP class for hub open failures (RTU-006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenErrorKind {
    BadRequest,
    NotFound,
    Unprocessable,
    Internal,
}

/// Session type: disk or S3 write-through (both implement [`SourceProvider`]).
type Session = Arc<dyn SourceProvider>;

/// Lazy sessions keyed by product directory name / slug.
pub struct ProjectsHub {
    projects_dir: PathBuf,
    show_core_layers: bool,
    sessions: Mutex<HashMap<String, Session>>,
}

impl ProjectsHub {
    pub fn new(projects_dir: PathBuf, show_core_layers: bool) -> Self {
        let mode = ide_source_mode();
        tracing::info!(
            ?mode,
            projects_dir = %projects_dir.display(),
            "ProjectsHub source mode"
        );
        Self {
            projects_dir,
            show_core_layers,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    pub fn source_mode(&self) -> IdeSourceMode {
        ide_source_mode()
    }

    pub fn list(&self) -> Result<Vec<ProjectInfo>, String> {
        match ide_source_mode() {
            IdeSourceMode::S3 => list_s3_projects(),
            IdeSourceMode::PreferS3 => {
                let mut by_name: HashMap<String, ProjectInfo> = HashMap::new();
                if let Ok(disk) = list_projects(&self.projects_dir) {
                    for p in disk {
                        by_name.insert(p.name.clone(), p);
                    }
                }
                // S3 wins on name collision (remote is preferred).
                match list_s3_projects() {
                    Ok(remote) => {
                        for p in remote {
                            by_name.insert(p.name.clone(), p);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(%e, "prefer_s3 list: DDB/S3 list failed; disk only");
                    }
                }
                let mut out: Vec<_> = by_name.into_values().collect();
                out.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(out)
            }
            IdeSourceMode::Disk => list_projects(&self.projects_dir),
        }
    }

    pub fn create(&self, name: &str) -> Result<ProjectInfo, String> {
        if matches!(ide_source_mode(), IdeSourceMode::S3) {
            return Err(
                "create project under VEIL_SOURCE_MODE=s3 is not supported from the hub — \
                 seed DDB META + S3 (scripts/seed-repo-s3.sh) or use the platform CreateRepo path"
                    .into(),
            );
        }
        let info = create_project(&self.projects_dir, name)?;
        self.sessions.lock().unwrap().remove(name);
        Ok(info)
    }

    /// Classify open errors for HTTP status (RTU-006).
    pub fn open_error_kind(err: &str) -> OpenErrorKind {
        let e = err.to_lowercase();
        if e.contains("invalid project name") || e.contains("empty") {
            OpenErrorKind::BadRequest
        } else if e.contains("not found")
            || e.contains("not a veil project")
            || e.contains("no s3/ddb repo")
            || e.contains("no s3")
        {
            OpenErrorKind::NotFound
        } else if e.contains("no packages")
            || e.contains("no .veil")
            || e.contains("empty after materialize")
            || e.contains("has no packages")
        {
            OpenErrorKind::Unprocessable
        } else {
            OpenErrorKind::Internal
        }
    }

    /// Open or return cached session for a product name/slug.
    pub fn open(&self, name: &str) -> Result<Session, String> {
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(format!("invalid project name: {name}"));
        }
        {
            let map = self.sessions.lock().unwrap();
            if let Some(p) = map.get(name) {
                return Ok(p.clone());
            }
        }

        let session = match ide_source_mode() {
            IdeSourceMode::S3 => {
                tracing::info!(%name, "opening project from S3 (strict)");
                let p = open_s3_project(name, self.show_core_layers)?;
                p as Session
            }
            IdeSourceMode::PreferS3 => match open_s3_project(name, self.show_core_layers) {
                Ok(p) => {
                    tracing::info!(%name, "opening project from S3 (prefer_s3)");
                    p as Session
                }
                Err(e) => {
                    tracing::warn!(%name, %e, "S3 open failed; falling back to disk hub");
                    self.open_disk(name)?
                }
            },
            IdeSourceMode::Disk => self.open_disk(name)?,
        };

        self.sessions
            .lock()
            .unwrap()
            .insert(name.to_string(), session.clone());
        Ok(session)
    }

    fn open_disk(&self, name: &str) -> Result<Session, String> {
        let root = self.projects_dir.join(name);
        if !root.is_dir() {
            return Err(format!("project not found: {name}"));
        }
        if !is_project_root(&root) {
            return Err(format!("project not found: {name} (not a VEIL project)"));
        }
        let _ = ensure_project_shape(&root);
        let paths = match collect_project_files(&root, self.show_core_layers) {
            Ok(p) => p,
            Err(e) => {
                return Err(format!(
                    "no packages in project {name}: {e} — run: veil init {name}"
                ));
            }
        };
        let entries: Vec<(PathBuf, String, bool)> = paths
            .into_iter()
            .map(|path| {
                let source = std::fs::read_to_string(&path).unwrap_or_default();
                let editable = is_source_editable(&path, &source);
                (path, source, editable)
            })
            .collect();
        if entries.is_empty() {
            return Err(format!(
                "no packages in project {name} — run: veil init {}",
                root.display()
            ));
        }
        let reg = LayerRegistry::for_veil_file(&entries[0].0)
            .unwrap_or_else(|_| LayerRegistry::builtin());
        let provider = FilesystemProvider::with_files_in_project(entries, reg, Some(root));
        Ok(Arc::new(provider) as Session)
    }

    pub fn invalidate(&self, name: &str) {
        self.sessions.lock().unwrap().remove(name);
    }

    /// Bind an already-open session provider (durable session workdir) into the hub cache.
    pub fn bind_session_provider(&self, name: &str, provider: Session) {
        self.sessions
            .lock()
            .unwrap()
            .insert(name.to_string(), provider);
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

    fn session(&self) -> Result<Session, String> {
        let name = CURRENT_PROJECT.try_with(|n| n.clone()).map_err(|_| {
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
        // Hub-level agent (/api/agent/chat) has no CURRENT_PROJECT. Platform UX
        // tools (navigate_to, list_changes, …) still work with empty source.
        match self.session() {
            Ok(p) => p.read_source(file).await,
            Err(e) if e.contains("project scope missing") => Ok(String::new()),
            Err(e) => Err(e),
        }
    }

    async fn write_source(&self, file: &str, content: &str) -> Result<(), String> {
        self.session()?.write_source(file, content).await?;
        // Bump durable session revision when a coding session is bound.
        if let Some(sid) = crate::session::current_session_id() {
            if let Ok(h) = crate::session::SessionManager::global().attach(&sid) {
                let path = if file.is_empty() {
                    self.session()?
                        .list_files()
                        .await
                        .into_iter()
                        .find(|f| f.active)
                        .map(|f| f.name)
                        .unwrap_or_else(|| "active".into())
                } else {
                    file.to_string()
                };
                h.bump_revision(&path, None);
            }
        }
        Ok(())
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
        // Drop cached session so the next open re-materializes from S3 (or re-reads disk).
        let name = CURRENT_PROJECT.try_with(|n| n.clone()).map_err(|_| {
            "project scope missing — use /api/p/{project}/… routes".to_string()
        })?;
        let prev_active_name = if let Ok(p) = self.session() {
            let files = p.list_files().await;
            files.into_iter().find(|f| f.active).map(|f| f.name)
        } else {
            None
        };
        self.hub.invalidate(&name);
        let p = self.hub.open(&name)?;
        if let Some(ref prev_name) = prev_active_name {
            let files = p.list_files().await;
            if let Some(f) = files.iter().find(|f| &f.name == prev_name) {
                let _ = p.set_active(f.index);
            }
        }
        let n = p.list_files().await.len();
        Ok(n)
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
