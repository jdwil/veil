//! DynamoDB store for triggers, sharing the `applications` single-table with the
//! artifact registry but in its own PK space.
//!
//! Key layout:
//!   PK = `TRIGGER#{tenant_id}`  SK = `T#{trigger_id}`  data = JSON(TriggerRecord)
//!
//! A per-tenant PK partitions triggers by tenant so listing a tenant's triggers
//! is a single `query` (no scan), and firing schedule/event triggers can query
//! per tenant. Reuses the same DDB client + table name as
//! [`ArtifactRegistryStore`](crate::artifact_registry::ArtifactRegistryStore).

use aws_sdk_dynamodb::types::AttributeValue;

use super::{TriggerError, TriggerRecord};

/// CRUD storage for [`TriggerRecord`]s.
#[derive(Clone)]
pub struct TriggerStore {
    ddb: aws_sdk_dynamodb::Client,
    table: String,
}

impl TriggerStore {
    /// Build a store over an existing DDB client + table (reuse the artifact
    /// registry's client so the host holds one AWS config).
    pub fn new(ddb: aws_sdk_dynamodb::Client, table: String) -> Self {
        Self { ddb, table }
    }

    /// Build from the standard env vars (`VEIL_DDB_TABLE`).
    // Retained: in-flight trigger store env constructor (triggers subsystem).
    #[allow(dead_code)]
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let ddb = aws_sdk_dynamodb::Client::new(&config);
        let table = std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
        Self::new(ddb, table)
    }

    fn pk(tenant_id: &str) -> String {
        format!("TRIGGER#{tenant_id}")
    }

    fn sk(trigger_id: &str) -> String {
        format!("T#{trigger_id}")
    }

    /// Upsert (create or replace) a trigger.
    pub async fn put(&self, record: &TriggerRecord) -> Result<(), TriggerError> {
        let data = serde_json::to_string(record)
            .map_err(|e| TriggerError::Storage(format!("serialize: {e}")))?;
        self.ddb
            .put_item()
            .table_name(&self.table)
            .item("PK", AttributeValue::S(Self::pk(&record.tenant_id)))
            .item("SK", AttributeValue::S(Self::sk(&record.id)))
            .item("data", AttributeValue::S(data))
            .send()
            .await
            .map_err(|e| TriggerError::Storage(format!("{e:?}")))?;
        Ok(())
    }

    /// Get a single trigger by tenant + id.
    pub async fn get(
        &self,
        tenant_id: &str,
        trigger_id: &str,
    ) -> Result<TriggerRecord, TriggerError> {
        let resp = self
            .ddb
            .get_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(Self::pk(tenant_id)))
            .key("SK", AttributeValue::S(Self::sk(trigger_id)))
            .send()
            .await
            .map_err(|e| TriggerError::Storage(format!("{e:?}")))?;

        let item = resp
            .item()
            .ok_or_else(|| TriggerError::NotFound(format!("{tenant_id}/{trigger_id}")))?;
        Self::parse(item)
    }

    /// List all triggers for a tenant.
    pub async fn list_for_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<TriggerRecord>, TriggerError> {
        let mut out = Vec::new();
        let mut start_key = None;
        loop {
            let mut req = self
                .ddb
                .query()
                .table_name(&self.table)
                .key_condition_expression("PK = :pk AND begins_with(SK, :sk)")
                .expression_attribute_values(":pk", AttributeValue::S(Self::pk(tenant_id)))
                .expression_attribute_values(":sk", AttributeValue::S("T#".into()));
            if let Some(k) = start_key {
                req = req.set_exclusive_start_key(Some(k));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| TriggerError::Storage(format!("{e:?}")))?;
            for item in resp.items() {
                out.push(Self::parse(item)?);
            }
            match resp.last_evaluated_key() {
                Some(k) if !k.is_empty() => start_key = Some(k.clone()),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Delete a trigger.
    // Retained: in-flight trigger CRUD delete (triggers subsystem).
    #[allow(dead_code)]
    pub async fn delete(&self, tenant_id: &str, trigger_id: &str) -> Result<(), TriggerError> {
        self.ddb
            .delete_item()
            .table_name(&self.table)
            .key("PK", AttributeValue::S(Self::pk(tenant_id)))
            .key("SK", AttributeValue::S(Self::sk(trigger_id)))
            .send()
            .await
            .map_err(|e| TriggerError::Storage(format!("{e:?}")))?;
        Ok(())
    }

    /// Upsert many triggers (registration path). Best-effort sequential; the
    /// first failure aborts and returns the error (callers may re-run — puts are
    /// idempotent by id).
    pub async fn put_many(&self, records: &[TriggerRecord]) -> Result<(), TriggerError> {
        for r in records {
            self.put(r).await?;
        }
        Ok(())
    }

    fn parse(
        item: &std::collections::HashMap<String, AttributeValue>,
    ) -> Result<TriggerRecord, TriggerError> {
        let data = item
            .get("data")
            .and_then(|v| v.as_s().ok())
            .ok_or_else(|| TriggerError::Storage("missing data field".into()))?;
        serde_json::from_str(data)
            .map_err(|e| TriggerError::Storage(format!("deserialize: {e}")))
    }
}
