//! Remote HTTP SourceProvider (AGT-010 / AGT-017 / AGT-018) — talk to another `veil serve`.
//!
//! ```text
//! VEIL_REMOTE_URL=http://host:3001  # no trailing slash
//! ```
//!
//! Uses in-process HTTP (RT-026), not curl. Proxies source, files, structured
//! edits, and can stream remote SSE events URL for live sync.

use async_trait::async_trait;
use veil_ir::LayerRegistry;

use super::{FileInfo, SourceProvider};

/// Proxies list/read/write/select/edit to a remote VEIL serve instance.
pub struct RemoteHttpProvider {
    base: String,
    registry: LayerRegistry,
    client: reqwest::blocking::Client,
}

impl RemoteHttpProvider {
    pub fn from_env(registry: LayerRegistry) -> Result<Self, String> {
        let base = std::env::var("VEIL_REMOTE_URL")
            .map_err(|_| "VEIL_REMOTE_URL not set".to_string())?
            .trim_end_matches('/')
            .to_string();
        Self::new(base, registry)
    }

    pub fn new(base: impl Into<String>, registry: LayerRegistry) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_string(),
            registry,
            client,
        })
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api{path}", self.base)
    }

    /// Public base for SSE proxy docs (AGT-018).
    pub fn events_url(&self) -> String {
        self.api("/events")
    }

    fn get(&self, path: &str) -> Result<String, String> {
        let url = self.api(path);
        let resp = self
            .client
            .get(&url)
            .send()
            .map_err(|e| format!("GET {url}: {e}"))?;
        let status = resp.status();
        let text = resp.text().map_err(|e| format!("GET {url} body: {e}"))?;
        if !status.is_success() {
            return Err(format!("GET {url} → {status}: {text}"));
        }
        Ok(text)
    }

    fn post(&self, path: &str, content_type: &str, body: &str) -> Result<String, String> {
        let url = self.api(path);
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", content_type)
            .body(body.to_string())
            .send()
            .map_err(|e| format!("POST {url}: {e}"))?;
        let status = resp.status();
        let text = resp.text().map_err(|e| format!("POST {url} body: {e}"))?;
        if !status.is_success() {
            return Err(format!("POST {url} → {status}: {text}"));
        }
        Ok(text)
    }

    /// AGT-017: forward structured edit JSON body to remote `POST /api/edit`.
    pub fn post_edit(&self, edit_json: &str) -> Result<String, String> {
        self.post("/edit", "application/json", edit_json)
    }
}

#[async_trait]
impl SourceProvider for RemoteHttpProvider {
    async fn list_files(&self) -> Vec<FileInfo> {
        match self.get("/files") {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => vec![],
        }
    }

    async fn read_source(&self, _file: &str) -> Result<String, String> {
        self.get("/source")
    }

    async fn write_source(&self, _file: &str, content: &str) -> Result<(), String> {
        self.post("/source", "text/plain; charset=utf-8", content)?;
        Ok(())
    }

    fn registry(&self) -> LayerRegistry {
        self.registry.clone()
    }

    fn is_editable(&self, _file: &str) -> bool {
        true
    }

    fn set_active(&self, index: usize) -> Result<(), String> {
        let body = serde_json::json!({ "index": index }).to_string();
        self.post("/files/select", "application/json", &body)?;
        Ok(())
    }

    /// Prefer remote structured edit (AGT-017). Returns Some when handled.
    async fn forward_edit(&self, edit_json: &str) -> Option<Result<String, String>> {
        Some(self.post_edit(edit_json))
    }

    fn remote_events_url(&self) -> Option<String> {
        Some(self.events_url())
    }
}
