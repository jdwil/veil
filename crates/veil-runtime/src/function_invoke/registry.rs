//! FunctionRegistry — resolution, caching, and in-process function registration.
//!
//! The registry holds in-process callable handles keyed by function_id.
//! Resolution checks the artifact registry for tenant visibility + sign-off,
//! then returns a cached CallableHandle.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use serde_json::Value;

use super::{CacheKey, CallableHandle, ResolveError};
use crate::artifact_registry::{ArtifactRegistryStore, Contribution};
use crate::tenancy::TenantId;

// ─── FunctionRegistry ───────────────────────────────────────────────────────

/// Holds in-process function closures and a resolution cache.
///
/// Functions are registered programmatically (e.g. at startup from compiled
/// crate code). Resolution validates tenant access via the artifact registry,
/// then hands back a cached `CallableHandle`.
#[derive(Clone)]
pub struct FunctionRegistry {
    /// In-process function closures keyed by function_id.
    functions: Arc<RwLock<HashMap<String, Arc<dyn Fn(Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> + Send + Sync>>>>,

    /// Resolved handle cache: (tenant, function_id, version) → handle.
    cache: Arc<RwLock<HashMap<CacheKey, CallableHandle>>>,

    /// Artifact registry store for tenant visibility + sign-off checks.
    artifact_store: Arc<ArtifactRegistryStore>,
}

impl FunctionRegistry {
    /// Create a new FunctionRegistry backed by the given artifact store.
    pub fn new(artifact_store: Arc<ArtifactRegistryStore>) -> Self {
        Self {
            functions: Arc::new(RwLock::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
            artifact_store,
        }
    }

    // ─── Registration ────────────────────────────────────────────────────

    /// Register an in-process function by its function_id.
    ///
    /// The function_id should match the id used in the artifact registry
    /// (e.g. `"pkg:orders/process_order"`).
    pub async fn register<F>(&self, function_id: impl Into<String>, f: F)
    where
        F: Fn(Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    {
        let mut fns = self.functions.write().await;
        fns.insert(function_id.into(), Arc::new(f));
    }

    /// Unregister a function (e.g. on hot-reload).
    pub async fn unregister(&self, function_id: &str) {
        let mut fns = self.functions.write().await;
        fns.remove(function_id);
        // Also invalidate all cache entries for this function.
        let mut cache = self.cache.write().await;
        cache.retain(|k, _| k.function_id != function_id);
    }

    // ─── Resolution ─────────────────────────────────────────────────────

    /// Resolve a function by id under the given tenant context.
    ///
    /// Steps:
    /// 1. Look up artifact in registry (checks existence)
    /// 2. Check tenant visibility
    /// 3. Check sign-off status
    /// 4. Look up in-process closure
    /// 5. Return cached CallableHandle
    pub async fn resolve(
        &self,
        tenant: &TenantId,
        function_id: &str,
    ) -> Result<CallableHandle, ResolveError> {
        // 1. Look up artifact in the registry (validates existence + tenant visibility).
        let record = self
            .artifact_store
            .resolve_function(tenant.as_str(), function_id)
            .await
            .map_err(|e| match e {
                crate::artifact_registry::RegistryError::NotFound(msg) => {
                    // Distinguish "not visible" vs "not found" from the error message.
                    if msg.contains("not visible") {
                        ResolveError::NotVisible {
                            tenant: tenant.as_str().to_string(),
                            function_id: function_id.to_string(),
                        }
                    } else {
                        ResolveError::NotFound(function_id.to_string())
                    }
                }
                crate::artifact_registry::RegistryError::Storage(msg) => {
                    ResolveError::Internal(msg)
                }
                crate::artifact_registry::RegistryError::InvalidInput(msg) => {
                    ResolveError::Internal(msg)
                }
            })?;

        // 2. Tenant visibility already checked by resolve_function above.

        // 3. Check sign-off status.
        if record.signed_off_by.is_none() {
            return Err(ResolveError::NotSignedOff(function_id.to_string()));
        }

        // 4. Check cache.
        let cache_key = CacheKey {
            tenant_id: tenant.as_str().to_string(),
            function_id: function_id.to_string(),
            version: record.version.clone(),
        };

        {
            let cache = self.cache.read().await;
            if let Some(handle) = cache.get(&cache_key) {
                // Return a clone of the cached handle.
                return Ok(clone_handle(handle));
            }
        }

        // 5. Look up in-process closure.
        let func = {
            let fns = self.functions.read().await;

            // Try exact function_id first, then try contribution name match.
            if let Some(f) = fns.get(function_id) {
                Some(f.clone())
            } else {
                // Try matching by contribution name within the artifact.
                let contrib_name = record
                    .contributions
                    .iter()
                    .find_map(|c| match c {
                        Contribution::BackendFunction { name, .. } => Some(name.as_str()),
                        _ => None,
                    });

                if let Some(name) = contrib_name {
                    fns.get(name).cloned()
                } else {
                    None
                }
            }
        };

        let func = func.ok_or_else(|| {
            ResolveError::NotFound(format!(
                "no in-process handler registered for {function_id}"
            ))
        })?;

        let handle = CallableHandle::InProcess(func);

        // Cache it.
        {
            let mut cache = self.cache.write().await;
            cache.insert(cache_key, clone_handle(&handle));
        }

        Ok(handle)
    }

    // ─── Cache Invalidation ─────────────────────────────────────────────

    /// Invalidate all cache entries for a function (e.g. new version signed off).
    pub async fn invalidate_function(&self, function_id: &str) {
        let mut cache = self.cache.write().await;
        cache.retain(|k, _| k.function_id != function_id);
    }

    /// Invalidate all cache entries for a tenant (e.g. tenant version pin changed).
    pub async fn invalidate_tenant(&self, tenant_id: &str) {
        let mut cache = self.cache.write().await;
        cache.retain(|k, _| k.tenant_id != tenant_id);
    }

    /// Clear the entire cache.
    pub async fn invalidate_all(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Clone a CallableHandle (clones the inner Arc).
fn clone_handle(handle: &CallableHandle) -> CallableHandle {
    match handle {
        CallableHandle::InProcess(f) => CallableHandle::InProcess(f.clone()),
    }
}
