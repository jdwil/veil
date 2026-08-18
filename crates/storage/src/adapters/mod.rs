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
fn attribute_value_s(_arg0: impl std::fmt::Debug) { /* stub — replace with real integration */
}
fn client_delete_item() { /* stub — replace with real integration */
}
fn client_delete_object() { /* stub — replace with real integration */
}
fn client_get_object() { /* stub — replace with real integration */
}
fn client_head_object() { /* stub — replace with real integration */
}
fn client_list_objects_v2() { /* stub — replace with real integration */
}
fn client_put_item() { /* stub — replace with real integration */
}
fn client_put_object() { /* stub — replace with real integration */
}
fn client_query() { /* stub — replace with real integration */
}
fn client_scan() { /* stub — replace with real integration */
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

/// Adapter: DdbMetadataStore (implements MetadataStore)
pub struct DdbMetadataStore {
    pub client: aws_sdk_dynamodb::Client,
    pub table: String,
}

impl DdbMetadataStore {
    /// Full-table scan with `LastEvaluatedKey` follow-up.
    ///
    /// FilterExpression is applied *after* each 1MB page. A single `.send()`
    /// therefore drops catalog rows that live on later pages (the Projects
    /// UI uses this path).
    async fn scan_filtered(
        &self,
        filter_expression: &str,
        values: &[(&str, aws_sdk_dynamodb::types::AttributeValue)],
    ) -> Result<Vec<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>>, DomainError> {
        let mut items = Vec::new();
        let mut exclusive_start_key = None;
        loop {
            let mut req = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression(filter_expression.to_string());
            for (k, v) in values {
                req = req.expression_attribute_values((*k).to_string(), v.clone());
            }
            if let Some(key) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(key));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| DomainError::External(format!("{e:?}")))?;
            items.extend(resp.items().iter().cloned());
            match resp.last_evaluated_key() {
                Some(key) if !key.is_empty() => exclusive_start_key = Some(key.clone()),
                _ => break,
            }
        }
        Ok(items)
    }
}

#[async_trait]
impl MetadataStore for DdbMetadataStore {
    async fn create_repo(&self, metadata: Repo) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", metadata.id.value)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&metadata)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn update_repo(&self, metadata: Repo) -> Result<(), DomainError> {
        // Same PK/SK as create — overwrite the META JSON blob.
        self.create_repo(metadata).await
    }

    async fn delete_branch(&self, repo_id: RepoId, name: String) -> Result<(), DomainError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .key(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("BRANCH#{}", name)),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn delete_repo(&self, id: RepoId) -> Result<(), DomainError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", id.value)),
            )
            .key(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn delete_tag(&self, repo_id: RepoId, name: String) -> Result<(), DomainError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .key(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("TAG#{}", name)),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn file_history(
        &self,
        repo_id: RepoId,
        path: String,
        limit: i64,
    ) -> Result<Vec<CommitInfo>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("COMMIT#".to_string()),
            )
            .limit((limit) as i32)
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn find_artifact_by_hash(
        &self,
        content_hash: String,
        target: CompilationTarget,
    ) -> Result<Option<ArtifactMetadata>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("content_hash_index".to_string())
            .key_condition_expression("content_hash = :h".to_string())
            .expression_attribute_values(
                ":h".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(content_hash),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Ok(None);
        };
        return Ok(Some(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?));
    }

    async fn get_artifact(&self, id: ArtifactId) -> Result<ArtifactMetadata, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("ARTIFACT#{}", id.value)),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Err(DomainError::NotFound);
        };
        return Ok(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?);
    }

    async fn get_branch(&self, repo_id: RepoId, name: String) -> Result<BranchInfo, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("BRANCH#{}", name)),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Err(DomainError::NotFound);
        };
        return Ok(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?);
    }

    async fn get_dependencies(&self, repo_id: RepoId) -> Result<Vec<DependencyEdge>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("DEP#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("TO#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn get_dependents(&self, dependency: String) -> Result<Vec<DependencyEdge>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("dependency_index".to_string())
            .key_condition_expression("dependency = :d".to_string())
            .expression_attribute_values(
                ":d".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(dependency),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn get_layer(&self, name: String) -> Result<LayerMetadata, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("LAYER#{}", name)),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Err(DomainError::NotFound);
        };
        return Ok(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?);
    }

    async fn get_repo(&self, id: RepoId) -> Result<Repo, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", id.value)),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Err(DomainError::NotFound);
        };
        return Ok(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?);
    }

    async fn get_stub(&self, crate_name: String) -> Result<StubMetadata, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("STUB#{}", crate_name)),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Err(DomainError::NotFound);
        };
        return Ok(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?);
    }

    async fn get_tag(&self, repo_id: RepoId, name: String) -> Result<TagInfo, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("TAG#{}", name)),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        let items = resp.items();
        if items.is_empty() {
            return Err(DomainError::NotFound);
        };
        return Ok(serde_json::from_str::<_>(
            &items[(0) as usize]
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map(|s| s.to_string())
                .map_err(|e| DomainError::External(format!("{:?}", e)))?,
        )?);
    }

    async fn list_artifacts(
        &self,
        repo_id: RepoId,
        branch: Option<String>,
    ) -> Result<Vec<ArtifactMetadata>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("begins_with(PK, :prefix)".to_string())
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("ARTIFACT#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn list_branches(&self, repo_id: RepoId) -> Result<Vec<BranchInfo>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("BRANCH#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn list_commits(
        &self,
        repo_id: RepoId,
        branch: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CommitInfo>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("COMMIT#".to_string()),
            )
            .limit((limit) as i32)
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn list_deployments(
        &self,
        artifact_id: ArtifactId,
    ) -> Result<Vec<DeploymentRecord>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("DEPLOY#{}", artifact_id.value)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("TARGET#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn list_layers(&self) -> Result<Vec<LayerMetadata>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("begins_with(PK, :prefix) AND SK = :sk".to_string())
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("LAYER#".to_string()),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, DomainError> {
        let items = self
            .scan_filtered(
                "begins_with(PK, :prefix) AND SK = :sk",
                &[
                    (
                        ":prefix",
                        aws_sdk_dynamodb::types::AttributeValue::S("REPO#".to_string()),
                    ),
                    (
                        ":sk",
                        aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
                    ),
                ],
            )
            .await?;
        let mut repos = Vec::with_capacity(items.len());
        for item in items {
            let data = item
                .get("data")
                .ok_or_else(|| DomainError::External("missing data".into()))?
                .as_s()
                .map_err(|e| DomainError::External(format!("{e:?}")))?;
            repos.push(
                serde_json::from_str::<Repo>(data)
                    .map_err(|e| DomainError::External(format!("repo META: {e}")))?,
            );
        }
        Ok(repos)
    }

    async fn list_stubs(&self) -> Result<Vec<StubMetadata>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("begins_with(PK, :prefix) AND SK = :sk".to_string())
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("STUB#".to_string()),
            )
            .expression_attribute_values(
                ":sk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn list_tags(&self, repo_id: RepoId) -> Result<Vec<TagInfo>, DomainError> {
        let resp = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)".to_string())
            .expression_attribute_values(
                ":pk".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .expression_attribute_values(
                ":prefix".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("TAG#".to_string()),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        return Ok(resp
            .items()
            .iter()
            .map(|i| {
                serde_json::from_str::<_>(
                    &i.get("data")
                        .ok_or_else(|| DomainError::External("missing data".into()))
                        .unwrap()
                        .as_s()
                        .map(|s| s.to_string())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect());
    }

    async fn put_artifact(&self, artifact: ArtifactMetadata) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!(
                    "ARTIFACT#{}",
                    artifact.id.value
                )),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&artifact)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_branch(&self, repo_id: RepoId, branch: BranchInfo) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("BRANCH#{}", branch.name)),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&branch)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_commit(&self, repo_id: RepoId, commit: CommitInfo) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("COMMIT#{}", commit.hash)),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&commit)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_dependency(&self, edge: DependencyEdge) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("DEP#{}", edge.dependent.value)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("TO#{}", edge.dependency)),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&edge)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_deployment(&self, record: DeploymentRecord) -> Result<(), DomainError> {
        let target_key = serde_json::to_string(&record.target)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!(
                    "DEPLOY#{}",
                    record.artifact_id.value
                )),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("TARGET#{}", target_key)),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&record)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_layer(&self, layer: LayerMetadata) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("LAYER#{}", layer.name)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&layer)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_stub(&self, stub: StubMetadata) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("STUB#{}", stub.crate_name)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S("META".to_string()),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&stub)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
    }

    async fn put_tag(&self, repo_id: RepoId, tag: TagInfo) -> Result<(), DomainError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item(
                "PK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("REPO#{}", repo_id.value)),
            )
            .item(
                "SK".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(format!("TAG#{}", tag.name)),
            )
            .item(
                "data".to_string(),
                aws_sdk_dynamodb::types::AttributeValue::S(serde_json::to_string(&tag)?),
            )
            .send()
            .await
            .map_err(|e| DomainError::External(format!("{e:?}")))?;
        Ok(())
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
