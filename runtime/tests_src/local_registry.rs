//! EXT-02: local File/Local-style registry roundtrip (no AWS).
//! Run: cargo test -p extensions --test local_registry

use std::sync::Arc;

use extensions::application::{self, Deps};
use extensions::domain::types::{ExtensionKind, ExtensionProvenance, ExtensionScope};
use extensions::domain::types::{ExtensionInvokeRequest, ExtensionInvokeResult, ExtensionRunStatus, ExtensionVersion};
use extensions::ports::{ExtensionExecutor, ExtensionRegistry, ExtensionSourceStore};
use async_trait::async_trait;
use uuid::Uuid;
use veil_shared::DomainError;

/// In-test filesystem registry (mirrors bootstrap LocalExtensionRegistry).
struct FsRegistry {
    root: std::path::PathBuf,
}

impl FsRegistry {
    fn new(root: std::path::PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }
    fn record_path(&self, id: &Uuid) -> std::path::PathBuf {
        self.root.join(format!("{id}.json"))
    }
    fn version_path(&self, id: &Uuid, version: i64) -> std::path::PathBuf {
        self.root
            .join(id.to_string())
            .join("versions")
            .join(format!("{version}.json"))
    }
}

#[async_trait]
impl ExtensionRegistry for FsRegistry {
    async fn create(
        &self,
        record: extensions::domain::types::ExtensionRecord,
    ) -> Result<extensions::domain::types::ExtensionRecord, DomainError> {
        let p = self.record_path(&record.extension_id);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let s = serde_json::to_string_pretty(&record).map_err(|e| DomainError::External(e.to_string()))?;
        std::fs::write(&p, s).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(record)
    }
    async fn get(
        &self,
        id: Uuid,
    ) -> Result<Option<extensions::domain::types::ExtensionRecord>, DomainError> {
        let p = self.record_path(&id);
        if !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(Some(
            serde_json::from_str(&s).map_err(|e| DomainError::External(e.to_string()))?,
        ))
    }
    async fn list(
        &self,
        _scope: Option<String>,
        _kind: Option<String>,
        _product_id: Option<String>,
        _tenant_id: Option<Uuid>,
    ) -> Result<Vec<extensions::domain::types::ExtensionRecord>, DomainError> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(rec) = serde_json::from_str(&s) {
                    out.push(rec);
                }
            }
        }
        Ok(out)
    }
    async fn update(
        &self,
        record: extensions::domain::types::ExtensionRecord,
    ) -> Result<extensions::domain::types::ExtensionRecord, DomainError> {
        self.create(record).await
    }
    async fn save_version(
        &self,
        ver: extensions::domain::types::ExtensionVersion,
    ) -> Result<extensions::domain::types::ExtensionVersion, DomainError> {
        let p = self.version_path(&ver.extension_id, ver.version);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let s = serde_json::to_string_pretty(&ver).map_err(|e| DomainError::External(e.to_string()))?;
        std::fs::write(&p, s).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(ver)
    }
    async fn get_version(
        &self,
        id: Uuid,
        version: i64,
    ) -> Result<Option<extensions::domain::types::ExtensionVersion>, DomainError> {
        let p = self.version_path(&id, version);
        if !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(Some(
            serde_json::from_str(&s).map_err(|e| DomainError::External(e.to_string()))?,
        ))
    }
    async fn list_versions(
        &self,
        id: Uuid,
    ) -> Result<Vec<extensions::domain::types::ExtensionVersion>, DomainError> {
        let dir = self.root.join(id.to_string()).join("versions");
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            if let Ok(s) = std::fs::read_to_string(e.path()) {
                if let Ok(ver) = serde_json::from_str(&s) {
                    out.push(ver);
                }
            }
        }
        out.sort_by_key(|v: &extensions::domain::types::ExtensionVersion| v.version);
        Ok(out)
    }
}

struct FsSources {
    root: std::path::PathBuf,
}

#[async_trait]
impl ExtensionSourceStore for FsSources {
    async fn ensure_package(&self, id: Uuid) -> Result<String, DomainError> {
        let r = self.root.join("src").join(id.to_string());
        std::fs::create_dir_all(&r).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(r.to_string_lossy().to_string())
    }
    async fn write_file(&self, id: Uuid, rel_path: String, content: String) -> Result<(), DomainError> {
        let p = self.root.join("src").join(id.to_string()).join(rel_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DomainError::External(e.to_string()))?;
        }
        std::fs::write(p, content).map_err(|e| DomainError::External(e.to_string()))
    }
    async fn read_file(&self, id: Uuid, rel_path: String) -> Result<Option<String>, DomainError> {
        let p = self.root.join("src").join(id.to_string()).join(rel_path);
        if !p.is_file() {
            return Ok(None);
        }
        Ok(Some(
            std::fs::read_to_string(p).map_err(|e| DomainError::External(e.to_string()))?,
        ))
    }
    async fn list_files(&self, id: Uuid, _prefix: String) -> Result<Vec<String>, DomainError> {
        let r = self.root.join("src").join(id.to_string());
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(r) {
            for e in rd.flatten() {
                out.push(e.file_name().to_string_lossy().to_string());
            }
        }
        Ok(out)
    }
    async fn package_root(&self, id: Uuid) -> Result<String, DomainError> {
        Ok(self
            .root
            .join("src")
            .join(id.to_string())
            .to_string_lossy()
            .to_string())
    }
}

struct StubExecutor;

#[async_trait]
impl ExtensionExecutor for StubExecutor {
    async fn publish(&self, id: Uuid) -> Result<ExtensionVersion, DomainError> {
        Ok(ExtensionVersion {
            extension_id: id,
            version: 1,
            source_commit: "test".into(),
            artifact_uris: serde_json::json!({}),
            published_on: chrono::Utc::now(),
            published_by: None,
            changelog: None,
        })
    }
    async fn invoke(&self, _req: ExtensionInvokeRequest) -> Result<ExtensionInvokeResult, DomainError> {
        Ok(ExtensionInvokeResult {
            status: ExtensionRunStatus::Succeeded,
            message: None,
            outputs: serde_json::json!({}),
        })
    }
}

#[tokio::test]
async fn create_list_get_version_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let deps = Deps {
        extension_executor: Arc::new(StubExecutor),
        extension_registry: Arc::new(FsRegistry::new(tmp.path().to_path_buf())),
        extension_source_store: Arc::new(FsSources {
            root: tmp.path().to_path_buf(),
        }),
    };

    let rec = application::create_extension(
        &deps,
        "Activate members".into(),
        "Reaction".into(),
        "Platform".into(),
        "Stock".into(),
        None,
        None,
        None,
        Some("seed".into()),
        None,
    )
    .await
    .expect("create");
    assert_eq!(rec.name, "Activate members");
    assert!(matches!(rec.kind, ExtensionKind::Reaction));
    assert!(matches!(rec.scope, ExtensionScope::Platform));
    assert!(matches!(rec.provenance, ExtensionProvenance::Stock));

    let listed = application::list_extensions(&deps, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);

    let got = application::get_extension(&deps, rec.extension_id)
        .await
        .unwrap();
    assert_eq!(got.extension_id, rec.extension_id);

    let ver = application::save_extension_version(
        &deps,
        rec.extension_id,
        "c0".into(),
        serde_json::json!({ "rust": "local" }),
        None,
    )
    .await
    .unwrap();
    assert_eq!(ver.version, 1);

    let versions = application::list_extension_versions(&deps, rec.extension_id)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
}
