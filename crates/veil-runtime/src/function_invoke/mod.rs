//! Phase 3 — Backend Function Resolution and Invocation.
//!
//! The runtime resolves and invokes backend functions by id under tenant context.
//! Functions are registered via the artifact registry with `Contribution::BackendFunction`.
//! Resolution checks tenant visibility + sign-off gate, then returns a `CallableHandle`
//! that can be invoked with JSON args.
//!
//! A `CallableHandle` is either an `InProcess` closure (registered at startup or
//! by tests) or a `Lambda` target (a deployed VEIL app). The Lambda path is the
//! generic production substrate for app-to-app execution: any signed-off backend
//! function whose registration carries a Lambda target (`invoke_kind = lambda`,
//! `function_name`) resolves to `CallableHandle::Lambda` and is invoked over the
//! AWS Lambda API. `rpc:dlx-auth` is the first consumer.

mod registry;
#[cfg(test)]
mod tests;

pub use registry::FunctionRegistry;

use serde_json::Value;
use std::sync::Arc;

/// Boxed, thread-safe error used across the invoke substrate.
pub type BoxErr = Box<dyn std::error::Error + Send + Sync>;

// ─── CallableHandle ─────────────────────────────────────────────────────────

/// A resolved, ready-to-invoke function reference.
///
/// - `InProcess` — a closure compiled into this process (registered at startup
///   or by tests).
/// - `Lambda` — a deployed VEIL app running as an AWS Lambda. This is the
///   production path for app-to-app execution: the runtime invokes another
///   VEIL app's function by name/ARN. Generic — any signed-off backend
///   function whose registration carries a Lambda target resolves to this.
///
/// WASM sandboxing and FFI are future additions.
pub enum CallableHandle {
    /// Function compiled into the same process (registered closure).
    InProcess(Arc<dyn Fn(Value) -> Result<Value, BoxErr> + Send + Sync>),
    /// A deployed Lambda invoked by function name (or ARN).
    Lambda {
        /// Lambda function name or ARN to invoke.
        function_name: String,
        /// Shared Lambda client (cheap to clone — wraps an Arc internally).
        client: aws_sdk_lambda::Client,
    },
}

impl CallableHandle {
    /// Invoke this handle with the given JSON arguments.
    ///
    /// For `InProcess`, the registered closure runs synchronously (awaited
    /// trivially). For `Lambda`, the args are serialized to JSON, sent as the
    /// invoke payload, and the response payload is deserialized back to JSON.
    /// A Lambda **function error** (the `FunctionError` field in the response)
    /// is surfaced as an `Err`, so the caller (e.g. the auth gate) fails closed.
    pub async fn invoke(&self, args: Value) -> Result<Value, BoxErr> {
        match self {
            CallableHandle::InProcess(f) => f(args),
            CallableHandle::Lambda {
                function_name,
                client,
            } => invoke_lambda(client, function_name, args).await,
        }
    }
}

/// Invoke a deployed Lambda with JSON args and return the JSON response.
async fn invoke_lambda(
    client: &aws_sdk_lambda::Client,
    function_name: &str,
    args: Value,
) -> Result<Value, BoxErr> {
    use aws_sdk_lambda::primitives::Blob;

    let payload = serde_json::to_vec(&args)
        .map_err(|e| -> BoxErr { format!("serialize invoke payload: {e}").into() })?;

    let resp = client
        .invoke()
        .function_name(function_name)
        .payload(Blob::new(payload))
        .send()
        .await
        .map_err(|e| -> BoxErr {
            format!("lambda invoke '{function_name}' failed: {e}").into()
        })?;

    // The response payload is the function's return value (or its error object
    // when FunctionError is set).
    let out_bytes = resp
        .payload()
        .map(|b| b.as_ref().to_vec())
        .unwrap_or_default();

    // A non-empty FunctionError means the function itself raised — fail closed.
    if let Some(func_err) = resp.function_error() {
        let detail = String::from_utf8_lossy(&out_bytes);
        return Err(format!(
            "lambda '{function_name}' returned FunctionError ({func_err}): {detail}"
        )
        .into());
    }

    if out_bytes.is_empty() {
        return Ok(Value::Null);
    }

    serde_json::from_slice::<Value>(&out_bytes).map_err(|e| -> BoxErr {
        let detail = String::from_utf8_lossy(&out_bytes);
        format!("lambda '{function_name}' returned non-JSON payload ({e}): {detail}").into()
    })
}

// Can't derive Debug/Clone for dyn Fn, so manual impls:
impl std::fmt::Debug for CallableHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallableHandle::InProcess(_) => f.write_str("CallableHandle::InProcess(...)"),
            CallableHandle::Lambda { function_name, .. } => f
                .debug_struct("CallableHandle::Lambda")
                .field("function_name", function_name)
                .finish(),
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
