//! Tests for the tenant resolution framework.

use std::sync::Arc;

use super::*;

/// Helper: build a Principal with claims.
fn principal(id: &str, claims: &[(&str, &str)]) -> veil_shared::Principal {
    veil_shared::Principal {
        id: id.into(),
        roles: vec![],
        claims: claims.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

/// Helper: build a RequestContext.
fn req_ctx(host: Option<&str>, headers: &[(&str, &str)], path: &str) -> RequestContext {
    RequestContext {
        host: host.map(|s| s.to_string()),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.to_string()))
            .collect(),
        path: path.to_string(),
    }
}

// ─── ClaimResolver Tests ────────────────────────────────────────────────────

#[tokio::test]
async fn claim_resolver_extracts_tenant_from_claims() {
    let resolver = ClaimResolver::new("org_id");
    let p = principal("user-1", &[("org_id", "acme")]);
    let ctx = req_ctx(None, &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("acme"));
}

#[tokio::test]
async fn claim_resolver_returns_error_on_missing_claim() {
    let resolver = ClaimResolver::new("org_id");
    let p = principal("user-1", &[("email", "user@example.com")]);
    let ctx = req_ctx(None, &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::MissingClaim(_))));
}

#[tokio::test]
async fn claim_resolver_returns_error_on_empty_claim() {
    let resolver = ClaimResolver::new("org_id");
    let p = principal("user-1", &[("org_id", "")]);
    let ctx = req_ctx(None, &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::MissingClaim(_))));
}

#[tokio::test]
async fn claim_resolver_uses_configured_field() {
    let resolver = ClaimResolver::new("company");
    let p = principal("user-1", &[("company", "widgets-inc")]);
    let ctx = req_ctx(None, &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("widgets-inc"));
}

// ─── SubdomainResolver Tests ────────────────────────────────────────────────

#[tokio::test]
async fn subdomain_resolver_extracts_first_label() {
    let resolver = SubdomainResolver::new("app.example.com");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(Some("acme.app.example.com"), &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("acme"));
}

#[tokio::test]
async fn subdomain_resolver_lowercases_tenant() {
    let resolver = SubdomainResolver::new("app.example.com");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(Some("ACME.app.example.com"), &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("acme"));
}

#[tokio::test]
async fn subdomain_resolver_strips_port() {
    let resolver = SubdomainResolver::new("app.example.com");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(Some("acme.app.example.com:8080"), &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("acme"));
}

#[tokio::test]
async fn subdomain_resolver_fails_when_no_subdomain() {
    let resolver = SubdomainResolver::new("app.example.com");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(Some("app.example.com"), &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::NotFound(_))));
}

#[tokio::test]
async fn subdomain_resolver_fails_when_wrong_base_domain() {
    let resolver = SubdomainResolver::new("app.example.com");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(Some("acme.other.domain.com"), &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::NotFound(_))));
}

#[tokio::test]
async fn subdomain_resolver_fails_with_no_host() {
    let resolver = SubdomainResolver::new("app.example.com");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::NotFound(_))));
}

// ─── HeaderResolver Tests ───────────────────────────────────────────────────

#[tokio::test]
async fn header_resolver_extracts_tenant_from_default_header() {
    let resolver = HeaderResolver::default();
    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[("x-tenant-id", "acme")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("acme"));
}

#[tokio::test]
async fn header_resolver_extracts_tenant_from_custom_header() {
    let resolver = HeaderResolver::new("X-Org-Id");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[("x-org-id", "widgets")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("widgets"));
}

#[tokio::test]
async fn header_resolver_is_case_insensitive() {
    let resolver = HeaderResolver::new("X-Tenant-ID");
    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[("x-tenant-id", "beta")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("beta"));
}

#[tokio::test]
async fn header_resolver_fails_on_missing_header() {
    let resolver = HeaderResolver::default();
    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[("x-other", "value")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::NotFound(_))));
}

#[tokio::test]
async fn header_resolver_fails_on_empty_header() {
    let resolver = HeaderResolver::default();
    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[("x-tenant-id", "")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert!(matches!(result, Err(ResolutionError::NotFound(_))));
}

// ─── FallbackResolver Tests ─────────────────────────────────────────────────

#[tokio::test]
async fn fallback_uses_primary_when_it_succeeds() {
    let primary = ClaimResolver::new("org_id");
    let fallback = HeaderResolver::new("x-tenant-id");
    let resolver = FallbackResolver::new(primary, fallback);

    let p = principal("user-1", &[("org_id", "from-claim")]);
    let ctx = req_ctx(None, &[("x-tenant-id", "from-header")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("from-claim"));
}

#[tokio::test]
async fn fallback_uses_secondary_when_primary_fails_with_not_found() {
    let primary = ClaimResolver::new("org_id");
    let fallback = HeaderResolver::new("x-tenant-id");
    let resolver = FallbackResolver::new(primary, fallback);

    let p = principal("user-1", &[]); // No org_id claim
    let ctx = req_ctx(None, &[("x-tenant-id", "from-header")], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("from-header"));
}

#[tokio::test]
async fn fallback_returns_error_when_both_fail() {
    let primary = ClaimResolver::new("org_id");
    let fallback = HeaderResolver::new("x-tenant-id");
    let resolver = FallbackResolver::new(primary, fallback);

    let p = principal("user-1", &[]);
    let ctx = req_ctx(None, &[], "/"); // No header either

    let result = resolver.resolve(&p, &ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn fallback_chain_tries_multiple() {
    let resolvers: Vec<Box<dyn TenantResolver>> = vec![
        Box::new(ClaimResolver::new("org_id")),
        Box::new(HeaderResolver::new("x-tenant-id")),
        Box::new(SubdomainResolver::new("app.example.com")),
    ];
    let resolver = FallbackResolver::chain(resolvers);

    // Only the subdomain one will match
    let p = principal("user-1", &[]);
    let ctx = req_ctx(Some("acme.app.example.com"), &[], "/");

    let result = resolver.resolve(&p, &ctx).await;
    assert_eq!(result.unwrap(), TenantId::new("acme"));
}

// ─── Middleware Tests ───────────────────────────────────────────────────────

mod middleware_tests {
    use super::*;
    use axum::{routing::get, Router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn test_handler(
        tenant: middleware::ResolvedTenant,
    ) -> String {
        format!("tenant={}", tenant.0)
    }

    fn build_test_router(resolver: Arc<dyn TenantResolver>) -> Router {
        Router::new()
            .route("/test", get(test_handler))
            .layer(TenantResolutionLayer::new(resolver))
    }

    #[tokio::test]
    async fn middleware_resolves_tenant_from_header() {
        let resolver: Arc<dyn TenantResolver> = Arc::new(HeaderResolver::default());
        let app = build_test_router(resolver);

        let req = Request::builder()
            .uri("/test")
            .header("x-tenant-id", "acme")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "tenant=acme");
    }

    #[tokio::test]
    async fn middleware_returns_403_when_resolution_fails() {
        let resolver: Arc<dyn TenantResolver> = Arc::new(HeaderResolver::default());
        let app = build_test_router(resolver);

        let req = Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn middleware_uses_principal_from_extensions() {
        let resolver: Arc<dyn TenantResolver> = Arc::new(ClaimResolver::new("org_id"));

        // We need to inject Principal into extensions before our middleware runs.
        // Use a wrapping layer that injects it.
        let app = Router::new()
            .route("/test", get(test_handler))
            .layer(TenantResolutionLayer::new(resolver))
            .layer(axum::middleware::from_fn(inject_principal));

        let req = Request::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(std::str::from_utf8(&body).unwrap(), "tenant=test-org");
    }

    /// Helper middleware that injects a test Principal into extensions.
    async fn inject_principal(
        mut req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let p = veil_shared::Principal {
            id: "test-user".into(),
            roles: vec![],
            claims: [("org_id".to_string(), "test-org".to_string())]
                .into_iter()
                .collect(),
        };
        req.extensions_mut().insert(p);
        next.run(req).await
    }

    #[tokio::test]
    async fn middleware_with_fallback_chain() {
        let resolver: Arc<dyn TenantResolver> = Arc::new(FallbackResolver::new(
            ClaimResolver::new("org_id"),
            HeaderResolver::default(),
        ));
        let app = build_test_router(resolver);

        // No Principal in extensions, no org_id claim on anonymous,
        // but header present → fallback resolves
        let req = Request::builder()
            .uri("/test")
            .header("x-tenant-id", "fallback-tenant")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            "tenant=fallback-tenant"
        );
    }
}

// ─── Config Tests ───────────────────────────────────────────────────────────

mod config_tests {
    use super::*;

    #[tokio::test]
    async fn config_builds_claim_resolver() {
        let config = TenantResolutionConfig {
            strategy: config::ResolutionStrategy::Claim,
            claim_field: "tenant".into(),
            ..Default::default()
        };

        let resolver = config.build_resolver(None).await.unwrap();
        let p = principal("u1", &[("tenant", "alpha")]);
        let ctx = req_ctx(None, &[], "/");
        assert_eq!(resolver.resolve(&p, &ctx).await.unwrap(), TenantId::new("alpha"));
    }

    #[tokio::test]
    async fn config_builds_header_resolver() {
        let config = TenantResolutionConfig {
            strategy: config::ResolutionStrategy::Header,
            header_name: "x-org".into(),
            ..Default::default()
        };

        let resolver = config.build_resolver(None).await.unwrap();
        let p = principal("u1", &[]);
        let ctx = req_ctx(None, &[("x-org", "beta")], "/");
        assert_eq!(resolver.resolve(&p, &ctx).await.unwrap(), TenantId::new("beta"));
    }

    #[tokio::test]
    async fn config_builds_subdomain_resolver() {
        let config = TenantResolutionConfig {
            strategy: config::ResolutionStrategy::Subdomain,
            base_domain: Some("app.test.io".into()),
            ..Default::default()
        };

        let resolver = config.build_resolver(None).await.unwrap();
        let p = principal("u1", &[]);
        let ctx = req_ctx(Some("gamma.app.test.io"), &[], "/");
        assert_eq!(resolver.resolve(&p, &ctx).await.unwrap(), TenantId::new("gamma"));
    }

    #[tokio::test]
    async fn config_requires_base_domain_for_subdomain() {
        let config = TenantResolutionConfig {
            strategy: config::ResolutionStrategy::Subdomain,
            base_domain: None,
            ..Default::default()
        };

        let result = config.build_resolver(None).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("VEIL_TENANT_BASE_DOMAIN"));
    }

    #[tokio::test]
    async fn config_requires_ddb_client_for_lookup() {
        let config = TenantResolutionConfig {
            strategy: config::ResolutionStrategy::Lookup,
            lookup_table: Some("my-table".into()),
            ..Default::default()
        };

        let result = config.build_resolver(None).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("DynamoDB client"));
    }

    #[tokio::test]
    async fn config_builds_fallback_chain() {
        let config = TenantResolutionConfig {
            strategy: config::ResolutionStrategy::Claim,
            fallback: Some(config::ResolutionStrategy::Header),
            claim_field: "org_id".into(),
            header_name: "x-tenant-id".into(),
            ..Default::default()
        };

        let resolver = config.build_resolver(None).await.unwrap();

        // Primary (claim) fails, fallback (header) succeeds
        let p = principal("u1", &[]);
        let ctx = req_ctx(None, &[("x-tenant-id", "delta")], "/");
        assert_eq!(resolver.resolve(&p, &ctx).await.unwrap(), TenantId::new("delta"));
    }
}

// ─── RequestContext Tests ───────────────────────────────────────────────────

#[test]
fn request_context_header_lookup_is_case_insensitive() {
    let ctx = req_ctx(None, &[("x-tenant-id", "value")], "/");
    assert_eq!(ctx.header("X-Tenant-Id"), Some("value"));
    assert_eq!(ctx.header("x-tenant-id"), Some("value"));
    assert_eq!(ctx.header("X-TENANT-ID"), Some("value"));
}

#[test]
fn request_context_header_returns_none_for_missing() {
    let ctx = req_ctx(None, &[], "/");
    assert_eq!(ctx.header("x-tenant-id"), None);
}
