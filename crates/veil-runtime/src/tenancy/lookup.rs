//! DynamoDB lookup-based tenant resolution.
//!
//! Resolves tenant by querying a mapping table that maps principal IDs to tenant IDs.
//! Used when the relationship isn't encoded in the token itself (e.g. user can
//! belong to multiple orgs, or tenant assignment is managed externally).

use async_trait::async_trait;
use aws_sdk_dynamodb::Client as DdbClient;
use veil_shared::Principal;

use super::{RequestContext, ResolutionError, TenantId, TenantResolver};

/// Resolves tenant from a DynamoDB mapping table.
///
/// Table schema expected:
/// - PK: `TENANT_MAP#<principal_id>`
/// - Attribute `tenant_id`: the resolved tenant identifier
///
/// # Example
/// ```ignore
/// let client = aws_sdk_dynamodb::Client::new(&config);
/// let resolver = LookupResolver::new(client, "veil-runtime-dev");
/// ```
#[derive(Debug, Clone)]
pub struct LookupResolver {
    client: DdbClient,
    table: String,
    /// The DynamoDB partition key prefix. Default: "TENANT_MAP#".
    pk_prefix: String,
    /// The attribute name holding the tenant id. Default: "tenant_id".
    tenant_attr: String,
}

impl LookupResolver {
    pub fn new(client: DdbClient, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
            pk_prefix: "TENANT_MAP#".into(),
            tenant_attr: "tenant_id".into(),
        }
    }

    /// Override the PK prefix (default `TENANT_MAP#`).
    pub fn with_pk_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.pk_prefix = prefix.into();
        self
    }

    /// Override the attribute name that holds the tenant id (default `tenant_id`).
    pub fn with_tenant_attr(mut self, attr: impl Into<String>) -> Self {
        self.tenant_attr = attr.into();
        self
    }
}

#[async_trait]
impl TenantResolver for LookupResolver {
    async fn resolve(
        &self,
        principal: &Principal,
        _ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError> {
        let pk = format!("{}{}", self.pk_prefix, principal.id);

        let result = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("PK", aws_sdk_dynamodb::types::AttributeValue::S(pk.clone()))
            .key(
                "SK",
                aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
            )
            .projection_expression(&self.tenant_attr)
            .send()
            .await
            .map_err(|e| {
                ResolutionError::LookupFailed(format!("DynamoDB GetItem failed: {e}"))
            })?;

        let item = result.item().ok_or_else(|| {
            ResolutionError::NotFound(format!(
                "no tenant mapping for principal '{}'",
                principal.id
            ))
        })?;

        let tenant_val = item.get(&self.tenant_attr).ok_or_else(|| {
            ResolutionError::NotFound(format!(
                "mapping for '{}' exists but has no '{}' attribute",
                principal.id, self.tenant_attr
            ))
        })?;

        match tenant_val.as_s() {
            Ok(s) if !s.is_empty() => Ok(TenantId::new(s.clone())),
            _ => Err(ResolutionError::NotFound(format!(
                "tenant_id attribute for '{}' is empty or not a string",
                principal.id
            ))),
        }
    }
}
