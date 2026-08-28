//! Pluggable auth binding (Model C) — the runtime delegates authentication to
//! a provider that satisfies the `auth_provider` layer contract:
//!
//! ```text
//! authenticate(token: Str) -> AuthResult { authenticated, subject, claims, error }
//! ```
//!
//! The runtime knows nothing about Cognito/Google/JWKS/tenancy at this layer —
//! it holds an [`AuthProviderBinding`] and calls [`AuthProviderBinding::authenticate`].
//! Concrete bindings:
//!
//! - [`LocalJwksProvider`] — the zero-config default. Validates a JWT against a
//!   configured JWKS (the existing [`crate::auth::AuthState`] path). No external
//!   app, no network hop beyond the JWKS fetch.
//! - [`RpcProvider`] — dogfoods runtime app-to-app execution: invokes a VEIL
//!   **auth app** function (via the invoke API / function registry) with the
//!   token and parses the returned `AuthResult` JSON. This is how an operator
//!   points the runtime at `dlx-auth` (or any auth VEIL project).
//! - [`FfiProvider`] — fetches a compiled auth artifact and calls it in-process
//!   (fast path). Structured here; activation depends on the auth artifact
//!   compiling to a loadable library (tracked separately).
//!
//! Selection is config-driven via `VEIL_AUTH_BINDING`:
//! - unset / `local`         → [`LocalJwksProvider`]
//! - `rpc:<function_id>`     → [`RpcProvider`] invoking that function
//! - `ffi:<artifact_id>`     → [`FfiProvider`] loading that artifact
//!
//! Keeping `local` as the default means existing deployments (JWKS auth) keep
//! working with no change.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// The uniform outcome of an authentication attempt — mirrors the
/// `auth_provider` layer's `AuthResult` struct exactly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthResult {
    /// Did the token verify?
    pub authenticated: bool,
    /// Verified subject/principal id (e.g. `sub`) when authenticated.
    #[serde(default)]
    pub subject: Option<String>,
    /// Provider claims as an opaque map — roles, scopes, tenant, custom.
    /// The access-rule evaluator filters on these.
    #[serde(default)]
    pub claims: HashMap<String, Value>,
    /// Human-readable failure reason when `authenticated` is false.
    #[serde(default)]
    pub error: Option<String>,
}

impl AuthResult {
    /// A failed authentication with a reason.
    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            authenticated: false,
            subject: None,
            claims: HashMap::new(),
            error: Some(reason.into()),
        }
    }
}

/// The runtime's view of an auth provider — anything that can turn an opaque
/// token into an [`AuthResult`]. Object-safe + async so bindings can do I/O
/// (JWKS fetch, RPC invoke, artifact load).
#[async_trait::async_trait]
pub trait AuthProviderBinding: Send + Sync {
    /// Authenticate an opaque credential token. Never errors — an invalid
    /// token yields `AuthResult { authenticated: false, error: Some(..) }`.
    async fn authenticate(&self, token: &str) -> AuthResult;

    /// A short label for logging/diagnostics.
    fn kind(&self) -> &'static str;
}

// ─── Local JWKS provider (default) ───────────────────────────────────────────

/// Default binding: validate the JWT against a configured JWKS via the existing
/// [`crate::auth::AuthState`]. Zero-config; no auth app required.
pub struct LocalJwksProvider {
    state: Arc<crate::auth::AuthState>,
}

impl LocalJwksProvider {
    pub fn new(state: Arc<crate::auth::AuthState>) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl AuthProviderBinding for LocalJwksProvider {
    async fn authenticate(&self, token: &str) -> AuthResult {
        if !self.state.can_validate() {
            return AuthResult::denied("auth provider not configured (no JWKS)");
        }
        match self.state.validate_claims(token) {
            Ok(claims) => {
                let subject = claims
                    .get("sub")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                AuthResult {
                    authenticated: true,
                    subject,
                    claims,
                    error: None,
                }
            }
            Err(e) => AuthResult::denied(e.to_string()),
        }
    }

    fn kind(&self) -> &'static str {
        "local-jwks"
    }
}

// ─── RPC provider (dogfoods app-to-app invoke) ───────────────────────────────

/// Delegating binding: invoke a VEIL auth app's `authenticate` function through
/// the runtime's function-invocation substrate, passing `{ "token": <token> }`
/// and parsing the returned `AuthResult` JSON.
///
/// This is the concrete realization of Model C: the runtime executes another
/// VEIL app (e.g. `dlx-auth`) to authenticate its own callers.
pub struct RpcProvider {
    registry: Arc<crate::function_invoke::FunctionRegistry>,
    function_id: String,
}

impl RpcProvider {
    pub fn new(
        registry: Arc<crate::function_invoke::FunctionRegistry>,
        function_id: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            function_id: function_id.into(),
        }
    }

    /// Parse a function-invoke JSON result into an AuthResult. Accepts either a
    /// bare AuthResult object or `{ "result": AuthResult }` (the invoke API
    /// wraps handler output in `result`).
    fn parse_result(value: Value) -> AuthResult {
        let inner = value
            .get("result")
            .cloned()
            .unwrap_or(value);
        serde_json::from_value::<AuthResult>(inner)
            .unwrap_or_else(|e| AuthResult::denied(format!("auth app returned malformed AuthResult: {e}")))
    }
}

#[async_trait::async_trait]
impl AuthProviderBinding for RpcProvider {
    async fn authenticate(&self, token: &str) -> AuthResult {
        // Auth runs before tenant resolution, so invoke under the system tenant.
        let tenant = crate::tenancy::TenantId::new("__system__");
        let handle = match self.registry.resolve(&tenant, &self.function_id).await {
            Ok(h) => h,
            Err(e) => {
                return AuthResult::denied(format!(
                    "auth app '{}' unavailable: {e}",
                    self.function_id
                ))
            }
        };
        match handle.invoke(json!({ "token": token })).await {
            Ok(out) => Self::parse_result(out),
            Err(e) => AuthResult::denied(format!("auth app invocation failed: {e}")),
        }
    }

    fn kind(&self) -> &'static str {
        "rpc"
    }
}

// ─── FFI provider (fetch compiled artifact, call in-process) ─────────────────

/// Delegating binding via FFI: load a compiled auth artifact and call its
/// `authenticate` entry point in-process. Structured for the fast path; the
/// concrete dlsym/loader is wired once auth artifacts compile to a loadable
/// library (the `Jwks` platform stub needs a backing crate first — tracked
/// separately). Until then this reports its unavailability as a denial so the
/// gate fails closed rather than silently allowing.
pub struct FfiProvider {
    artifact_id: String,
}

impl FfiProvider {
    pub fn new(artifact_id: impl Into<String>) -> Self {
        Self {
            artifact_id: artifact_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl AuthProviderBinding for FfiProvider {
    async fn authenticate(&self, _token: &str) -> AuthResult {
        AuthResult::denied(format!(
            "ffi auth binding for artifact '{}' not yet loadable (auth artifact must compile to a loadable library)",
            self.artifact_id
        ))
    }

    fn kind(&self) -> &'static str {
        "ffi"
    }
}

// ─── Binding selection ───────────────────────────────────────────────────────

/// How the runtime binds its auth provider, parsed from `VEIL_AUTH_BINDING`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingSpec {
    /// Local JWKS validation (default).
    Local,
    /// Invoke a VEIL auth app function by id.
    Rpc { function_id: String },
    /// Load and call a compiled auth artifact by id.
    Ffi { artifact_id: String },
}

impl BindingSpec {
    /// Parse from the `VEIL_AUTH_BINDING` env var. Unset/empty/`local` → Local;
    /// `rpc:<id>` → Rpc; `ffi:<id>` → Ffi. Unknown forms fall back to Local
    /// with a warning.
    pub fn from_env() -> Self {
        match std::env::var("VEIL_AUTH_BINDING") {
            Ok(v) => Self::parse(&v),
            Err(_) => BindingSpec::Local,
        }
    }

    pub fn parse(raw: &str) -> Self {
        let s = raw.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("local") {
            return BindingSpec::Local;
        }
        if let Some(id) = s.strip_prefix("rpc:") {
            return BindingSpec::Rpc {
                function_id: id.trim().to_string(),
            };
        }
        if let Some(id) = s.strip_prefix("ffi:") {
            return BindingSpec::Ffi {
                artifact_id: id.trim().to_string(),
            };
        }
        tracing::warn!("unknown VEIL_AUTH_BINDING='{s}'; falling back to local JWKS");
        BindingSpec::Local
    }
}

/// Build the active [`AuthProviderBinding`] from the binding spec, given the
/// runtime's local auth state (for the default) and function registry (for RPC).
pub fn build_binding(
    spec: BindingSpec,
    local_state: Arc<crate::auth::AuthState>,
    registry: Arc<crate::function_invoke::FunctionRegistry>,
) -> Arc<dyn AuthProviderBinding> {
    match spec {
        BindingSpec::Local => Arc::new(LocalJwksProvider::new(local_state)),
        BindingSpec::Rpc { function_id } => Arc::new(RpcProvider::new(registry, function_id)),
        BindingSpec::Ffi { artifact_id } => Arc::new(FfiProvider::new(artifact_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_spec_defaults_to_local() {
        assert_eq!(BindingSpec::parse(""), BindingSpec::Local);
        assert_eq!(BindingSpec::parse("local"), BindingSpec::Local);
        assert_eq!(BindingSpec::parse("LOCAL"), BindingSpec::Local);
    }

    #[test]
    fn binding_spec_parses_rpc() {
        assert_eq!(
            BindingSpec::parse("rpc:dlx-auth/authenticate"),
            BindingSpec::Rpc { function_id: "dlx-auth/authenticate".into() }
        );
    }

    #[test]
    fn binding_spec_parses_ffi() {
        assert_eq!(
            BindingSpec::parse("ffi:dlx-auth"),
            BindingSpec::Ffi { artifact_id: "dlx-auth".into() }
        );
    }

    #[test]
    fn binding_spec_unknown_falls_back_local() {
        assert_eq!(BindingSpec::parse("weird:thing"), BindingSpec::Local);
    }

    #[test]
    fn rpc_parse_result_handles_wrapped_and_bare() {
        // wrapped in { result: ... }
        let wrapped = serde_json::json!({
            "result": { "authenticated": true, "subject": "u1", "claims": {"email":"a@b.com"}, "error": null }
        });
        let r = RpcProvider::parse_result(wrapped);
        assert!(r.authenticated);
        assert_eq!(r.subject.as_deref(), Some("u1"));
        assert_eq!(r.claims.get("email").and_then(|v| v.as_str()), Some("a@b.com"));

        // bare AuthResult
        let bare = serde_json::json!({ "authenticated": false, "error": "bad token" });
        let r = RpcProvider::parse_result(bare);
        assert!(!r.authenticated);
        assert_eq!(r.error.as_deref(), Some("bad token"));
    }

    #[test]
    fn auth_result_denied() {
        let r = AuthResult::denied("nope");
        assert!(!r.authenticated);
        assert_eq!(r.error.as_deref(), Some("nope"));
        assert!(r.claims.is_empty());
    }
}
