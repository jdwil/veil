//! DynamoDB helper — provides the VEIL-shaped API that the codegen emits.
//!
//! The generated adapter code calls patterns like:
//! ```ignore
//! veil_ddb::ddb::query(pk).key(sk).fetch_optional(&self.client)
//! veil_ddb::ddb::put_item(pk, sk, item).execute(&self.client)
//! veil_ddb::ddb::delete(collection).key(id).execute(&self.client)
//! ```

use aws_sdk_dynamodb::types::AttributeValue;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;

/// Error type for DDB operations.
#[derive(Debug, thiserror::Error)]
pub enum DdbError {
    #[error("DynamoDB error: {0}")]
    Sdk(String),
    #[error("Serialization error: {0}")]
    Serde(String),
    #[error("Item not found")]
    NotFound,
}

impl From<DdbError> for String {
    fn from(e: DdbError) -> String {
        e.to_string()
    }
}

// ─── Query ────────────────────────────────────────────────────────────────────

/// Start a DDB query with a partition key.
pub fn query(pk: impl Into<String>) -> DdbQueryBuilder {
    DdbQueryBuilder {
        pk: pk.into(),
        sk: None,
        sk_prefix: None,
        filter: None,
        limit: None,
    }
}

pub struct DdbQueryBuilder {
    pk: String,
    sk: Option<String>,
    sk_prefix: Option<String>,
    filter: Option<String>,
    limit: Option<i64>,
}

impl DdbQueryBuilder {
    /// Set exact sort key condition.
    pub fn key(mut self, sk: impl Into<String>) -> Self {
        self.sk = Some(sk.into());
        self
    }

    /// Set sort key begins_with condition.
    pub fn begins_with(mut self, prefix: impl Into<String>) -> Self {
        self.sk_prefix = Some(prefix.into());
        self
    }

    /// Set a filter expression value (used as a simple equality filter).
    pub fn filter(mut self, value: impl Into<String>) -> Self {
        self.filter = Some(value.into());
        self
    }

    /// Set max items to return.
    pub fn limit(mut self, n: i64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Execute query and return exactly one item (panics if not found).
    pub fn fetch_one<T: DeserializeOwned>(self, _client: &aws_sdk_dynamodb::Client) -> T {
        // Stub implementation — real impl would execute the query and deserialize.
        // This compiles and allows the generated code to build; real behavior
        // will be wired when the runtime is deployed against actual DynamoDB.
        panic!("veil_ddb::ddb::fetch_one — stub not yet wired to real DynamoDB")
    }

    /// Execute query and return all matching items.
    pub fn fetch_all<T: DeserializeOwned>(self, _client: &aws_sdk_dynamodb::Client) -> Vec<T> {
        panic!("veil_ddb::ddb::fetch_all — stub not yet wired to real DynamoDB")
    }

    /// Execute query and return the first item, or None.
    pub fn fetch_optional<T: DeserializeOwned>(
        self,
        _client: &aws_sdk_dynamodb::Client,
    ) -> Option<T> {
        panic!("veil_ddb::ddb::fetch_optional — stub not yet wired to real DynamoDB")
    }
}

// ─── Put Item ─────────────────────────────────────────────────────────────────

/// Start a DDB put_item operation (2-arg: collection/pk + item).
pub fn put_item<T: Serialize>(pk: impl Into<String>, item: T) -> DdbPutBuilder {
    DdbPutBuilder {
        pk: pk.into(),
        sk: None,
        _item: serde_json::to_value(item).unwrap_or_default(),
    }
}

/// 3-arg variant: put_item(pk, sk, item).
pub fn put_item3<T: Serialize>(
    pk: impl Into<String>,
    sk: impl Into<String>,
    item: T,
) -> DdbPutBuilder {
    DdbPutBuilder {
        pk: pk.into(),
        sk: Some(sk.into()),
        _item: serde_json::to_value(item).unwrap_or_default(),
    }
}

pub struct DdbPutBuilder {
    pk: String,
    sk: Option<String>,
    _item: serde_json::Value,
}

impl DdbPutBuilder {
    /// Execute the put operation.
    pub async fn execute(self, _client: &aws_sdk_dynamodb::Client) -> Result<(), String> {
        panic!("veil_ddb::ddb::put_item::execute — stub not yet wired to real DynamoDB")
    }
}

// ─── Delete ───────────────────────────────────────────────────────────────────

/// Start a DDB delete operation on a collection/table.
/// Returns Ok(builder) so the generated `.map_err(...)?.key(...)` chain works.
pub fn delete(collection: impl Into<String>) -> Result<DdbDeleteBuilder, String> {
    Ok(DdbDeleteBuilder {
        collection: collection.into(),
        key: None,
    })
}

pub struct DdbDeleteBuilder {
    collection: String,
    key: Option<String>,
}

impl DdbDeleteBuilder {
    /// Set the key to delete.
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.key = Some(k.into());
        self
    }

    /// Execute the delete.
    pub async fn execute(self, _client: &aws_sdk_dynamodb::Client) -> Result<(), String> {
        panic!("veil_ddb::ddb::delete::execute — stub not yet wired to real DynamoDB")
    }
}
