//! Phase 3 — Backend Function Resolution and Invocation.
//!
//! The runtime resolves and invokes backend functions by id under tenant context.
//! Functions are registered via the artifact registry with `Contribution::BackendFunction`.
//! Resolution checks tenant visibility + sign-off gate, then returns a `CallableHandle`
//! that can be invoked with JSON args.

mod registry;
#[cfg(test)]
mod tests;

pub use registry::FunctionRegistry;

use serde_json::Value;
use std::sync::Arc;

// ─── CallableHandle ─────────────────────────────────────────────────────────

/// A resolved, ready-to-invoke function reference.
///
/// Start with `InProcess` only. WASM sandboxing and FFI are future additions.
pub enum CallableHandle {
    /// Function compiled into the same process (registered closure).
    InProcess(
        Arc<
            dyn Fn(Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>
                + Send
                + Sync,
        >,
    ),
}

impl CallableHandle {
    /// Invoke this handle with the given JSON arguments.
    pub fn invoke(&self, args: Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            CallableHandle::InProcess(f) => f(args),
        }
    }
}

// Can't derive Debug/Clone for dyn Fn, so manual impls:
impl std::fmt::Debug for CallableHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallableHandle::InProcess(_) => f.write_str("CallableHandle::InProcess(...)"),
        }
    }
}

// ─── ResolveError ───────────────────────────────────────────────────────────

/// Errors arising from function resolution.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ResolveError {
    /// Function id not found in registry.
    #[error("function not found: {0}")]
    NotFound(String),

    /// Function exists but is not visible to this tenant.
    #[error("function not visible to tenant {tenant}: {function_id}")]
    NotVisible {
        tenant: String,
        function_id: String,
    },

    /// Function has not been signed off.
    #[error("function not signed off: {0}")]
    NotSignedOff(String),

    /// Internal error (storage, etc.).
    #[error("internal error: {0}")]
    Internal(String),
}

impl ResolveError {
    /// Map to an HTTP status code.
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            ResolveError::NotFound(_) => StatusCode::NOT_FOUND,
            ResolveError::NotVisible { .. } => StatusCode::FORBIDDEN,
            ResolveError::NotSignedOff(_) => StatusCode::FORBIDDEN,
            ResolveError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

// ─── CacheKey ───────────────────────────────────────────────────────────────

/// Cache key for resolved handles: (tenant_id, function_id, version).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    pub tenant_id: String,
    pub function_id: String,
    pub version: String,
}
