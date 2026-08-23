//! DynamoDB + S3 storage for the Artifact Registry.
//!
//! Uses the same VEIL_DDB_TABLE and BUCKET as the rest of the runtime.
//! DDB key schema:
//!   PK = "ARTREG#{artifact_id}"  SK = "V#{version}"  data = JSON(ArtifactRecord)
//!   PK = "ARTREG#{artifact_id}"  SK = "LATEST"       data = JSON(ArtifactRecord)
//!
//! S3 key: "artifacts/{artifact_id}/{version}/{filename}"

use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;

use super::types::*;

/// Storage backend for the artifact registry.
#[derive(Clone)]
pub struct ArtifactRegistryStore {
    pub ddb: aws_sdk_dynamodb::Client,
    pub s3: S3Client,
    pub table: String,
    pub bucket: String,
}

impl ArtifactRegistryStore {
    pub fn new(
        ddb: aws_sdk_dynamodb::Client,
        s3: S3Client,
        table: String,
        bucket: String,
    ) -> Self {
        Self {
            ddb,
            s3,
            table,
            bucket,
        }
    }

    /// Create from the standard env vars (VEIL_DDB_TABLE, BUCKET).
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let ddb = aws_sdk_dynamodb::Client::new(&config);
        let s3 = S3Client::new(&config);
        let table =
            std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
        let bucket = std::env::var("BUCKET").unwrap_or_else(|_| "veil-runtime-dev".into());
        Self::new(ddb, s3, table, bucket)
    }

    // ─── DynamoDB: Write ─────────────────────────────────────────────────

    /// Register (or update) an artifact record.
    /// Writes both the versioned row and the LATEST pointer.
    pub async fn put_artifact(&self, record: &ArtifactRecord) -> Result<(), RegistryError> {
        let pk = format!("ARTREG#{}", record.id);
        let sk_version = format!("V#{}", record.version);
        let data = serde_json::to_string(record)
            .map_err(|e| RegistryError::Storage(format!("serialize: {e}")))?;

        // Write versioned row.
        self.ddb
            .put_item()
            .table_name(&self.table)
            .item("PK", AttributeValue::S(pk.clone()))
            .item("SK", AttributeValue::S(sk_version))
            .item("data", AttributeValue::S(data.clone()))
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

        // Write LATEST pointer (overwrite).
        self.ddb
            .put_item()
            .table_name(&self.table)
            .item("PK", AttributeValue::S(pk))
            .item("SK", AttributeValue::S("LATEST".into()))
            .item("data", AttributeValue::S(data))
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

        Ok(())
    }

    /// Delete an artifact record (both versioned row and LATEST).
    pub async fn delete_artifact(
        &self,
        artifact_id: &str,
        version: &str,
    ) -> Result<(), RegistryError> {
        let pk = format!("ARTREG#{}", artifact_id);
        let sk = format!("V#{}", version);

        self.ddb
            .delete_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(pk.clone()))
            .key("SK", AttributeValue::S(sk))
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

        // If this was the latest, remove LATEST pointer too.
        // (Caller should re-point LATEST if older versions exist.)
        self.ddb
            .delete_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S("LATEST".into()))
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

        Ok(())
    }

    // ─── DynamoDB: Read ──────────────────────────────────────────────────

    /// Get the latest version of an artifact by id.
    pub async fn get_latest(&self, artifact_id: &str) -> Result<ArtifactRecord, RegistryError> {
        let pk = format!("ARTREG#{}", artifact_id);
        let resp = self
            .ddb
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":sk", AttributeValue::S("LATEST".into()))
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

        let items = resp.items();
        if items.is_empty() {
            return Err(RegistryError::NotFound(format!(
                "artifact not found: {artifact_id}"
            )));
        }

        Self::parse_record(&items[0])
    }

    /// Get a specific version of an artifact.
    pub async fn get_version(
        &self,
        artifact_id: &str,
        version: &str,
    ) -> Result<ArtifactRecord, RegistryError> {
        let pk = format!("ARTREG#{}", artifact_id);
        let sk = format!("V#{}", version);
        let resp = self
            .ddb
            .query()
            .table_name(&self.table)
            .key_condition_expression("PK = :pk AND SK = :sk")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":sk", AttributeValue::S(sk))
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

        let items = resp.items();
        if items.is_empty() {
            return Err(RegistryError::NotFound(format!(
                "artifact version not found: {artifact_id}@{version}"
            )));
        }

        Self::parse_record(&items[0])
    }

    /// List all registered artifacts (LATEST versions only).
    /// Uses a scan with filter — acceptable for Phase 1 registry size.
    pub async fn list_all(&self) -> Result<Vec<ArtifactRecord>, RegistryError> {
        let mut items = Vec::new();
        let mut exclusive_start_key = None;

        loop {
            let mut req = self
                .ddb
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(PK, :prefix) AND SK = :sk")
                .expression_attribute_values(
                    ":prefix",
                    AttributeValue::S("ARTREG#".into()),
                )
                .expression_attribute_values(":sk", AttributeValue::S("LATEST".into()));

            if let Some(key) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(key));
            }

            let resp = req
                .send()
                .await
                .map_err(|e| RegistryError::Storage(format!("{e:?}")))?;

            items.extend(resp.items().iter().cloned());

            match resp.last_evaluated_key() {
                Some(key) if !key.is_empty() => exclusive_start_key = Some(key.clone()),
                _ => break,
            }
        }

        items
            .iter()
            .map(|item| Self::parse_record(item))
            .collect()
    }

    /// List all artifacts visible to a specific tenant (LATEST only).
    pub async fn list_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ArtifactRecord>, RegistryError> {
        let all = self.list_all().await?;
        Ok(all
            .into_iter()
            .filter(|r| match &r.tenant_visibility {
                TenantVisibility::All => true,
                TenantVisibility::Specific(tenants) => tenants.contains(&tenant_id.to_string()),
                TenantVisibility::None => false,
            })
            .collect())
    }

    // ─── S3: Blob Storage ────────────────────────────────────────────────

    /// Upload an artifact blob to S3.
    /// Returns the S3 key.
    pub async fn put_blob(
        &self,
        artifact_id: &str,
        version: &str,
        filename: &str,
        data: Vec<u8>,
    ) -> Result<String, RegistryError> {
        let key = format!("artifacts/{artifact_id}/{version}/{filename}");
        self.s3
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(data.into())
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("s3 put: {e:?}")))?;
        Ok(key)
    }

    /// Get an artifact blob from S3.
    pub async fn get_blob(&self, blob_key: &str) -> Result<Vec<u8>, RegistryError> {
        let resp = self
            .s3
            .get_object()
            .bucket(&self.bucket)
            .key(blob_key)
            .send()
            .await
            .map_err(|e| RegistryError::Storage(format!("s3 get: {e:?}")))?;

        resp.body
            .collect()
            .await
            .map(|b| b.into_bytes().to_vec())
            .map_err(|e| RegistryError::Storage(format!("s3 body: {e:?}")))
    }

    /// Generate a direct URL for an artifact blob (not pre-signed for now;
    /// pre-signing can be added when auth boundary is built in Phase 2).
    pub fn blob_url(&self, blob_key: &str) -> String {
        // For local/dev, direct S3 URL. Production would use CloudFront or presign.
        format!(
            "https://{}.s3.amazonaws.com/{}",
            self.bucket, blob_key
        )
    }

    // ─── Resolve APIs ────────────────────────────────────────────────────

    /// Resolve all contributions of a given kind visible to tenant + principal.
    pub async fn resolve_contributions(
        &self,
        tenant_id: &str,
        principal: &Principal,
        kind: ContributionKind,
    ) -> Result<Vec<ResolvedContribution>, RegistryError> {
        let artifacts = self.list_for_tenant(tenant_id).await?;
        let mut results = Vec::new();

        for artifact in &artifacts {
            for contribution in &artifact.contributions {
                let matches_kind = match (kind, contribution) {
                    (ContributionKind::MenuItem, Contribution::MenuItem { .. }) => true,
                    (ContributionKind::Route, Contribution::Route { .. }) => true,
                    (ContributionKind::SlotFill, Contribution::SlotFill { .. }) => true,
                    (ContributionKind::BackendFunction, Contribution::BackendFunction { .. }) => {
                        true
                    }
                    _ => false,
                };

                if !matches_kind {
                    continue;
                }

                // Role filtering: if contribution specifies roles, principal must have at least one.
                let role_ok = match contribution {
                    Contribution::MenuItem { roles, .. } => {
                        roles.is_empty()
                            || roles.iter().any(|r| principal.roles.contains(r))
                    }
                    Contribution::BackendFunction { .. } => {
                        // For backend functions, capabilities are what the function NEEDS,
                        // not a role gate. Always visible if tenant-visible.
                        true
                    }
                    _ => true,
                };

                if role_ok {
                    results.push(ResolvedContribution {
                        artifact_id: artifact.id.clone(),
                        artifact_version: artifact.version.clone(),
                        contribution: contribution.clone(),
                    });
                }
            }
        }

        Ok(results)
    }

    /// Resolve a backend function by its package-style id (e.g. "pkg:orders/process_order").
    /// Returns the latest version's record if the function is registered and tenant-visible.
    pub async fn resolve_function(
        &self,
        tenant_id: &str,
        function_id: &str,
    ) -> Result<ArtifactRecord, RegistryError> {
        // function_id might be "pkg:orders/process_order@2.1.0" (with version pin)
        // or just "pkg:orders/process_order" (resolve latest).
        let (artifact_id, version) = if let Some(idx) = function_id.rfind('@') {
            (&function_id[..idx], Some(&function_id[idx + 1..]))
        } else {
            (function_id, None)
        };

        let record = if let Some(v) = version {
            self.get_version(artifact_id, v).await?
        } else {
            self.get_latest(artifact_id).await?
        };

        // Check tenant visibility.
        match &record.tenant_visibility {
            TenantVisibility::All => Ok(record),
            TenantVisibility::Specific(tenants) => {
                if tenants.contains(&tenant_id.to_string()) {
                    Ok(record)
                } else {
                    Err(RegistryError::NotFound(format!(
                        "function not visible to tenant {tenant_id}: {function_id}"
                    )))
                }
            }
            TenantVisibility::None => Err(RegistryError::NotFound(format!(
                "function not published: {function_id}"
            ))),
        }
    }

    /// Resolve a UI artifact to a downloadable URL.
    pub async fn resolve_ui_artifact(
        &self,
        tenant_id: &str,
        artifact_id: &str,
    ) -> Result<ArtifactUrl, RegistryError> {
        let record = self.get_latest(artifact_id).await?;

        // Check tenant visibility.
        match &record.tenant_visibility {
            TenantVisibility::All => {}
            TenantVisibility::Specific(tenants) => {
                if !tenants.contains(&tenant_id.to_string()) {
                    return Err(RegistryError::NotFound(format!(
                        "artifact not visible to tenant {tenant_id}: {artifact_id}"
                    )));
                }
            }
            TenantVisibility::None => {
                return Err(RegistryError::NotFound(format!(
                    "artifact not published: {artifact_id}"
                )));
            }
        }

        let blob_key = record.blob_key.as_ref().ok_or_else(|| {
            RegistryError::NotFound(format!("artifact has no blob: {artifact_id}"))
        })?;

        Ok(ArtifactUrl {
            url: self.blob_url(blob_key),
            artifact_id: record.id.clone(),
            version: record.version.clone(),
            artifact_type: record.artifact_type.clone(),
        })
    }

    // ─── Helpers ─────────────────────────────────────────────────────────

    fn parse_record(
        item: &std::collections::HashMap<String, AttributeValue>,
    ) -> Result<ArtifactRecord, RegistryError> {
        let data = item
            .get("data")
            .and_then(|v| v.as_s().ok())
            .ok_or_else(|| RegistryError::Storage("missing data field".into()))?;
        serde_json::from_str(data)
            .map_err(|e| RegistryError::Storage(format!("deserialize: {e}")))
    }
}
