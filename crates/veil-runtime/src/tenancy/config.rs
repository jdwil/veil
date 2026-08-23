//! Configuration model for tenant resolution.
//!
//! Authors declare which strategy (or chain of strategies) the runtime uses
//! to resolve tenants. This config is read from environment variables or
//! provided programmatically.

use std::sync::Arc;

use super::{
    ClaimResolver, FallbackResolver, HeaderResolver, LookupResolver, SubdomainResolver,
    TenantResolver,
};

/// Identifies a built-in resolution strategy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStrategy {
    /// Extract tenant from a named JWT claim on the Principal.
    Claim,
    /// Extract tenant from request subdomain.
    Subdomain,
    /// Extract tenant from a named request header.
    Header,
    /// Look up tenant from a DynamoDB mapping table.
    Lookup,
}

/// Author-facing configuration for tenant resolution.
///
/// # Environment Variables
///
/// | Variable | Default | Description |
/// |----------|---------|-------------|
/// | `VEIL_TENANT_STRATEGY` | `claim` | Primary strategy: `claim`, `subdomain`, `header`, `lookup` |
/// | `VEIL_TENANT_FALLBACK` | (none) | Fallback strategy (same options) |
/// | `VEIL_TENANT_CLAIM_FIELD` | `org_id` | Claim field name (for `claim` strategy) |
/// | `VEIL_TENANT_HEADER` | `x-tenant-id` | Header name (for `header` strategy) |
/// | `VEIL_TENANT_BASE_DOMAIN` | (required for `subdomain`) | Base domain to strip |
/// | `VEIL_TENANT_LOOKUP_TABLE` | value of `VEIL_DDB_TABLE` | DynamoDB table for lookup |
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TenantResolutionConfig {
    /// Primary resolution strategy.
    pub strategy: ResolutionStrategy,
    /// Optional fallback strategy (tried when primary returns NotFound/MissingClaim).
    pub fallback: Option<ResolutionStrategy>,
    /// Claim field name for `Claim` strategy.
    pub claim_field: String,
    /// Header name for `Header` strategy.
    pub header_name: String,
    /// Base domain for `Subdomain` strategy (e.g. "app.example.com").
    pub base_domain: Option<String>,
    /// DynamoDB table name for `Lookup` strategy.
    pub lookup_table: Option<String>,
}

impl Default for TenantResolutionConfig {
    fn default() -> Self {
        Self {
            strategy: ResolutionStrategy::Claim,
            fallback: None,
            claim_field: "org_id".into(),
            header_name: "x-tenant-id".into(),
            base_domain: None,
            lookup_table: None,
        }
    }
}

impl TenantResolutionConfig {
    /// Read configuration from environment variables.
    pub fn from_env() -> Self {
        let strategy = std::env::var("VEIL_TENANT_STRATEGY")
            .ok()
            .and_then(|s| parse_strategy(&s))
            .unwrap_or(ResolutionStrategy::Claim);

        let fallback = std::env::var("VEIL_TENANT_FALLBACK")
            .ok()
            .and_then(|s| parse_strategy(&s));

        let claim_field = std::env::var("VEIL_TENANT_CLAIM_FIELD")
            .unwrap_or_else(|_| "org_id".into());

        let header_name = std::env::var("VEIL_TENANT_HEADER")
            .unwrap_or_else(|_| "x-tenant-id".into());

        let base_domain = std::env::var("VEIL_TENANT_BASE_DOMAIN").ok();

        let lookup_table = std::env::var("VEIL_TENANT_LOOKUP_TABLE")
            .or_else(|_| std::env::var("VEIL_DDB_TABLE"))
            .ok();

        Self {
            strategy,
            fallback,
            claim_field,
            header_name,
            base_domain,
            lookup_table,
        }
    }

    /// Build the resolver (or resolver chain) from this config.
    ///
    /// Requires a DynamoDB client when using `Lookup` strategy.
    pub async fn build_resolver(
        &self,
        ddb_client: Option<aws_sdk_dynamodb::Client>,
    ) -> Result<Arc<dyn TenantResolver>, String> {
        let primary = self.build_single(&self.strategy, &ddb_client)?;

        let resolver: Arc<dyn TenantResolver> = match &self.fallback {
            Some(fallback_strategy) => {
                let fallback = self.build_single(fallback_strategy, &ddb_client)?;
                Arc::new(FallbackResolver::from_arcs(primary, fallback))
            }
            None => primary,
        };

        Ok(resolver)
    }

    fn build_single(
        &self,
        strategy: &ResolutionStrategy,
        ddb_client: &Option<aws_sdk_dynamodb::Client>,
    ) -> Result<Arc<dyn TenantResolver>, String> {
        match strategy {
            ResolutionStrategy::Claim => {
                Ok(Arc::new(ClaimResolver::new(&self.claim_field)))
            }
            ResolutionStrategy::Subdomain => {
                let base = self.base_domain.as_deref().ok_or_else(|| {
                    "VEIL_TENANT_BASE_DOMAIN is required for subdomain strategy".to_string()
                })?;
                Ok(Arc::new(SubdomainResolver::new(base)))
            }
            ResolutionStrategy::Header => {
                Ok(Arc::new(HeaderResolver::new(&self.header_name)))
            }
            ResolutionStrategy::Lookup => {
                let client = ddb_client.clone().ok_or_else(|| {
                    "DynamoDB client required for lookup strategy".to_string()
                })?;
                let table = self.lookup_table.as_deref().ok_or_else(|| {
                    "VEIL_TENANT_LOOKUP_TABLE (or VEIL_DDB_TABLE) is required for lookup strategy"
                        .to_string()
                })?;
                Ok(Arc::new(LookupResolver::new(client, table)))
            }
        }
    }
}

/// Parse a strategy string (case-insensitive).
fn parse_strategy(s: &str) -> Option<ResolutionStrategy> {
    match s.to_lowercase().as_str() {
        "claim" | "claims" | "jwt" => Some(ResolutionStrategy::Claim),
        "subdomain" | "domain" => Some(ResolutionStrategy::Subdomain),
        "header" => Some(ResolutionStrategy::Header),
        "lookup" | "dynamodb" | "ddb" => Some(ResolutionStrategy::Lookup),
        _ => None,
    }
}
