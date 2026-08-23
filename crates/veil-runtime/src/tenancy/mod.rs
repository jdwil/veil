//! Tenant resolution framework.
//!
//! Authors configure how incoming requests map to a TenantId.
//! Built-in strategies: claim extraction (JWT), subdomain, header, DynamoDB lookup.
//! Compose via [`FallbackResolver`] for chained resolution.

mod claim;
mod config;
mod fallback;
mod header;
mod lookup;
mod middleware;
mod subdomain;

pub use claim::ClaimResolver;
pub use config::{ResolutionStrategy, TenantResolutionConfig};
pub use fallback::FallbackResolver;
pub use header::HeaderResolver;
pub use lookup::LookupResolver;
pub use middleware::{TenantResolutionLayer, TenantResolutionService};
pub use subdomain::SubdomainResolver;

use async_trait::async_trait;
use std::fmt;
use veil_shared::Principal;

// ─── Core Types ─────────────────────────────────────────────────────────────

/// Opaque tenant identifier resolved from the request context.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TenantId(pub String);

impl TenantId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Request context passed to resolvers — headers, host, path info.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Request host header value (e.g. "acme.app.example.com").
    pub host: Option<String>,
    /// All request headers as name→value pairs (lowercase names).
    pub headers: Vec<(String, String)>,
    /// Request URI path.
    pub path: String,
}

impl RequestContext {
    /// Get the first header value matching `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k == &lower)
            .map(|(_, v)| v.as_str())
    }
}

/// Errors arising from tenant resolution.
#[derive(Debug, thiserror::Error)]
pub enum ResolutionError {
    /// The resolver could not determine a tenant from the available context.
    #[error("tenant not found: {0}")]
    NotFound(String),

    /// The principal lacks the required claim/field.
    #[error("missing claim: {0}")]
    MissingClaim(String),

    /// External service (e.g. DynamoDB) failed.
    #[error("lookup failed: {0}")]
    LookupFailed(String),

    /// Catch-all for unexpected issues.
    #[error("internal error: {0}")]
    Internal(String),
}

// ─── Trait ──────────────────────────────────────────────────────────────────

/// Author-configured function from (principal + request context) → TenantId.
///
/// The runtime calls this on every incoming request after authentication.
/// Built-in implementations cover common patterns; authors can compose them
/// with [`FallbackResolver`] or provide a custom impl.
#[async_trait]
pub trait TenantResolver: Send + Sync {
    /// Resolve the tenant for this request.
    async fn resolve(
        &self,
        principal: &Principal,
        ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError>;
}

/// Blanket implementation so `Arc<dyn TenantResolver>` can be used as a TenantResolver.
#[async_trait]
impl TenantResolver for std::sync::Arc<dyn TenantResolver> {
    async fn resolve(
        &self,
        principal: &Principal,
        ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError> {
        (**self).resolve(principal, ctx).await
    }
}

#[cfg(test)]
mod tests;
