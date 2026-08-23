//! Axum middleware/layer for tenant resolution.
//!
//! Inserts a resolved [`TenantId`] into the request extensions so downstream
//! handlers can extract it via `Extension<TenantId>` or a custom extractor.

use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use tower::{Layer, Service};

use super::{RequestContext, TenantId, TenantResolver};
use veil_shared::Principal;

// ─── Layer ──────────────────────────────────────────────────────────────────

/// Tower [`Layer`] that wraps a service with tenant resolution.
///
/// Extracts `Principal` from request extensions (must be inserted by an
/// upstream auth layer), runs the configured resolver, and inserts `TenantId`
/// into request extensions for downstream handlers.
///
/// # Usage
/// ```ignore
/// let resolver = config.build_resolver(ddb_client).await?;
/// let app = Router::new()
///     .route("/api/data", get(handler))
///     .layer(TenantResolutionLayer::new(resolver));
/// ```
#[derive(Clone)]
pub struct TenantResolutionLayer {
    resolver: Arc<dyn TenantResolver>,
}

impl TenantResolutionLayer {
    pub fn new(resolver: Arc<dyn TenantResolver>) -> Self {
        Self { resolver }
    }
}

impl<S> Layer<S> for TenantResolutionLayer {
    type Service = TenantResolutionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TenantResolutionService {
            inner,
            resolver: self.resolver.clone(),
        }
    }
}

// ─── Service ────────────────────────────────────────────────────────────────

/// The [`Service`] that performs tenant resolution on each request.
#[derive(Clone)]
pub struct TenantResolutionService<S> {
    inner: S,
    resolver: Arc<dyn TenantResolver>,
}

impl<S> Service<Request> for TenantResolutionService<S>
where
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let resolver = self.resolver.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Build RequestContext from the incoming request.
            let request_ctx = build_request_context(&req);

            // Get Principal from extensions (inserted by auth middleware).
            // If no Principal is present, use a default anonymous one.
            let principal = req
                .extensions()
                .get::<Principal>()
                .cloned()
                .unwrap_or_else(|| Principal {
                    id: "anonymous".into(),
                    roles: vec![],
                    claims: std::collections::HashMap::new(),
                });

            // Run the resolver.
            match resolver.resolve(&principal, &request_ctx).await {
                Ok(tenant_id) => {
                    tracing::debug!(
                        tenant = %tenant_id,
                        principal = %principal.id,
                        "tenant resolved"
                    );
                    req.extensions_mut().insert(tenant_id);
                    inner.call(req).await
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        principal = %principal.id,
                        path = %request_ctx.path,
                        "tenant resolution failed"
                    );
                    let response = resolution_error_response(&e);
                    Ok(response)
                }
            }
        })
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Extract request context from an axum Request.
fn build_request_context(req: &Request) -> RequestContext {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_lowercase(), v.to_string()))
        })
        .collect();

    let path = req.uri().path().to_string();

    RequestContext {
        host,
        headers,
        path,
    }
}

/// Map resolution errors to HTTP responses.
fn resolution_error_response(err: &super::ResolutionError) -> Response {
    use super::ResolutionError;

    let (status, message) = match err {
        ResolutionError::NotFound(_) | ResolutionError::MissingClaim(_) => {
            (StatusCode::FORBIDDEN, "tenant could not be resolved")
        }
        ResolutionError::LookupFailed(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "tenant lookup service unavailable")
        }
        ResolutionError::Internal(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error during tenant resolution")
        }
    };

    let body = serde_json::json!({
        "error": message,
        "code": status.as_u16(),
    });

    (status, axum::Json(body)).into_response()
}

// ─── Extractor (convenience) ────────────────────────────────────────────────

/// Axum extractor that pulls TenantId from request extensions.
///
/// Returns 403 if no TenantId was resolved (middleware not in the chain, or
/// resolution failed before reaching this handler — which shouldn't happen
/// if the layer is installed).
///
/// # Usage
/// ```ignore
/// async fn handler(tenant: ResolvedTenant) -> impl IntoResponse {
///     format!("Hello, tenant {}", tenant.0)
/// }
/// ```
pub struct ResolvedTenant(pub TenantId);

impl<S> axum::extract::FromRequestParts<S> for ResolvedTenant
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, axum::Json<serde_json::Value>);

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantId>()
            .cloned()
            .map(ResolvedTenant)
            .ok_or_else(|| {
                let body = serde_json::json!({
                    "error": "tenant not resolved",
                    "code": 403,
                });
                (StatusCode::FORBIDDEN, axum::Json(body))
            })
    }
}
