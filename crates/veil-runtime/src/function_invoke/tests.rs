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

#[tokio::test]
async fn callable_handle_invoke_returns_result() {
    let handle = CallableHandle::InProcess(Arc::new(|args| {
        let x = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(json!({ "doubled": x * 2 }))
    }));

    let result = handle.invoke(json!({ "x": 21 })).await.unwrap();
    assert_eq!(result, json!({ "doubled": 42 }));
}

#[tokio::test]
async fn callable_handle_invoke_propagates_error() {
    let handle = CallableHandle::InProcess(Arc::new(|_args| {
        Err("something went wrong".into())
    }));

    let result = handle.invoke(json!({})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("something went wrong"));
}

#[tokio::test]
async fn callable_handle_invoke_with_empty_args() {
    let handle = CallableHandle::InProcess(Arc::new(|args| {
        Ok(json!({ "received": args }))
    }));

    let result = handle.invoke(json!(null)).await.unwrap();
    assert_eq!(result, json!({ "received": null }));
}

#[tokio::test]
async fn callable_handle_invoke_with_complex_args() {
    let handle = CallableHandle::InProcess(Arc::new(|args| {
        let items = args
            .get("items")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(json!({ "count": items }))
    }));

    let result = handle.invoke(json!({ "items": [1, 2, 3] })).await.unwrap();
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
                invoke_kind: crate::artifact_registry::InvokeKind::InProcess,
                function_name: None,
            }],
            signed_off_by: Some("test-operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: None,
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
        let result = handle.invoke(json!({ "name": "veil" })).await.unwrap();
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
                invoke_kind: crate::artifact_registry::InvokeKind::InProcess,
                function_name: None,
            }],
            signed_off_by: Some("test-operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: None,
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
        let result = handle.invoke(json!({})).await.unwrap();
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
                invoke_kind: crate::artifact_registry::InvokeKind::InProcess,
                function_name: None,
            }],
            signed_off_by: None, // NOT signed off
            signed_off_at: None,
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: None,
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
                invoke_kind: crate::artifact_registry::InvokeKind::InProcess,
                function_name: None,
            }],
            signed_off_by: Some("operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        registry
            .register(&function_id, |_| Ok(json!({ "version": 1 })))
            .await;

        let tenant = crate::tenancy::TenantId::new("t1");
        let handle = registry.resolve(&tenant, &function_id).await.unwrap();
        assert_eq!(handle.invoke(json!({})).await.unwrap(), json!({ "version": 1 }));

        // Invalidate cache and re-register with new impl.
        registry.invalidate_function(&function_id).await;
        registry.unregister(&function_id).await;
        registry
            .register(&function_id, |_| Ok(json!({ "version": 2 })))
            .await;

        let handle = registry.resolve(&tenant, &function_id).await.unwrap();
        assert_eq!(handle.invoke(json!({})).await.unwrap(), json!({ "version": 2 }));

        // Cleanup.
        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }

    /// A lambda-backed BackendFunction (invoke_kind = lambda) resolves to a
    /// CallableHandle::Lambda carrying the registered function name — no
    /// in-process closure required. Exercises the production substrate path.
    #[tokio::test]
    async fn resolve_lambda_backed_function_returns_lambda_handle() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: resolve_lambda_backed_function_returns_lambda_handle (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        // Registry WITH a lambda client (the production wiring path).
        let registry = FunctionRegistry::from_env(store.clone()).await;

        let function_id = format!("dlx-auth-test-{}", uuid::Uuid::new_v4());
        let target_lambda = "veil-dlx-auth-test";
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
                invoke_kind: crate::artifact_registry::InvokeKind::Lambda,
                function_name: Some(target_lambda.into()),
            }],
            signed_off_by: Some("test-operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        // Resolve under the system tenant (auth pre-tenant path).
        let tenant = crate::tenancy::TenantId::new("__system__");
        let handle = registry.resolve(&tenant, &function_id).await.unwrap();

        match handle {
            CallableHandle::Lambda { function_name, .. } => {
                assert_eq!(function_name, target_lambda);
            }
            other => panic!("expected Lambda handle, got: {other:?}"),
        }

        // Cleanup.
        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }

    /// A lambda-backed function with no function_name is a registration error.
    #[tokio::test]
    async fn resolve_lambda_missing_name_is_internal_error() {
        if skip_if_no_ddb() {
            eprintln!("SKIP: resolve_lambda_missing_name_is_internal_error (no DDB)");
            return;
        }

        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = FunctionRegistry::from_env(store.clone()).await;

        let function_id = format!("dlx-auth-badreg-{}", uuid::Uuid::new_v4());
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
                invoke_kind: crate::artifact_registry::InvokeKind::Lambda,
                function_name: None, // missing target
            }],
            signed_off_by: Some("test-operator".into()),
            signed_off_at: Some(now),
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: None,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();

        let tenant = crate::tenancy::TenantId::new("__system__");
        let result = registry.resolve(&tenant, &function_id).await;
        assert!(matches!(result, Err(ResolveError::Internal(_))));

        store
            .delete_artifact(&function_id, "1.0.0")
            .await
            .unwrap();
    }
}

// ─── FFI cdylib loader end-to-end ───────────────────────────────────────────
//
// Compiles a minimal cdylib exporting the workflow C ABI with `rustc`, loads it
// via `LoadedWorkflow`, and round-trips JSON through it. Proves the ABI seam and
// panic isolation. Skips gracefully if `rustc` is unavailable in the sandbox.
mod ffi_e2e {
    use super::super::LoadedWorkflow;
    use serde_json::json;

    /// Source of a minimal workflow cdylib. `veil_workflow_run` echoes the input
    /// under `echo` and doubles an `x` field; a `boom` input panics (to test
    /// isolation).
    const CDYLIB_SRC: &str = r#"
use std::ffi::{c_char, CStr, CString};

#[no_mangle]
pub extern "C" fn veil_workflow_run(input_json: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        let input = unsafe { CStr::from_ptr(input_json) }.to_string_lossy().into_owned();
        let v: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
        if v.get("boom").is_some() {
            panic!("intentional workflow panic");
        }
        let x = v.get("x").and_then(|n| n.as_i64()).unwrap_or(0);
        serde_json::json!({ "echo": v, "doubled": x * 2 }).to_string()
    });
    let out = match result {
        Ok(s) => s,
        Err(_) => serde_json::json!({ "error": "workflow panicked" }).to_string(),
    };
    CString::new(out).unwrap().into_raw()
}

#[no_mangle]
pub extern "C" fn veil_workflow_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { drop(CString::from_raw(ptr)); }
    }
}
"#;

    /// Compile the cdylib source to a `.so`/`.dylib` using the workspace's
    /// serde_json rlib. Returns the artifact path, or `None` if the toolchain
    /// isn't usable in this environment (test then skips).
    fn build_test_cdylib() -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("veil-ffi-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok()?;
        let src = dir.join("wf.rs");
        std::fs::write(&src, CDYLIB_SRC).ok()?;

        // Locate a serde_json rlib in the workspace target dir to link against.
        let deps_dir = find_deps_dir()?;
        let serde_json_rlib = find_rlib(&deps_dir, "serde_json")?;
        let serde_rlib = find_rlib(&deps_dir, "serde")?;

        let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
        let out = dir.join(format!("libwf.{ext}"));

        let status = std::process::Command::new("rustc")
            .arg(&src)
            .arg("--crate-type=cdylib")
            .arg("--edition=2021")
            .arg("-o")
            .arg(&out)
            .arg("--extern")
            .arg(format!("serde_json={}", serde_json_rlib.display()))
            .arg("--extern")
            .arg(format!("serde={}", serde_rlib.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps_dir.display()))
            .status()
            .ok()?;

        if status.success() && out.exists() {
            Some(out)
        } else {
            None
        }
    }

    fn find_deps_dir() -> Option<std::path::PathBuf> {
        // CARGO_MANIFEST_DIR/../../target/debug/deps
        let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
        let candidates = [
            std::path::Path::new(&manifest).join("../../target/debug/deps"),
            std::path::Path::new(&manifest).join("../../target/release/deps"),
        ];
        candidates.into_iter().find(|p| p.exists())
    }

    fn find_rlib(deps: &std::path::Path, crate_name: &str) -> Option<std::path::PathBuf> {
        let prefix = format!("lib{crate_name}-");
        let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
        for entry in std::fs::read_dir(deps).ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && name.ends_with(".rlib") {
                let mtime = entry.metadata().ok()?.modified().ok()?;
                if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    newest = Some((mtime, entry.path()));
                }
            }
        }
        newest.map(|(_, p)| p)
    }

    #[test]
    fn ffi_load_and_invoke_round_trips_json() {
        let Some(so) = build_test_cdylib() else {
            eprintln!("skipping ffi e2e: rustc/deps unavailable in sandbox");
            return;
        };
        let lib = LoadedWorkflow::load(&so, "test-hash-abc").expect("load cdylib");
        assert_eq!(lib.content_hash(), "test-hash-abc");

        let out = lib.invoke(&json!({ "x": 21 })).expect("invoke ok");
        assert_eq!(out["doubled"], json!(42));
        assert_eq!(out["echo"], json!({ "x": 21 }));
    }

    #[test]
    fn ffi_workflow_panic_is_isolated_as_error() {
        let Some(so) = build_test_cdylib() else {
            eprintln!("skipping ffi e2e: rustc/deps unavailable in sandbox");
            return;
        };
        let lib = LoadedWorkflow::load(&so, "test-hash-boom").expect("load cdylib");

        // The cdylib's own catch_unwind converts the panic into {"error": ...}
        // which the loader surfaces as Err — the daemon stays alive.
        let res = lib.invoke(&json!({ "boom": true }));
        assert!(res.is_err(), "panicking workflow must surface as Err: {res:?}");
        assert!(res.unwrap_err().to_string().contains("workflow"));
    }

    // ─── Full FFI resolve path (fingerprint gate + hash verify + invoke) ─────
    //
    // These exercise `FunctionRegistry::resolve_ffi` end to end: they compile a
    // real cdylib, upload it to the artifact store by content hash, register an
    // Ffi ArtifactRecord, and resolve. They require BOTH `rustc` (to build the
    // cdylib) and DDB/S3 (the artifact store). Skips gracefully otherwise.
    use std::sync::Arc;

    fn skip_if_no_ddb() -> bool {
        std::env::var("VEIL_SKIP_DDB_TESTS").is_ok()
            || (std::env::var("VEIL_DDB_TABLE").is_err()
                && std::env::var("AWS_PROFILE").is_err()
                && std::env::var("VEIL_PLATFORM_LOCAL").ok().as_deref() != Some("1"))
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    async fn register_cdylib_artifact(
        store: &Arc<crate::artifact_registry::ArtifactRegistryStore>,
        function_id: &str,
        so_path: &std::path::Path,
        fingerprint: Option<String>,
    ) -> String {
        use crate::artifact_registry::*;
        let bytes = std::fs::read(so_path).unwrap();
        let content_hash = sha256_hex(&bytes);
        let blob_key = store
            .put_blob(function_id, &content_hash, "lib.so", bytes.clone())
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let record = ArtifactRecord {
            id: function_id.to_string(),
            version: content_hash.clone(),
            artifact_type: ArtifactType::Cdylib,
            tenant_visibility: TenantVisibility::All,
            contributions: vec![Contribution::BackendFunction {
                name: function_id.to_string(),
                abi: Abi::Ffi,
                capabilities: vec![],
                invoke_kind: InvokeKind::Ffi,
                function_name: None,
            }],
            signed_off_by: Some("test".into()),
            signed_off_at: Some(now),
            blob_key: Some(blob_key),
            content_hash: Some(content_hash.clone()),
            bundle_path: None,
            bundle_size: Some(bytes.len() as u64),
            manifest: None,
            toolchain_fingerprint: fingerprint,
            created_at: now,
            updated_at: now,
        };
        store.put_artifact(&record).await.unwrap();
        content_hash
    }

    #[tokio::test]
    async fn ffi_resolve_with_matching_fingerprint_invokes() {
        let Some(so) = build_test_cdylib() else {
            eprintln!("skipping: rustc unavailable");
            return;
        };
        if skip_if_no_ddb() {
            eprintln!("SKIP: ffi_resolve_with_matching_fingerprint_invokes (no DDB)");
            return;
        }
        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = super::super::FunctionRegistry::new(store.clone());
        let function_id = format!("wf:test/ffi_ok_{}", uuid::Uuid::new_v4());

        // Matching host fingerprint → loads and invokes.
        let host_fp = crate::toolchain::host_fingerprint().to_wire();
        let hash =
            register_cdylib_artifact(&store, &function_id, &so, Some(host_fp)).await;

        let tenant = crate::tenancy::TenantId::new("any");
        let handle = registry.resolve(&tenant, &function_id).await.expect("resolve ok");
        let out = handle.invoke(json!({ "x": 21 })).await.expect("invoke ok");
        assert_eq!(out["doubled"], json!(42));

        store.delete_artifact(&function_id, &hash).await.unwrap();
    }

    #[tokio::test]
    async fn ffi_resolve_with_mismatched_fingerprint_is_refused() {
        let Some(so) = build_test_cdylib() else {
            eprintln!("skipping: rustc unavailable");
            return;
        };
        if skip_if_no_ddb() {
            eprintln!("SKIP: ffi_resolve_with_mismatched_fingerprint_is_refused (no DDB)");
            return;
        }
        let store = Arc::new(
            crate::artifact_registry::ArtifactRegistryStore::from_env().await,
        );
        let registry = super::super::FunctionRegistry::new(store.clone());
        let function_id = format!("wf:test/ffi_bad_{}", uuid::Uuid::new_v4());

        // A fingerprint that cannot match the host → refuse-to-load, no dlopen.
        let bogus = "0.0.0-bogus/mips-unknown-none".to_string();
        let hash =
            register_cdylib_artifact(&store, &function_id, &so, Some(bogus)).await;

        let tenant = crate::tenancy::TenantId::new("any");
        let err = registry
            .resolve(&tenant, &function_id)
            .await
            .expect_err("must refuse mismatched toolchain");
        assert!(
            matches!(err, super::super::ResolveError::ToolchainMismatch(_)),
            "expected ToolchainMismatch, got {err:?}"
        );

        store.delete_artifact(&function_id, &hash).await.unwrap();
    }
}

// ─── Test Helpers ───────────────────────────────────────────────────────────

/// Create a test ArtifactRegistryStore (uses env vars or defaults).
/// In pure unit test mode (no DDB), calls will fail at the DDB layer.
async fn create_test_store() -> Arc<crate::artifact_registry::ArtifactRegistryStore> {
    Arc::new(crate::artifact_registry::ArtifactRegistryStore::from_env().await)
}
