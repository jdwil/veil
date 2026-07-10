//! Remote HTTP SourceProvider (AGT-010) — talk to another `veil serve`.
//!
//! ```text
//! VEIL_REMOTE_URL=http://host:3001  # no trailing slash
//! ```
//!
//! Local IDE can run against remote package source without LocalFs on the client
//! machine (reads/writes go over HTTP). Layers still load from the client
//! registry (or remote package's `use` paths must be available).

use async_trait::async_trait;
use veil_ir::LayerRegistry;

use super::{FileInfo, SourceProvider};

/// Proxies list/read/write/select to a remote VEIL serve instance.
pub struct RemoteHttpProvider {
    base: String,
    registry: LayerRegistry,
}

impl RemoteHttpProvider {
    pub fn from_env(registry: LayerRegistry) -> Result<Self, String> {
        let base = std::env::var("VEIL_REMOTE_URL")
            .map_err(|_| "VEIL_REMOTE_URL not set".to_string())?
            .trim_end_matches('/')
            .to_string();
        Ok(Self { base, registry })
    }

    pub fn new(base: impl Into<String>, registry: LayerRegistry) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            registry,
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api{path}", self.base)
    }

    fn curl_get(&self, path: &str) -> Result<String, String> {
        let url = self.api(path);
        let out = std::process::Command::new("curl")
            .args(["-sS", "-f", &url])
            .output()
            .map_err(|e| format!("curl GET {url}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "GET {url} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn curl_post(&self, path: &str, content_type: &str, body: &str) -> Result<String, String> {
        let url = self.api(path);
        let out = std::process::Command::new("curl")
            .args([
                "-sS",
                "-f",
                "-X",
                "POST",
                "-H",
                &format!("Content-Type: {content_type}"),
                "-d",
                body,
                &url,
            ])
            .output()
            .map_err(|e| format!("curl POST {url}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "POST {url} failed: {} {}",
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[async_trait]
impl SourceProvider for RemoteHttpProvider {
    async fn list_files(&self) -> Vec<FileInfo> {
        match self.curl_get("/files") {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => vec![],
        }
    }

    async fn read_source(&self, _file: &str) -> Result<String, String> {
        self.curl_get("/source")
    }

    async fn write_source(&self, _file: &str, content: &str) -> Result<(), String> {
        self.curl_post("/source", "text/plain; charset=utf-8", content)?;
        Ok(())
    }

    fn registry(&self) -> &LayerRegistry {
        &self.registry
    }

    fn is_editable(&self, _file: &str) -> bool {
        true
    }

    fn set_active(&self, index: usize) -> Result<(), String> {
        let body = serde_json::json!({ "index": index }).to_string();
        self.curl_post("/files/select", "application/json", &body)?;
        Ok(())
    }
}
