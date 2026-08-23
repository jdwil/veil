//! Claim-based tenant resolution.
//!
//! Extracts the tenant from a configured claim field on the [`Principal`].
//! This is the most common pattern when using JWT-based auth: the IdP includes
//! an org/tenant claim in the token, and the auth layer populates `Principal.claims`.

use async_trait::async_trait;
use veil_shared::Principal;

use super::{RequestContext, ResolutionError, TenantId, TenantResolver};

/// Resolves tenant from a named claim on the authenticated principal.
///
/// # Example
/// ```ignore
/// let resolver = ClaimResolver::new("org_id");
/// // Given a Principal with claims = {"org_id": "acme"}, resolves TenantId("acme").
/// ```
#[derive(Debug, Clone)]
pub struct ClaimResolver {
    /// The claim field name to extract (e.g. "org_id", "tenant_id", "company").
    claim_field: String,
}

impl ClaimResolver {
    pub fn new(claim_field: impl Into<String>) -> Self {
        Self {
            claim_field: claim_field.into(),
        }
    }

    pub fn claim_field(&self) -> &str {
        &self.claim_field
    }
}

#[async_trait]
impl TenantResolver for ClaimResolver {
    async fn resolve(
        &self,
        principal: &Principal,
        _ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError> {
        principal
            .claims
            .get(&self.claim_field)
            .filter(|v| !v.is_empty())
            .map(|v| TenantId::new(v.clone()))
            .ok_or_else(|| {
                ResolutionError::MissingClaim(format!(
                    "principal '{}' has no claim '{}'",
                    principal.id, self.claim_field
                ))
            })
    }
}
