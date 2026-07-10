//! Source provider abstraction — the storage backend for the dev server.

pub mod filesystem;
pub mod remote;

use async_trait::async_trait;
use veil_ir::LayerRegistry;

/// Metadata about a loaded file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileInfo {
    pub index: usize,
    pub name: String,
    pub path: String,
    pub editable: bool,
    pub active: bool,
}

/// Abstraction over where .veil source lives.
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
}
