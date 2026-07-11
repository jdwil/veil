//! Source provider abstraction — the storage backend for the dev server.

pub mod filesystem;
pub mod hub;
pub mod remote;

use async_trait::async_trait;
use veil_ir::LayerRegistry;

/// Kind of project file in the serve set (DSL-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    #[default]
    Package,
    Layer,
    Stub,
}

impl FileKind {
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "layer" => FileKind::Layer,
            "stub" => FileKind::Stub,
            _ => FileKind::Package,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Package => "package",
            FileKind::Layer => "layer",
            FileKind::Stub => "stub",
        }
    }
}

/// Metadata about a loaded file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub index: usize,
    pub name: String,
    pub path: String,
    pub editable: bool,
    pub active: bool,
    /// package | layer | stub
    #[serde(default)]
    pub kind: FileKind,
}

/// Abstraction over where .veil / .layer source lives.
///
/// - [`filesystem::FilesystemProvider`] — reads from local disk (veil-cli)
/// - A `VcsProvider` in veil-runtime would read from S3/git via the Bus
#[async_trait]
pub trait SourceProvider: Send + Sync + 'static {
    /// List available files.
    async fn list_files(&self) -> Vec<FileInfo>;

    /// Read source content of a file by name/path.
    async fn read_source(&self, file: &str) -> Result<String, String>;

    /// Write source content back (edit commit).
    async fn write_source(&self, file: &str, content: &str) -> Result<(), String>;

    /// Layer registry for the **active** source (per-file when multi-file).
    ///
    /// Callers receive an owned clone so multi-file providers can switch
    /// registries when the active file changes.
    fn registry(&self) -> LayerRegistry;

    /// Is the given file editable?
    fn is_editable(&self, file: &str) -> bool;

    /// Kind of the active file (or named file when non-empty).
    fn file_kind(&self, file: &str) -> FileKind {
        let _ = file;
        FileKind::Package
    }

    /// Switch the active file by index (UX-011). Default: unsupported.
    fn set_active(&self, _index: usize) -> Result<(), String> {
        Err("set_active not supported by this provider".into())
    }

    /// Baseline source for structural diff (UX-021).
    ///
    /// Default: `None` (caller may use session snapshot only).
    /// Filesystem provider returns `git show HEAD:<path>` when available.
    async fn baseline_source(&self, _file: &str) -> Result<Option<(String, String)>, String> {
        Ok(None)
    }

    /// AGT-017: optional remote-forward for structured edits.
    ///
    /// When `Some`, the API handler uses the remote result instead of local
    /// apply+write. Default: `None` (handle locally).
    async fn forward_edit(&self, _edit_json: &str) -> Option<Result<String, String>> {
        None
    }

    /// AGT-018: URL for SSE events when this provider is a remote proxy.
    fn remote_events_url(&self) -> Option<String> {
        None
    }

    /// Re-read active (or all) files from disk into the in-memory cache.
    ///
    /// Used after an external ACP agent (e.g. Kiro) mutates workspace files.
    /// Default: no-op.
    async fn reload_from_disk(&self) -> Result<usize, String> {
        Ok(0)
    }

    /// Packages in the serve set that `use` the given layer name (DSL-014).
    async fn layer_dependents(&self, _layer_name: &str) -> Vec<FileInfo> {
        Vec::new()
    }

    /// Append a newly scaffolded file into the live serve set (DSL-013).
    fn register_file(
        &self,
        _path: std::path::PathBuf,
        _source: String,
        _editable: bool,
    ) -> Result<usize, String> {
        Err("register_file not supported".into())
    }

    /// Active IDE project root (single-project session). Default: unknown.
    fn project_root(&self) -> Option<std::path::PathBuf> {
        None
    }
}
