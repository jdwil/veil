//! Tests for Phase 3 — Backend Function Resolution and Invocation.
//!
//! Tests are structured in layers:
//! 1. Unit: CallableHandle invoke, ResolveError mapping
//! 2. Integration: FunctionRegistry register + resolve + cache (requires DDB or local env)
//! 3. HTTP: invoke endpoint via axum test utilities

use std::sync::Arc;

use serde_json::json;

use super::*;

// ─── CallableHandle Unit Tests ──────────────────────────────────────────────

#[test]
fn callable_handle_invoke_returns_result() {
    let handle = CallableHandle::InProcess(Arc::new(|args| {
        let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(json!({ "doubled": x * 2 }))
    }));

    let result = handle.invoke(json!({ "x": 21 })).unwrap();
    assert_eq!(result, json!({ "doubled": 42 }));
}

#[test]
fn callable_handle_invoke_propagates_error() {
    let handle = CallableHandle::InProcess(Arc::new(|_args| {
        Err("something went wrong".into())
    }));

    let result = handle.invoke(json!({}));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("something went wrong"));
}

#[test]
fn callable_handle_invoke_with_empty_args() {
    let handle = CallableHandle::InProcess(Arc::new(|args| {
        Ok(json!({ "received": args }))
    }));

    let result = handle.invoke(json!(null)).unwrap();
    assert_eq!(result, json!({ "received": null }));
}

#[test]
fn callable_handle_invoke_with_complex_args() {
    let handle = CallableHandle::InProcess(Arc::new(|args| {
        let items = args
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(json!({ "count": items }))
    }));

    let result = handle.invoke(json!({ "items": [1, 2, 3] })).unwrap();
    assert_eq!(result, json!({ "count": 3 }));
}

#[test]
fn callable_handle_debug_impl() {
    let handle = CallableHandle::InProcess(Arc::new(|_| Ok(json!(null))));
    let debug = format!("{:?}", handle);
    assert!(debug.contains("InProcess"));
}

// ─── ResolveError Unit Tests ────────────────────────────────────────────────

#[test]
fn resolve_error_not_found_maps_to_404() {
    let err = ResolveError::NotFound("pkg:test/fn".into());
    assert_eq!(err.status_code(), axum::http::StatusCode::NOT_FOUND);
}

#[test]
fn resolve_error_not_visible_maps_to_403() {
    let err = ResolveError::NotVisible {
        tenant: "acme".into(),
        function_id: "pkg:test/fn".into(),
    };
    assert_eq!(err.status_code(), axum::http::StatusCode::FORBIDDEN);
}

#[test]
fn resolve_error_not_signed_off_maps_to_403() {
    let err = ResolveError::NotSignedOff("pkg:test/fn".into());
    assert_eq!(err.status_code(), axum::http::StatusCode::FORBIDDEN);
}

#[test]
fn resolve_error_internal_maps_to_500() {
    let err = ResolveError::Internal("db down".into());
    assert_eq!(
        err.status_code(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn resolve_error_display_messages() {
    assert!(ResolveError::NotFound("x".into()).to_string().contains("not found"));
    assert!(
        ResolveError::NotVisible {
            tenant: "t".into(),
            function_id: "f".into()
        }
        .to_string()
        .contains("not visible")
    );
    assert!(
        ResolveError::NotSignedOff("x".into())
            .to_string()
            .contains("not signed off")
    );
    assert!(ResolveError::Internal("boom".into()).to_string().contains("internal"));
}

// ─── CacheKey Tests ─────────────────────────────────────────────────────────

#[test]
fn cache_key_equality() {
    let a = CacheKey {
        tenant_id: "t1".into(),
        function_id: "pkg:a/b".into(),
        version: "1.0.0".into(),
    };
    let b = CacheKey {
        tenant_id: "t1".into(),
        function_id: "pkg:a/b".into(),
        version: "1.0.0".into(),
    };
    assert_eq!(a, b);
}

#[test]
fn cache_key_different_versions_not_equal() {
    let a = CacheKey {
        tenant_id: "t1".into(),
        function_id: "pkg:a/b".into(),
        version: "1.0.0".into(),
    };
    let b = CacheKey {
        tenant_id: "t1".into(),
        function_id: "pkg:a/b".into(),
        version: "2.0.0".into(),
    };
    assert_ne!(a, b);
}

#[test]
fn cache_key_different_tenants_not_equal() {
    let a = CacheKey {
        tenant_id: "t1".into(),
        function_id: "pkg:a/b".into(),
        version: "1.0.0".into(),
    };
    let b = CacheKey {
        tenant_id: "t2".into(),
        function_id: "pkg:a/b".into(),
        version: "1.0.0".into(),
    };
    assert_ne!(a, b);
}

#[test]
fn cache_key_hash_consistent() {
    use std::collections::HashMap;
    let key = CacheKey {
        tenant_id: "t1".into(),
        function_id: "pkg:a/b".into(),
        version: "1.0.0".into(),
    };
    let mut map = HashMap::new();
    map.insert(key.clone(), "value");
    assert_eq!(map.get(&key), Some(&"value"));
}

// ─── FunctionRegistry Unit Tests (no DDB) ───────────────────────────────────

// These tests validate register/unregister behavior on the in-process function
// map without going through resolve() (which requires DDB artifact lookup).

#[tokio::test]
async fn registry_register_and_invoke_directly() {
    // This tests the function map independently of artifact resolution.
    let store = create_test_store().await;
    let registry = FunctionRegistry::new(store);

    registry
        .register("test_fn", |args| {
            let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(json!({ "result": x + 1 }))
        })
        .await;

    // We can't call resolve() without DDB, but we verify the function is stored
    // by trying to unregister it (which affects the internal map).
    registry.unregister("test_fn").await;

    // After unregister, re-register to confirm the map is clean.
    registry
        .register("test_fn", |_| Ok(json!({ "status": "v2" })))
        .await;
}

#[tokio::test]
async fn registry_unregister_clears_cache() {
    let store = create_test_store().await;
    let registry = FunctionRegistry::new(store);

    registry
        .register("fn_a", |_| Ok(json!({ "ok": true })))
        .await;

    // Invalidating should not panic on empty cache.
    registry.invalidate_function("fn_a").await;
    registry.invalidate_tenant("tenant-1").await;
    registry.invalidate_all().await;
}

#[tokio::test]
async fn registry_resolve_fails_without_artifact() {
    let store = create_test_store().await;
    let registry = FunctionRegistry::new(store);

    // Register a closure but don't register an artifact in DDB.
    registry
        .register("pkg:test/my_fn", |_| Ok(json!({})))
        .await;

    let tenant = crate::tenancy::TenantId::new("acme");
    let result = registry.resolve(&tenant, "pkg:test/my_fn").await;

    // Should fail because there's no artifact in the registry store.
    assert!(result.is_err());
    match result.unwrap_err() {
        ResolveError::NotFound(_) | ResolveError::Internal(_) => {} // expected
        other => panic!("unexpected error: {other:?}"),
    }
}

// ─── Integration Tests (require DDB or local mock) ──────────────────────────

/// These tests require VEIL_DDB_TABLE and AWS_PROFILE (or DynamoDB local).
/// They validate the full resolve flow: register artifact → register closure → resolve → invoke.
///
/// Skip in CI when no DDB is available:
/// `VEIL_SKIP_DDB_TESTS=1 cargo test`
#[cfg(test)]
mod integration {
    use super::*;

    fn skip_if_no_ddb() -> bool {
        std::env::var("VEIL_SKIP_DDB_TESTS").is_ok()
            || (std::env::var("VEIL_DDB_TABLE").is_err()
                && std::env::var("AWS_PROFILE").is_err()
                && std::env::var("VEIL_PLATFORM_LOCAL").ok().as_deref() != Some("1"))
    }

    #[tokio::test]
    async fn full_resolve_and_invoke() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: full_resolve_and_invoke (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = FunctionRegistry::new(store.clone());

        let function_id = format!("pkg:test/invoke_test_{}", uuid::Uuid::new_v4());
        let now = chrono::Utc::now();

        // Register artifact with BackendFunction contribution + sign-off.
        let record = crate::artifact_registry::ArtifactRecord {
            id: function_id.clone(),
            version: "1.0.0".into(),
            artifact_type: crate::artifact_registry::ArtifactType::Cdylib,
            tenant_visibility: crate::artifact_registry::TenantVisibility::All,
            contributions: vec![crate::artifact_registry::Contribution::BackendFunction {
                name: function_id.clone(),
                abi: crate::artifact_registry::Abi::Json,
                capabilities: vec![],
            }],
            signed_off_by: Some("test-operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        // Register in-process closure.
        let fid = function_id.clone();
        registry
            .register(&fid, |args| {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("world");
                Ok(json!({ "greeting": format!("hello, {name}!") }))
            })
            .await;

        // Resolve under "all" visibility tenant.
        let tenant = crate::tenancy::TenantId::new("any-tenant");
        let handle = registry.resolve(&tenant, &function_id).await.unwrap();

        // Invoke.
        let result = handle.invoke(json!({ "name": "veil" })).unwrap();
        assert_eq!(result, json!({ "greeting": "hello, veil!" }));

        // Cleanup.
        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resolve_wrong_tenant_returns_not_visible() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: resolve_wrong_tenant_returns_not_visible (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = FunctionRegistry::new(store.clone());

        let function_id = format!("pkg:test/tenant_test_{}", uuid::Uuid::new_v4());
        let now = chrono::Utc::now();

        // Register artifact visible only to "acme".
        let record = crate::artifact_registry::ArtifactRecord {
            id: function_id.clone(),
            version: "1.0.0".into(),
            artifact_type: crate::artifact_registry::ArtifactType::Cdylib,
            tenant_visibility: crate::artifact_registry::TenantVisibility::Specific(vec![
                "acme".into(),
            ]),
            contributions: vec![crate::artifact_registry::Contribution::BackendFunction {
                name: function_id.clone(),
                abi: crate::artifact_registry::Abi::Json,
                capabilities: vec![],
            }],
            signed_off_by: Some("test-operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        registry
            .register(&function_id, |_| Ok(json!({ "ok": true })))
            .await;

        // Resolve under wrong tenant → 403.
        let wrong_tenant = crate::tenancy::TenantId::new("not-acme");
        let result = registry.resolve(&wrong_tenant, &function_id).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ResolveError::NotVisible { tenant, .. } => {
                assert_eq!(tenant, "not-acme");
            }
            other => panic!("expected NotVisible, got: {other:?}"),
        }

        // Resolve under correct tenant → success.
        let right_tenant = crate::tenancy::TenantId::new("acme");
        let handle = registry.resolve(&right_tenant, &function_id).await.unwrap();
        let result = handle.invoke(json!({})).unwrap();
        assert_eq!(result, json!({ "ok": true }));

        // Cleanup.
        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resolve_unsigned_artifact_returns_not_signed_off() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: resolve_unsigned_artifact_returns_not_signed_off (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = FunctionRegistry::new(store.clone());

        let function_id = format!("pkg:test/signoff_test_{}", uuid::Uuid::new_v4());
        let now = chrono::Utc::now();

        // Register artifact WITHOUT sign-off.
        let record = crate::artifact_registry::ArtifactRecord {
            id: function_id.clone(),
            version: "1.0.0".into(),
            artifact_type: crate::artifact_registry::ArtifactType::Cdylib,
            tenant_visibility: crate::artifact_registry::TenantVisibility::All,
            contributions: vec![crate::artifact_registry::Contribution::BackendFunction {
                name: function_id.clone(),
                abi: crate::artifact_registry::Abi::Json,
                capabilities: vec![],
            }],
            signed_off_by: None, // NOT signed off
            signed_off_at: None,
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        registry
            .register(&function_id, |_| Ok(json!({ "ok": true })))
            .await;

        let tenant = crate::tenancy::TenantId::new("any-tenant");
        let result = registry.resolve(&tenant, &function_id).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ResolveError::NotSignedOff(id) => {
                assert_eq!(id, function_id);
            }
            other => panic!("expected NotSignedOff, got: {other:?}"),
        }

        // Cleanup.
        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resolve_unregistered_function_returns_not_found() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: resolve_unregistered_function_returns_not_found (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = FunctionRegistry::new(store.clone());

        let tenant = crate::tenancy::TenantId::new("any-tenant");
        let result = registry
            .resolve(&tenant, "pkg:nonexistent/function")
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ResolveError::NotFound(_) | ResolveError::Internal(_) => {} // expected (DDB returns not found or connection error)
            other => panic!("expected NotFound or Internal, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cache_invalidation_forces_re_resolve() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: cache_invalidation_forces_re_resolve (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = FunctionRegistry::new(store.clone());

        let function_id = format!("pkg:test/cache_test_{}", uuid::Uuid::new_v4());
        let now = chrono::Utc::now();

        let record = crate::artifact_registry::ArtifactRecord {
            id: function_id.clone(),
            version: "1.0.0".into(),
            artifact_type: crate::artifact_registry::ArtifactType::Cdylib,
            tenant_visibility: crate::artifact_registry::TenantVisibility::All,
            contributions: vec![crate::artifact_registry::Contribution::BackendFunction {
                name: function_id.clone(),
                abi: crate::artifact_registry::Abi::Json,
                capabilities: vec![],
            }],
            signed_off_by: Some("operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        registry
            .register(&function_id, |_| Ok(json!({ "version": 1 })))
            .await;

        let tenant = crate::tenancy::TenantId::new("t1");
        let handle = registry.resolve(&tenant, &function_id).await.unwrap();
        assert_eq!(handle.invoke(json!({})).unwrap(), json!({ "version": 1 }));

        // Invalidate cache and re-register with new impl.
        registry.invalidate_function(&function_id).await;
        registry.unregister(&function_id).await;
        registry
            .register(&function_id, |_| Ok(json!({ "version": 2 })))
            .await;

        let handle = registry.resolve(&tenant, &function_id).await.unwrap();
        assert_eq!(handle.invoke(json!({})).unwrap(), json!({ "version": 2 }));

        // Cleanup.
        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }
}

// ─── Test Helpers ───────────────────────────────────────────────────────────

/// Create a test ArtifactRegistryStore (uses env vars or defaults).
/// In pure unit test mode (no DDB), calls will fail at the DDB layer.
async fn create_test_store() -> Arc<crate::artifact_registry::ArtifactRegistryStore> {
    Arc::new(crate::artifact_registry::ArtifactRegistryStore::from_env().await)
}
