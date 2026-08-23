//! Subdomain-based tenant resolution.
//!
//! Extracts the tenant from the first subdomain label of the request host.
//! E.g. `acme.app.example.com` → TenantId("acme").

use async_trait::async_trait;
use veil_shared::Principal;

use super::{RequestContext, ResolutionError, TenantId, TenantResolver};

/// Resolves tenant from the first subdomain of the request host.
///
/// Strips the leftmost label from the host (before the first `.`). The
/// `base_domain` is used to determine how many labels to skip from the right.
///
/// # Example
/// ```ignore
/// let resolver = SubdomainResolver::new("app.example.com");
/// // Host: "acme.app.example.com" → TenantId("acme")
/// // Host: "app.example.com" → ResolutionError::NotFound
/// ```
#[derive(Debug, Clone)]
pub struct SubdomainResolver {
    /// Base domain that is NOT part of the tenant prefix.
    /// E.g. "app.example.com" means the first label before it is the tenant.
    base_domain: String,
    /// Number of labels in the base domain (cached for fast comparison).
    base_label_count: usize,
}

impl SubdomainResolver {
    pub fn new(base_domain: impl Into<String>) -> Self {
        let base = base_domain.into();
        let count = base.split('.').count();
        Self {
            base_domain: base,
            base_label_count: count,
        }
    }

    pub fn base_domain(&self) -> &str {
        &self.base_domain
    }
}

#[async_trait]
impl TenantResolver for SubdomainResolver {
    async fn resolve(
        &self,
        _principal: &Principal,
        ctx: &RequestContext,
    ) -> Result<TenantId, ResolutionError> {
        let host = ctx.host.as_deref().unwrap_or("");

        // Strip port if present.
        let host_no_port = host.split(':').next().unwrap_or(host);

        if host_no_port.is_empty() {
            return Err(ResolutionError::NotFound(
                "no host header in request".into(),
            ));
        }

        let labels: Vec<&str> = host_no_port.split('.').collect();
        let total = labels.len();

        // Must have at least one label more than the base domain.
        if total <= self.base_label_count {
            return Err(ResolutionError::NotFound(format!(
                "host '{}' has no subdomain prefix before base '{}'",
                host_no_port, self.base_domain
            )));
        }

        // Verify the suffix matches the base domain.
        let suffix = &labels[total - self.base_label_count..];
        let base_labels: Vec<&str> = self.base_domain.split('.').collect();
        if suffix
            .iter()
            .zip(base_labels.iter())
            .any(|(a, b)| !a.eq_ignore_ascii_case(b))
        {
            return Err(ResolutionError::NotFound(format!(
                "host '{}' does not end with base domain '{}'",
                host_no_port, self.base_domain
            )));
        }

        // The tenant is the leftmost label (first subdomain).
        let tenant = labels[0];
        if tenant.is_empty() {
            return Err(ResolutionError::NotFound(
                "empty subdomain label".into(),
            ));
        }

        Ok(TenantId::new(tenant.to_lowercase()))
    }
}
