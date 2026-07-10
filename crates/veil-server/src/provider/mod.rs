//! Source provider abstraction — the storage backend for the dev server.

pub mod filesystem;

use async_trait::async_trait;
use veil_ir::LayerRegistry;

/// Metadata about a loaded file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub editable: bool,
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

    /// Get the layer registry for parsing.
    fn registry(&self) -> &LayerRegistry;

    /// Is the given file editable?
    fn is_editable(&self, file: &str) -> bool;
}
