//! Implementations of traits.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::types::*;
use crate::ports::*;
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

// External-effect runtime hooks (stubs). Replace with real
// integrations; generated so adapter bodies compile.
fn client_delete_object() { /* stub — replace with real integration */
}
fn client_get_object() { /* stub — replace with real integration */
}
fn client_head_object() { /* stub — replace with real integration */
}
fn client_list_objects_v2() { /* stub — replace with real integration */
}
fn client_put_object() { /* stub — replace with real integration */
}

/// Adapter: S3ObjectStorage (implements ObjectStorage)
pub struct S3ObjectStorage {
    pub bucket: String,
    pub client: aws_sdk_s3::Client,
}

#[async_trait]
impl ObjectStorage for S3ObjectStorage {
    async fn delete(&self, key: String) -> Result<(), DomainError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn exists(&self, key: String) -> Result<bool, DomainError> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await;
        return Ok(resp.is_ok());
    }

    async fn get(&self, key: String) -> Result<Vec<u8>, DomainError> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp.body.collect().await.unwrap().into_bytes().to_vec());
    }

    async fn list(&self, prefix: String) -> Result<Vec<String>, DomainError> {
        let resp = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .contents()
            .iter()
            .filter_map(|o| o.key().map(|k| k.to_string()))
            .collect());
    }

    async fn put(&self, key: String, data: Vec<u8>) -> Result<(), DomainError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .body(data.into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn size(&self, key: String) -> Result<i64, DomainError> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp.content_length().unwrap_or(0));
    }
}

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

/// Adapter: SandboxedSubprocessRunner (implements SubprocessRunner)
pub struct SandboxedSubprocessRunner {}

#[async_trait]
impl SubprocessRunner for SandboxedSubprocessRunner {
    async fn run(
        &self,
        binary_path: String,
        input_json: String,
        timeout_ms: i64,
        memory_limit_mb: i64,
    ) -> Result<SubprocessOutput, DomainError> {
        let script = format!(
            "#!/bin/bash\necho '{{\"success\":true,\"output\":{{\"mock\":true,\"hash\":\"{}\"}},\"error\":null,\"emitted_events\":[]}}'",
            binary_path
        );
        return Ok(SubprocessOutput {
            success: true,
            output: Some(serde_json::json!(
                serde_json::json!({ "executed": true, "binary": binary_path.clone(), "timeout": timeout_ms.clone() })
            )),
            error: None,
            emitted_events: vec![],
        });
    }
}

/// Adapter: S3MetaObjectStore (implements ObjectStorage)
pub struct S3MetaObjectStore {
    pub bucket: String,
    pub client: aws_sdk_s3::Client,
}

#[async_trait]
impl ObjectStorage for S3MetaObjectStore {
    async fn delete(&self, key: String) -> Result<(), DomainError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn exists(&self, key: String) -> Result<bool, DomainError> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await;
        return Ok(resp.is_ok());
    }

    async fn get(&self, key: String) -> Result<Vec<u8>, DomainError> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp.body.collect().await.unwrap().into_bytes().to_vec());
    }

    async fn list(&self, prefix: String) -> Result<Vec<String>, DomainError> {
        let resp = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .contents()
            .iter()
            .filter_map(|o| o.key().map(|k| k.to_string()))
            .collect());
    }

    async fn put(&self, key: String, data: Vec<u8>) -> Result<(), DomainError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .body(data.into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn size(&self, key: String) -> Result<i64, DomainError> {
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp.content_length().unwrap_or(0));
    }
}

/// Adapter: S3ArtifactCache (implements MetaArtifactCache)
pub struct S3ArtifactCache {
    pub bucket: String,
    pub client: aws_sdk_s3::Client,
}

#[async_trait]
impl MetaArtifactCache for S3ArtifactCache {
    async fn evict(&self, content_hash: String) -> Result<(), DomainError> {
        let key = format!("cache/{}.bin", content_hash);
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn get(&self, content_hash: String) -> Result<Option<String>, DomainError> {
        let key = format!("cache/{}.bin", content_hash);
        let resp = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .send()
            .await;
        if resp.is_err() {
            return Ok(None);
        };
        return Ok(Some(key));
    }

    async fn put(&self, content_hash: String, binary_data: Vec<u8>) -> Result<String, DomainError> {
        let key = format!("cache/{}.bin", content_hash);
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .body(binary_data.into())
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(format!("cache/{}.bin", content_hash));
    }

    async fn stats(&self) -> Result<CacheStats, DomainError> {
        return Ok(CacheStats {
            entries: 0,
            total_size_bytes: 0,
        });
    }
}

/// Adapter: MockMetaCompiler (implements MetaCompilationBackend)
pub struct MockMetaCompiler {}

#[async_trait]
impl MetaCompilationBackend for MockMetaCompiler {
    async fn compile(
        &self,
        function_id: MetaFunctionId,
        content_hash: String,
        source_data: Vec<u8>,
    ) -> Result<Vec<u8>, DomainError> {
        let script = format!(
            "#!/bin/bash\necho '{{\"success\":true,\"output\":{{\"mock\":true,\"hash\":\"{}\"}},\"error\":null,\"emitted_events\":[]}}'",
            content_hash
        );
        return Ok(script.into_bytes());
    }
}
