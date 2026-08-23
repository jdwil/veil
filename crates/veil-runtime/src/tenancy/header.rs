//! Header-based tenant resolution.
//!
//! Reads the tenant identifier from a configured request header.
//! Common in API gateway setups where the gateway injects X-Tenant-Id.

use async_trait::async_trait;
use veil_shared::Principal;

use super::{RequestContext, ResolutionError, TenantId, TenantResolver};

/// Default header name used if none is configured.
pub const DEFAULT_TENANT_HEADER: &str = "x-tenant-id";

/// Resolves tenant from a named request header.
///
/// # Example
/// ```ignore
/// let resolver = HeaderResolver::default(); // uses "x-tenant-id"
/// let resolver = HeaderResolver::new("x-org-id"); // custom header
/// ```
#[derive(Debug, Clone)]
pub struct HeaderResolver {
    /// Header name to read (stored lowercase for case-insensitive matching).
    header_name: String,
}

impl HeaderResolver {
    pub fn new(header_name: impl Into<String>) -> Self {
        Self {
            header_name: header_name.into().to_lowercase(),
        }
    }

    pub fn header_name(&self) -> &str {
        &self.header_name
    }
}

impl Default for HeaderResolver {
    fn default() -> Self {
        Self::new(DEFAULT_TENANT_HEADER)
    }
}

#[async_trait]
impl TenantResolver for HeaderResolver {
    async fn resolve(
        &self,
        _principal: &Principal,
        ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError> {
        ctx.header(&self.header_name)
            .filter(|v| !v.is_empty())
            .map(|v| TenantId::new(v.to_string()))
            .ok_or_else(|| {
                ResolutionError::NotFound(format!(
                    "header '{}' not present or empty",
                    self.header_name
                ))
            })
    }
}
