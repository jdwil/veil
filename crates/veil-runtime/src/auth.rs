//! Configurable authentication middleware for the ProductHost.
//!
//! When `VEIL_AUTH_ENABLED=true`, validates JWT tokens from the `Authorization: Bearer <token>`
//! header using Cognito JWKS. When disabled (default), all requests pass through.
//!
//! ## Configuration (env vars)
//! - `VEIL_AUTH_ENABLED` — "true" to enable (default: disabled)
//! - `VEIL_AUTH_PROVIDER` — "cognito" or "none" (default: "none")
//! - `VEIL_AUTH_COGNITO_REGION` — AWS region for the user pool
//! - `VEIL_AUTH_COGNITO_USER_POOL_ID` — Cognito user pool ID
//! - `VEIL_AUTH_COGNITO_CLIENT_ID` — Cognito app client ID (audience)

use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
};
use futures::future::BoxFuture;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

// ─── Config ─────────────────────────────────────────────────────────────────

/// Auth configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub enabled: bool,
    pub provider: AuthProvider,
}

#[derive(Debug, Clone)]
pub enum AuthProvider {
    None,
    Cognito(CognitoConfig),
}

#[derive(Debug, Clone)]
pub struct CognitoConfig {
    pub region: String,
    pub user_pool_id: String,
    // Retained: audience check for the in-flight token-validation path.
    #[allow(dead_code)]
    pub client_id: String,
}

impl AuthConfig {
    /// Load auth configuration from environment variables.
    pub fn from_env() -> Self {
        let enabled = std::env::var("VEIL_AUTH_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        let provider_str = std::env::var("VEIL_AUTH_PROVIDER")
            .unwrap_or_else(|_| "none".into());

        let provider = if !enabled || provider_str.eq_ignore_ascii_case("none") {
            AuthProvider::None
        } else if provider_str.eq_ignore_ascii_case("cognito") {
            let region = std::env::var("VEIL_AUTH_COGNITO_REGION")
                .unwrap_or_else(|_| "us-west-2".into());
            let user_pool_id = std::env::var("VEIL_AUTH_COGNITO_USER_POOL_ID")
                .unwrap_or_default();
            let client_id = std::env::var("VEIL_AUTH_COGNITO_CLIENT_ID")
                .unwrap_or_default();

            if user_pool_id.is_empty() || client_id.is_empty() {
                tracing::warn!(
                    "VEIL_AUTH_ENABLED=true with cognito provider but missing \
                     VEIL_AUTH_COGNITO_USER_POOL_ID or VEIL_AUTH_COGNITO_CLIENT_ID; auth disabled"
                );
                AuthProvider::None
            } else {
                AuthProvider::Cognito(CognitoConfig {
                    region,
                    user_pool_id,
                    client_id,
                })
            }
        } else {
            tracing::warn!("Unknown VEIL_AUTH_PROVIDER={provider_str}; auth disabled");
            AuthProvider::None
        };

        Self { enabled, provider }
    }

    /// Whether auth is actually active (enabled AND has a valid provider).
    // Retained: only reached via the in-flight AuthState::new token-validation
    // path (the active path is new_for_claims).
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.enabled && !matches!(self.provider, AuthProvider::None)
    }

    /// Build a config carrying a Cognito provider whenever the Cognito env vars
    /// are present, **independent of `VEIL_AUTH_ENABLED`**. Used for claim-based
    /// access control (contribution filtering), which needs to validate tokens
    /// and read their claims even when the coarse `/api/*` gate is off. Returns
    /// a `None` provider if pool/client are not configured.
    pub fn cognito_from_env() -> Self {
        let region = std::env::var("VEIL_AUTH_COGNITO_REGION")
            .unwrap_or_else(|_| "us-west-2".into());
        let user_pool_id = std::env::var("VEIL_AUTH_COGNITO_USER_POOL_ID").unwrap_or_default();
        let client_id = std::env::var("VEIL_AUTH_COGNITO_CLIENT_ID").unwrap_or_default();

        let provider = if user_pool_id.is_empty() {
            AuthProvider::None
        } else {
            AuthProvider::Cognito(CognitoConfig {
                region,
                user_pool_id,
                client_id,
            })
        };

        Self {
            // `enabled` is irrelevant for the claims-extraction path; the flag
            // only governs the coarse AuthLayer.
            enabled: false,
            provider,
        }
    }

    /// Human-readable provider name for logging.
    // Retained: logging helper for the in-flight token-validation path.
    #[allow(dead_code)]
    pub fn provider_name(&self) -> &str {
        match &self.provider {
            AuthProvider::None => "none",
            AuthProvider::Cognito(_) => "cognito",
        }
    }
}

// ─── JWKS / Key Set ─────────────────────────────────────────────────────────

/// Cached JWKS key set for token validation.
#[derive(Clone)]
pub struct JwksKeySet {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Clone, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKeyRaw>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct JwkKeyRaw {
    kid: String,
    kty: String,
    alg: Option<String>,
    n: String,
    e: String,
    #[serde(rename = "use")]
    use_field: Option<String>,
}

#[derive(Clone)]
struct JwkKey {
    kid: String,
    decoding_key: DecodingKey,
}

impl JwksKeySet {
    /// Fetch JWKS from Cognito well-known endpoint.
    pub async fn fetch_cognito(region: &str, user_pool_id: &str) -> Result<Self, String> {
        let url = format!(
            "https://cognito-idp.{region}.amazonaws.com/{user_pool_id}/.well-known/jwks.json"
        );
        let resp: JwksResponse = reqwest::Client::new()
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("JWKS fetch failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("JWKS parse failed: {e}"))?;

        let mut keys = Vec::new();
        for raw in resp.keys {
            if raw.kty != "RSA" {
                continue;
            }
            if let Ok(dk) = DecodingKey::from_rsa_components(&raw.n, &raw.e) {
                keys.push(JwkKey {
                    kid: raw.kid,
                    decoding_key: dk,
                });
            }
        }

        if keys.is_empty() {
            return Err("No valid RSA keys found in JWKS".into());
        }

        Ok(Self { keys })
    }

    /// Find the decoding key by kid.
    pub fn find_key(&self, kid: &str) -> Option<&DecodingKey> {
        self.keys
            .iter()
            .find(|k| k.kid == kid)
            .map(|k| &k.decoding_key)
    }
}

// ─── Claims ─────────────────────────────────────────────────────────────────

/// Standard claims from a Cognito ID token.
// Retained: scaffolding for the in-flight auth subsystem (JWT claim
// extraction). Not yet constructed while token validation is being wired.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize)]
pub struct CognitoClaims {
    pub sub: String,
    pub email: Option<String>,
    #[serde(rename = "cognito:username")]
    pub username: Option<String>,
    pub aud: Option<String>,
    pub iss: Option<String>,
    pub token_use: Option<String>,
    pub exp: Option<u64>,
    pub iat: Option<u64>,
    /// Custom claims (e.g. groups) are flattened into this.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

// ─── Validator ──────────────────────────────────────────────────────────────

/// Shared auth state: JWKS keys + configuration.
#[derive(Clone)]
pub struct AuthState {
    config: AuthConfig,
    jwks: Option<Arc<JwksKeySet>>,
}

impl AuthState {
    /// Create a new AuthState. If auth is active, fetches JWKS eagerly.
    // Retained: in-flight coarse-gate token-validation entry point (active
    // code uses new_for_claims for claim-based filtering).
    #[allow(dead_code)]
    pub async fn new(config: AuthConfig) -> Self {
        let jwks = if config.is_active() {
            Self::fetch_jwks(&config.provider).await
        } else {
            None
        };

        Self { config, jwks }
    }

    /// Create an AuthState for **claim extraction only** (claim-based access
    /// control on contribution listing). Unlike [`Self::new`], this fetches the
    /// JWKS whenever a Cognito provider is configured, regardless of the coarse
    /// `VEIL_AUTH_ENABLED` gate. This decouples "can I validate a token to read
    /// its claims for filtering" from "do I reject unauthenticated /api/*
    /// requests" (the latter is the [`AuthLayer`]'s job). Config comes from the
    /// same `VEIL_AUTH_COGNITO_*` env vars — no hardcoding.
    pub async fn new_for_claims(config: AuthConfig) -> Self {
        let jwks = Self::fetch_jwks(&config.provider).await;
        Self { config, jwks }
    }

    /// Fetch the JWKS key set for a provider, if any.
    async fn fetch_jwks(provider: &AuthProvider) -> Option<Arc<JwksKeySet>> {
        match provider {
            AuthProvider::Cognito(cfg) => {
                match JwksKeySet::fetch_cognito(&cfg.region, &cfg.user_pool_id).await {
                    Ok(ks) => {
                        tracing::info!(
                            keys = ks.keys.len(),
                            pool = %cfg.user_pool_id,
                            "JWKS loaded for Cognito auth"
                        );
                        Some(Arc::new(ks))
                    }
                    Err(e) => {
                        tracing::error!("Failed to load JWKS: {e}; token validation disabled");
                        None
                    }
                }
            }
            AuthProvider::None => None,
        }
    }

    /// Validate a bearer token. Returns claims on success.
    // Retained: in-flight JWT validation for the coarse /api/* auth gate.
    #[allow(dead_code)]
    pub fn validate_token(&self, token: &str) -> Result<CognitoClaims, AuthError> {
        let jwks = self.jwks.as_ref().ok_or(AuthError::NotConfigured)?;

        let header = decode_header(token).map_err(|e| AuthError::InvalidToken(e.to_string()))?;
        let kid = header.kid.ok_or(AuthError::InvalidToken("missing kid".into()))?;

        let key = jwks
            .find_key(&kid)
            .ok_or(AuthError::InvalidToken(format!("unknown kid: {kid}")))?;

        let mut validation = Validation::new(Algorithm::RS256);
        // Set audience if we have a client_id.
        if let AuthProvider::Cognito(cfg) = &self.config.provider {
            validation.set_audience(&[&cfg.client_id]);
            let issuer = format!(
                "https://cognito-idp.{}.amazonaws.com/{}",
                cfg.region, cfg.user_pool_id
            );
            validation.set_issuer(&[&issuer]);
        }
        // Accept both id_token and access_token.
        validation.validate_exp = true;

        let token_data =
            decode::<CognitoClaims>(token, key, &validation)
                .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims)
    }

    /// Validate a bearer token for the purpose of *claim extraction* and return
    /// the full set of claims as a flat JSON map (claim name → value).
    ///
    /// This is used by claim-based access control (`crate::access`). It performs
    /// the same cryptographic validation as [`Self::validate_token`] — signature
    /// against JWKS, issuer, and expiry — but does NOT require a specific
    /// audience. Cognito **access tokens** omit the `aud` claim (they carry
    /// `client_id` instead), so requiring `aud` here would reject otherwise
    /// valid tokens. Signature + issuer + expiry are the security-relevant
    /// checks; audience is an authorization scope the coarse [`AuthLayer`]
    /// already enforces where required.
    ///
    /// Returns a map suitable for `crate::access::AccessRule::evaluate`.
    pub fn validate_claims(
        &self,
        token: &str,
    ) -> Result<std::collections::HashMap<String, serde_json::Value>, AuthError> {
        let jwks = self.jwks.as_ref().ok_or(AuthError::NotConfigured)?;

        let header = decode_header(token).map_err(|e| AuthError::InvalidToken(e.to_string()))?;
        let kid = header.kid.ok_or(AuthError::InvalidToken("missing kid".into()))?;
        let key = jwks
            .find_key(&kid)
            .ok_or(AuthError::InvalidToken(format!("unknown kid: {kid}")))?;

        let mut validation = Validation::new(Algorithm::RS256);
        // Validate issuer + expiry, but not audience (see doc comment).
        validation.validate_aud = false;
        validation.validate_exp = true;
        if let AuthProvider::Cognito(cfg) = &self.config.provider {
            let issuer = format!(
                "https://cognito-idp.{}.amazonaws.com/{}",
                cfg.region, cfg.user_pool_id
            );
            validation.set_issuer(&[&issuer]);
        }

        // Decode into a generic JSON object so every claim (standard + custom)
        // is available to the access evaluator.
        let token_data = decode::<serde_json::Map<String, serde_json::Value>>(token, key, &validation)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims.into_iter().collect())
    }

    /// Whether this AuthState has a usable key set (i.e. can validate tokens).
    pub fn can_validate(&self) -> bool {
        self.jwks.is_some()
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AuthError {
    #[error("auth not configured")]
    NotConfigured,
    #[error("missing authorization header")]
    MissingHeader,
    #[error("invalid token: {0}")]
    InvalidToken(String),
}

// ─── Middleware Layer ────────────────────────────────────────────────────────

/// Tower layer that enforces auth on requests to `/api/*` paths, delegating
/// validation to a pluggable [`crate::auth_provider::AuthProviderBinding`].
/// Skips auth for `/health`, static files, and all non-API paths.
///
/// The `enabled` flag corresponds to the operator's coarse `VEIL_AUTH_ENABLED`
/// switch: when off, everything passes through (local dev). When on, `/api/*`
/// requires a token that the bound provider accepts.
#[derive(Clone)]
pub struct AuthLayer {
    binding: Arc<dyn crate::auth_provider::AuthProviderBinding>,
    enabled: bool,
}

impl AuthLayer {
    pub fn new(
        binding: Arc<dyn crate::auth_provider::AuthProviderBinding>,
        enabled: bool,
    ) -> Self {
        Self { binding, enabled }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            binding: self.binding.clone(),
            enabled: self.enabled,
        }
    }
}

/// The middleware service that checks auth on API routes.
#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    binding: Arc<dyn crate::auth_provider::AuthProviderBinding>,
    enabled: bool,
}

impl<S> Service<Request<Body>> for AuthMiddleware<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // If auth is not enabled, pass through everything.
        if !self.enabled {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }

        let path = req.uri().path().to_string();

        // Skip auth for non-API paths: health, static assets, SPA routes.
        let requires_auth = path.starts_with("/api/");

        if !requires_auth {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }

        // Extract bearer token.
        let auth_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let token = auth_header.as_deref().and_then(|h| {
            h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))
        }).map(|s| s.to_string());

        match token {
            Some(t) => {
                let mut inner = self.inner.clone();
                let binding = self.binding.clone();
                Box::pin(async move {
                    let result = binding.authenticate(&t).await;
                    if result.authenticated {
                        inner.call(req).await
                    } else {
                        let msg = result.error.unwrap_or_else(|| "invalid token".into());
                        tracing::debug!(error = %msg, path = %path, provider = binding.kind(), "auth rejected");
                        Ok(Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .header("content-type", "application/json")
                            .body(Body::from(
                                serde_json::json!({
                                    "error": "unauthorized",
                                    "message": msg,
                                })
                                .to_string(),
                            ))
                            .unwrap())
                    }
                })
            }
            None => {
                tracing::debug!(path = %path, "auth missing bearer token");
                Box::pin(async move {
                    Ok(Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .header("content-type", "application/json")
                        .header("www-authenticate", "Bearer")
                        .body(Body::from(
                            serde_json::json!({
                                "error": "unauthorized",
                                "message": "missing authorization header",
                            })
                            .to_string(),
                        ))
                        .unwrap())
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // These tests mutate process-global env vars (`VEIL_AUTH_*`). Rust runs
    // tests in parallel by default, so without serialization one test's
    // `remove_var` races another's `set_var`, producing flaky failures
    // (e.g. `assertion failed: config.enabled`). Serialize them on a shared
    // lock. `.unwrap_or_else(|e| e.into_inner())` tolerates a poisoned lock so
    // a panic in one test doesn't cascade into the others.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn auth_config_defaults_to_disabled() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Clear any env vars that might be set.
        unsafe {
            std::env::remove_var("VEIL_AUTH_ENABLED");
            std::env::remove_var("VEIL_AUTH_PROVIDER");
        }
        let config = AuthConfig::from_env();
        assert!(!config.enabled);
        assert!(!config.is_active());
    }

    #[test]
    fn auth_config_enabled_without_pool_is_inactive() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("VEIL_AUTH_ENABLED", "true");
            std::env::set_var("VEIL_AUTH_PROVIDER", "cognito");
            std::env::remove_var("VEIL_AUTH_COGNITO_USER_POOL_ID");
            std::env::remove_var("VEIL_AUTH_COGNITO_CLIENT_ID");
        }
        let config = AuthConfig::from_env();
        assert!(config.enabled);
        // Provider falls back to None because pool ID is empty.
        assert!(!config.is_active());
        // Clean up
        unsafe {
            std::env::remove_var("VEIL_AUTH_ENABLED");
            std::env::remove_var("VEIL_AUTH_PROVIDER");
        }
    }

    #[test]
    fn auth_config_fully_configured() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("VEIL_AUTH_ENABLED", "true");
            std::env::set_var("VEIL_AUTH_PROVIDER", "cognito");
            std::env::set_var("VEIL_AUTH_COGNITO_REGION", "us-east-1");
            std::env::set_var("VEIL_AUTH_COGNITO_USER_POOL_ID", "us-east-1_TestPool");
            std::env::set_var("VEIL_AUTH_COGNITO_CLIENT_ID", "test-client-id");
        }
        let config = AuthConfig::from_env();
        assert!(config.enabled);
        assert!(config.is_active());
        match &config.provider {
            AuthProvider::Cognito(cfg) => {
                assert_eq!(cfg.region, "us-east-1");
                assert_eq!(cfg.user_pool_id, "us-east-1_TestPool");
                assert_eq!(cfg.client_id, "test-client-id");
            }
            _ => panic!("expected Cognito provider"),
        }
        // Clean up
        unsafe {
            std::env::remove_var("VEIL_AUTH_ENABLED");
            std::env::remove_var("VEIL_AUTH_PROVIDER");
            std::env::remove_var("VEIL_AUTH_COGNITO_REGION");
            std::env::remove_var("VEIL_AUTH_COGNITO_USER_POOL_ID");
            std::env::remove_var("VEIL_AUTH_COGNITO_CLIENT_ID");
        }
    }
}
