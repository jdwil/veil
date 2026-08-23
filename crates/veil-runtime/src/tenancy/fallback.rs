//! Fallback combinator for chaining tenant resolution strategies.
//!
//! Tries the primary resolver first; if it fails with a non-fatal error,
//! falls back to the secondary. Compose multiple fallbacks for longer chains.

use async_trait::async_trait;
use std::sync::Arc;
use veil_shared::Principal;

use super::{RequestContext, ResolutionError, TenantId, TenantResolver};

/// Chains two resolvers: tries `primary`, falls back to `fallback` on failure.
///
/// Only `NotFound` and `MissingClaim` errors trigger the fallback.
/// `LookupFailed` and `Internal` errors propagate immediately (they indicate
/// infrastructure problems, not missing data).
///
/// # Example
/// ```ignore
/// let resolver = FallbackResolver::new(
///     ClaimResolver::new("org_id"),
///     SubdomainResolver::new("app.example.com"),
/// );
/// ```
pub struct FallbackResolver {
    primary: Box<dyn TenantResolver>,
    fallback: Box<dyn TenantResolver>,
}

impl FallbackResolver {
    pub fn new(
        primary: impl TenantResolver + 'static,
        fallback: impl TenantResolver + 'static,
    ) -> Self {
        Self {
            primary: Box::new(primary),
            fallback: Box::new(fallback),
        }
    }

    /// Construct from Arc'd resolvers (used by config builder).
    pub fn from_arcs(
        primary: Arc<dyn TenantResolver>,
        fallback: Arc<dyn TenantResolver>,
    ) -> Self {
        Self {
            primary: Box::new(primary),
            fallback: Box::new(fallback),
        }
    }

    /// Build a chain from a vec of resolvers (left-to-right priority).
    /// Panics if the vec is empty.
    pub fn chain(resolvers: Vec<Box<dyn TenantResolver>>) -> Box<dyn TenantResolver> {
        assert!(!resolvers.is_empty(), "resolver chain must not be empty");
        let mut iter = resolvers.into_iter().rev();
        let mut current: Box<dyn TenantResolver> = iter.next().unwrap();
        for resolver in iter {
            current = Box::new(Self {
                primary: resolver,
                fallback: current,
            });
        }
        current
    }
}

impl std::fmt::Debug for FallbackResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackResolver")
            .field("primary", &"<dyn TenantResolver>")
            .field("fallback", &"<dyn TenantResolver>")
            .finish()
    }
}

/// Returns true if this error should trigger a fallback (data not found),
/// false if it should propagate (infrastructure failure).
fn is_recoverable(err: &ResolutionError) -> bool {
    matches!(err, ResolutionError::NotFound(_) | ResolutionError::MissingClaim(_))
}

#[async_trait]
impl TenantResolver for FallbackResolver {
    async fn resolve(
        &self,
        principal: &Principal,
        ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError> {
        match self.primary.resolve(principal, ctx).await {
            Ok(tenant) => Ok(tenant),
            Err(e) if is_recoverable(&e) => {
                tracing::debug!(
                    primary_error = %e,
                    "primary resolver failed, trying fallback"
                );
                self.fallback.resolve(principal, ctx).await
            }
            Err(e) => Err(e),
        }
    }
}
