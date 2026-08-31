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

    /// Lambda client for resolving `invoke_kind = lambda` functions to a
    /// [`CallableHandle::Lambda`]. `None` in pure unit tests that never resolve
    /// a Lambda-backed function; production builds it from env.
    lambda_client: Option<aws_sdk_lambda::Client>,

    /// LRU of dlopen'd workflow cdylibs, keyed by content hash. Hot workflows
    /// stay resident so warm invokes skip dlopen + S3 fetch.
    ffi_cache: super::FfiLibraryCache,

    /// Local directory the `.so` artifacts are fetched into before dlopen.
    ffi_cache_dir: std::path::PathBuf,
}

impl FunctionRegistry {
    /// Create a new FunctionRegistry backed by the given artifact store.
    ///
    /// No Lambda client is attached; resolving a `invoke_kind = lambda`
    /// function returns [`ResolveError::Internal`]. Use [`Self::with_lambda`]
    /// or [`Self::from_env`] for the production path.
    pub fn new(artifact_store: Arc<ArtifactRegistryStore>) -> Self {
        Self {
            functions: Arc::new(RwLock::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
            artifact_store,
            lambda_client: None,
            ffi_cache: super::FfiLibraryCache::new(default_ffi_cache_capacity()),
            ffi_cache_dir: default_ffi_cache_dir(),
        }
    }

    /// Create a FunctionRegistry with a Lambda client attached, enabling
    /// resolution of Lambda-backed backend functions (the production path).
    pub fn with_lambda(
        artifact_store: Arc<ArtifactRegistryStore>,
        lambda_client: aws_sdk_lambda::Client,
    ) -> Self {
        Self {
            functions: Arc::new(RwLock::new(HashMap::new())),
            cache: Arc::new(RwLock::new(HashMap::new())),
            artifact_store,
            lambda_client: Some(lambda_client),
            ffi_cache: super::FfiLibraryCache::new(default_ffi_cache_capacity()),
            ffi_cache_dir: default_ffi_cache_dir(),
        }
    }

    /// Build a FunctionRegistry from ambient AWS config, wiring a Lambda client
    /// so Lambda-backed functions resolve. Used at runtime startup.
    pub async fn from_env(artifact_store: Arc<ArtifactRegistryStore>) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let lambda_client = aws_sdk_lambda::Client::new(&config);
        Self::with_lambda(artifact_store, lambda_client)
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

        // 5. Find the BackendFunction contribution to learn how to invoke it.
        let backend_fn = record.contributions.iter().find_map(|c| match c {
            Contribution::BackendFunction {
                name,
                invoke_kind,
                function_name,
                ..
            } => Some((name.clone(), invoke_kind.clone(), function_name.clone())),
            _ => None,
        });

        let handle = match backend_fn {
            // 5a. Lambda-backed function → construct a Lambda handle.
            Some((_, crate::artifact_registry::InvokeKind::Lambda, Some(fn_name))) => {
                let client = self.lambda_client.clone().ok_or_else(|| {
                    ResolveError::Internal(format!(
                        "function '{function_id}' is lambda-backed but no Lambda client is configured"
                    ))
                })?;
                CallableHandle::Lambda {
                    function_name: fn_name,
                    client,
                }
            }
            // Lambda kind but missing target is a registration error.
            Some((_, crate::artifact_registry::InvokeKind::Lambda, None)) => {
                return Err(ResolveError::Internal(format!(
                    "function '{function_id}' registered as lambda but has no function_name"
                )));
            }
            // 5b. In-process function → look up the registered closure.
            Some((contrib_name, crate::artifact_registry::InvokeKind::InProcess, _)) => {
                let func = {
                    let fns = self.functions.read().await;
                    fns.get(function_id)
                        .or_else(|| fns.get(&contrib_name))
                        .cloned()
                };
                let func = func.ok_or_else(|| {
                    ResolveError::NotFound(format!(
                        "no in-process handler registered for {function_id}"
                    ))
                })?;
                CallableHandle::InProcess(func)
            }
            // 5c. FFI cdylib workflow → fetch the .so by content hash and dlopen.
            Some((_, crate::artifact_registry::InvokeKind::Ffi, _)) => {
                self.resolve_ffi(function_id, &record).await?
            }
            // No BackendFunction contribution → fall back to closure by id.
            None => {
                let func = {
                    let fns = self.functions.read().await;
                    fns.get(function_id).cloned()
                };
                let func = func.ok_or_else(|| {
                    ResolveError::NotFound(format!(
                        "no in-process handler registered for {function_id}"
                    ))
                })?;
                CallableHandle::InProcess(func)
            }
        };

        // Cache it.
        {
            let mut cache = self.cache.write().await;
            cache.insert(cache_key, clone_handle(&handle));
        }

        Ok(handle)
    }

    // ─── FFI cdylib resolution ──────────────────────────────────────────

    /// Resolve a cdylib-backed workflow to a [`CallableHandle::Ffi`].
    ///
    /// Fetches the `.so` from the artifact store (by content hash / blob key)
    /// into the local cache dir on a miss, `dlopen`s it, and keeps the loaded
    /// library resident in the LRU. The content hash pins the exact artifact so
    /// the on-disk file is immutable and safe to reuse.
    async fn resolve_ffi(
        &self,
        function_id: &str,
        record: &crate::artifact_registry::ArtifactRecord,
    ) -> Result<CallableHandle, ResolveError> {
        let hash = record.content_hash.clone().ok_or_else(|| {
            ResolveError::Internal(format!(
                "function '{function_id}' is ffi-backed but has no content_hash"
            ))
        })?;

        // Fast path: already resident.
        if let Some(lib) = self.ffi_cache.get(&hash) {
            return Ok(CallableHandle::Ffi(lib));
        }

        // The blob key defaults to the spec layout `artifacts/{id}/{version}/lib.so`.
        let blob_key = record.blob_key.clone().unwrap_or_else(|| {
            format!("artifacts/{}/{}/lib.so", record.id, record.version)
        });

        // Fetch into a content-addressed local file (immutable once written).
        let so_path = self.ffi_cache_dir.join(format!("{hash}.so"));
        if !so_path.exists() {
            let bytes = self
                .artifact_store
                .get_blob(&blob_key)
                .await
                .map_err(|e| ResolveError::Internal(format!("fetch .so '{blob_key}': {e}")))?;
            if let Some(parent) = so_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ResolveError::Internal(format!("create ffi cache dir: {e}"))
                })?;
            }
            // Write to a temp file then rename so a concurrent resolver never
            // dlopens a partially written .so.
            let tmp = so_path.with_extension("so.partial");
            tokio::fs::write(&tmp, &bytes)
                .await
                .map_err(|e| ResolveError::Internal(format!("write .so: {e}")))?;
            tokio::fs::rename(&tmp, &so_path)
                .await
                .map_err(|e| ResolveError::Internal(format!("rename .so: {e}")))?;
        }

        // dlopen off the async runtime (blocking + runs library initializers).
        let load_path = so_path.clone();
        let load_hash = hash.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            super::LoadedWorkflow::load(&load_path, load_hash)
        })
        .await
        .map_err(|e| ResolveError::Internal(format!("dlopen join error: {e}")))?
        .map_err(|e| ResolveError::Internal(format!("dlopen workflow: {e}")))?;

        let lib = self.ffi_cache.insert(hash, Arc::new(loaded));
        Ok(CallableHandle::Ffi(lib))
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

/// Default resident-library capacity for the FFI LRU. Override via
/// `VEIL_FFI_CACHE_CAPACITY`.
fn default_ffi_cache_capacity() -> usize {
    std::env::var("VEIL_FFI_CACHE_CAPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32)
}

/// Default local directory `.so` artifacts are fetched into before dlopen.
/// Override via `VEIL_FFI_CACHE_DIR`; defaults to a subdir of the system temp.
fn default_ffi_cache_dir() -> std::path::PathBuf {
    std::env::var("VEIL_FFI_CACHE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("veil-ffi-artifacts"))
}

/// Clone a CallableHandle (clones the inner Arc).
fn clone_handle(handle: &CallableHandle) -> CallableHandle {
    match handle {
        CallableHandle::InProcess(f) => CallableHandle::InProcess(f.clone()),
        CallableHandle::Lambda {
            function_name,
            client,
        } => CallableHandle::Lambda {
            function_name: function_name.clone(),
            client: client.clone(),
        },
        CallableHandle::Ffi(lib) => CallableHandle::Ffi(lib.clone()),
    }
}
