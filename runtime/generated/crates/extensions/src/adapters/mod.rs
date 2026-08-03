//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

/// Adapter: BusAuthAdapter (implements AuthService)
pub struct BusAuthAdapter {}

#[async_trait]
impl AuthService for BusAuthAdapter {
    async fn check_permission(
        &self,
        principal: Principal,
        permission: String,
    ) -> Result<bool, DomainError> {
        return Ok(true);
    }

    async fn validate_token(&self, token: String) -> Result<Principal, DomainError> {
        return Ok(Principal {
            id: token.clone(),
            roles: vec![],
            claims: HashMap::new(),
        });
    }
}

/// Adapter: FileExtensionRegistry (implements ExtensionRegistry)
pub struct FileExtensionRegistry {
    pub dir: String,
}

#[async_trait]
impl ExtensionRegistry for FileExtensionRegistry {
    async fn create(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError> {
        veil_ext_store::ExtStore::put_record(
            self.dir.clone().clone(),
            format!("{}", record.extension_id),
            serde_json::to_string(&record)?,
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(record);
    }

    async fn get(&self, id: Uuid) -> Result<Option<ExtensionRecord>, DomainError> {
        let raw = veil_ext_store::ExtStore::get_record(self.dir.clone().clone(), format!("{}", id))
            .map_err(|e| DomainError::External(e.to_string()))?;
        if raw == "".to_string() {
            return Ok(None);
        };
        let rec = serde_json::from_str::<_>(&raw)?;
        return Ok(Some(rec));
    }

    async fn get_version(
        &self,
        id: Uuid,
        version: i64,
    ) -> Result<Option<ExtensionVersion>, DomainError> {
        let raw = veil_ext_store::ExtStore::get_version(
            self.dir.clone().clone(),
            format!("{}", id),
            format!("{}", version),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        if raw == "".to_string() {
            return Ok(None);
        };
        let ver = serde_json::from_str::<_>(&raw)?;
        return Ok(Some(ver));
    }

    async fn list(
        &self,
        scope: Option<String>,
        kind: Option<String>,
        product_id: Option<String>,
        tenant_id: Option<Uuid>,
    ) -> Result<Vec<ExtensionRecord>, DomainError> {
        let raws = veil_ext_store::ExtStore::list_records(self.dir.clone().clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let mut out = vec![];
        for raw in raws {
            let rec = serde_json::from_str::<_>(&raw)?;
            out.push(rec);
        }
        return Ok(out);
    }

    async fn list_versions(&self, id: Uuid) -> Result<Vec<ExtensionVersion>, DomainError> {
        let raws =
            veil_ext_store::ExtStore::list_versions(self.dir.clone().clone(), format!("{}", id))
                .map_err(|e| DomainError::External(e.to_string()))?;
        let mut out = vec![];
        for raw in raws {
            let ver = serde_json::from_str::<_>(&raw)?;
            out.push(ver);
        }
        return Ok(out);
    }

    async fn save_version(&self, ver: ExtensionVersion) -> Result<ExtensionVersion, DomainError> {
        veil_ext_store::ExtStore::put_version(
            self.dir.clone().clone(),
            format!("{}", ver.extension_id),
            format!("{}", ver.version),
            serde_json::to_string(&ver)?,
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(ver);
    }

    async fn update(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError> {
        veil_ext_store::ExtStore::put_record(
            self.dir.clone().clone(),
            format!("{}", record.extension_id),
            serde_json::to_string(&record)?,
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(record);
    }
}

/// Adapter: FileExtensionSourceStore (implements ExtensionSourceStore)
pub struct FileExtensionSourceStore {
    pub dir: String,
}

#[async_trait]
impl ExtensionSourceStore for FileExtensionSourceStore {
    async fn ensure_package(&self, id: Uuid) -> Result<String, DomainError> {
        let root =
            veil_ext_store::ExtStore::ensure_package(self.dir.clone().clone(), format!("{}", id))
                .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(root);
    }

    async fn list_files(&self, id: Uuid, prefix: String) -> Result<Vec<String>, DomainError> {
        let names =
            veil_ext_store::ExtStore::list_source(self.dir.clone().clone(), format!("{}", id))
                .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(names);
    }

    async fn package_root(&self, id: Uuid) -> Result<String, DomainError> {
        return Ok(veil_ext_store::ExtStore::package_root(
            self.dir.clone().clone(),
            format!("{}", id),
        ));
    }

    async fn read_file(&self, id: Uuid, rel_path: String) -> Result<Option<String>, DomainError> {
        let raw = veil_ext_store::ExtStore::read_source(
            self.dir.clone().clone(),
            format!("{}", id),
            rel_path.clone(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        if raw == "".to_string() {
            return Ok(None);
        };
        return Ok(Some(raw));
    }

    async fn write_file(
        &self,
        id: Uuid,
        rel_path: String,
        content: String,
    ) -> Result<(), DomainError> {
        veil_ext_store::ExtStore::write_source(
            self.dir.clone().clone(),
            format!("{}", id),
            rel_path.clone(),
            content.clone(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(());
    }
}

/// Adapter: FileExtensionArtifactStore (implements ExtensionArtifactStore)
pub struct FileExtensionArtifactStore {
    pub dir: String,
}

#[async_trait]
impl ExtensionArtifactStore for FileExtensionArtifactStore {
    async fn get_uri(
        &self,
        id: Uuid,
        version: i64,
        target: String,
    ) -> Result<Option<String>, DomainError> {
        let uri = veil_ext_store::ExtStore::get_artifact_uri(
            self.dir.clone().clone(),
            format!("{}", id),
            format!("{}", version),
            target.clone(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        if uri == "".to_string() {
            return Ok(None);
        };
        return Ok(Some(uri));
    }

    async fn put(
        &self,
        id: Uuid,
        version: i64,
        target: String,
        data: Vec<u8>,
    ) -> Result<String, DomainError> {
        let uri = veil_ext_store::ExtStore::put_artifact(
            self.dir.clone().clone(),
            format!("{}", id),
            format!("{}", version),
            target.clone(),
            "artifact".to_string(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(uri);
    }
}

/// Adapter: FileExtensionExecutor (implements ExtensionExecutor)
pub struct FileExtensionExecutor {
    pub dir: String,
}

#[async_trait]
impl ExtensionExecutor for FileExtensionExecutor {
    async fn invoke(
        &self,
        req: ExtensionInvokeRequest,
    ) -> Result<ExtensionInvokeResult, DomainError> {
        let res = ExtensionInvokeResult {
            status: ExtensionRunStatus::Succeeded.clone(),
            message: None,
            outputs: serde_json::json!(serde_json::json!({})),
        };
        return Ok(res);
    }

    async fn publish(&self, id: Uuid) -> Result<ExtensionVersion, DomainError> {
        let ver = ExtensionVersion {
            extension_id: id.clone(),
            version: 1,
            source_commit: "local".to_string(),
            artifact_uris: serde_json::json!(serde_json::json!({})),
            published_on: Utc::now(),
            published_by: None,
            changelog: None,
        };
        return Ok(ver);
    }
}

/// Adapter: DdbExtensionRegistry (implements ExtensionRegistry)
pub struct DdbExtensionRegistry {
    pub dir: String,
}

#[async_trait]
impl ExtensionRegistry for DdbExtensionRegistry {
    async fn create(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError> {
        veil_ext_store::ExtStore::put_record(
            self.dir.clone().clone(),
            format!("{}", record.extension_id),
            serde_json::to_string(&record)?,
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(record);
    }

    async fn get(&self, id: Uuid) -> Result<Option<ExtensionRecord>, DomainError> {
        let raw = veil_ext_store::ExtStore::get_record(self.dir.clone().clone(), format!("{}", id))
            .map_err(|e| DomainError::External(e.to_string()))?;
        if raw == "".to_string() {
            return Ok(None);
        };
        let rec = serde_json::from_str::<_>(&raw)?;
        return Ok(Some(rec));
    }

    async fn get_version(
        &self,
        id: Uuid,
        version: i64,
    ) -> Result<Option<ExtensionVersion>, DomainError> {
        let raw = veil_ext_store::ExtStore::get_version(
            self.dir.clone().clone(),
            format!("{}", id),
            format!("{}", version),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        if raw == "".to_string() {
            return Ok(None);
        };
        let ver = serde_json::from_str::<_>(&raw)?;
        return Ok(Some(ver));
    }

    async fn list(
        &self,
        scope: Option<String>,
        kind: Option<String>,
        product_id: Option<String>,
        tenant_id: Option<Uuid>,
    ) -> Result<Vec<ExtensionRecord>, DomainError> {
        let raws = veil_ext_store::ExtStore::list_records(self.dir.clone().clone())
            .map_err(|e| DomainError::External(e.to_string()))?;
        let mut out = vec![];
        for raw in raws {
            let rec = serde_json::from_str::<_>(&raw)?;
            out.push(rec);
        }
        return Ok(out);
    }

    async fn list_versions(&self, id: Uuid) -> Result<Vec<ExtensionVersion>, DomainError> {
        let raws =
            veil_ext_store::ExtStore::list_versions(self.dir.clone().clone(), format!("{}", id))
                .map_err(|e| DomainError::External(e.to_string()))?;
        let mut out = vec![];
        for raw in raws {
            let ver = serde_json::from_str::<_>(&raw)?;
            out.push(ver);
        }
        return Ok(out);
    }

    async fn save_version(&self, ver: ExtensionVersion) -> Result<ExtensionVersion, DomainError> {
        veil_ext_store::ExtStore::put_version(
            self.dir.clone().clone(),
            format!("{}", ver.extension_id),
            format!("{}", ver.version),
            serde_json::to_string(&ver)?,
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(ver);
    }

    async fn update(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError> {
        veil_ext_store::ExtStore::put_record(
            self.dir.clone().clone(),
            format!("{}", record.extension_id),
            serde_json::to_string(&record)?,
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(record);
    }
}

/// Adapter: S3ExtensionSourceStore (implements ExtensionSourceStore)
pub struct S3ExtensionSourceStore {
    pub dir: String,
}

#[async_trait]
impl ExtensionSourceStore for S3ExtensionSourceStore {
    async fn ensure_package(&self, id: Uuid) -> Result<String, DomainError> {
        let root =
            veil_ext_store::ExtStore::ensure_package(self.dir.clone().clone(), format!("{}", id))
                .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(root);
    }

    async fn list_files(&self, id: Uuid, prefix: String) -> Result<Vec<String>, DomainError> {
        let names =
            veil_ext_store::ExtStore::list_source(self.dir.clone().clone(), format!("{}", id))
                .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(names);
    }

    async fn package_root(&self, id: Uuid) -> Result<String, DomainError> {
        return Ok(veil_ext_store::ExtStore::package_root(
            self.dir.clone().clone(),
            format!("{}", id),
        ));
    }

    async fn read_file(&self, id: Uuid, rel_path: String) -> Result<Option<String>, DomainError> {
        let raw = veil_ext_store::ExtStore::read_source(
            self.dir.clone().clone(),
            format!("{}", id),
            rel_path.clone(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        if raw == "".to_string() {
            return Ok(None);
        };
        return Ok(Some(raw));
    }

    async fn write_file(
        &self,
        id: Uuid,
        rel_path: String,
        content: String,
    ) -> Result<(), DomainError> {
        veil_ext_store::ExtStore::write_source(
            self.dir.clone().clone(),
            format!("{}", id),
            rel_path.clone(),
            content.clone(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(());
    }
}

/// Adapter: S3ExtensionArtifactStore (implements ExtensionArtifactStore)
pub struct S3ExtensionArtifactStore {
    pub dir: String,
}

#[async_trait]
impl ExtensionArtifactStore for S3ExtensionArtifactStore {
    async fn get_uri(
        &self,
        id: Uuid,
        version: i64,
        target: String,
    ) -> Result<Option<String>, DomainError> {
        let uri = veil_ext_store::ExtStore::get_artifact_uri(
            self.dir.clone().clone(),
            format!("{}", id),
            format!("{}", version),
            target.clone(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        if uri == "".to_string() {
            return Ok(None);
        };
        return Ok(Some(uri));
    }

    async fn put(
        &self,
        id: Uuid,
        version: i64,
        target: String,
        data: Vec<u8>,
    ) -> Result<String, DomainError> {
        let uri = veil_ext_store::ExtStore::put_artifact(
            self.dir.clone().clone(),
            format!("{}", id),
            format!("{}", version),
            target.clone(),
            "artifact".to_string(),
        )
        .map_err(|e| DomainError::External(e.to_string()))?;
        return Ok(uri);
    }
}
