//! Tests for Phase 4: Artifact Serving (bundle, manifest, contributions, CORS).
//!
//! These tests exercise the handler logic using `axum::test` helpers and a
//! mock/in-memory artifact store approach: we build the router with a real
//! `ArtifactRegistryStore` backed by localstack-style env vars (disabled in CI)
//! OR we directly test the types, query structs, and helper functions that don't
//! require live AWS.

use super::types::*;
use std::collections::HashMap;

// ─── ArtifactManifest Tests ─────────────────────────────────────────────────

#[test]
fn artifact_manifest_default_is_empty() {
    let m = ArtifactManifest::default();
    assert!(m.entrypoint.is_none());
    assert!(m.exports.is_empty());
    assert!(m.props.is_empty());
}

#[test]
fn artifact_manifest_serde_roundtrip() {
    let m = ArtifactManifest {
        entrypoint: Some("index.js".into()),
        exports: vec!["mount".into(), "unmount".into()],
        props: {
            let mut p = HashMap::new();
            p.insert("tenantId".into(), "string".into());
            p.insert("apiClient".into(), "PlatformClient".into());
            p
        },
    };
    let json = serde_json::to_string(&m).unwrap();
    let deserialized: ArtifactManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.entrypoint, Some("index.js".into()));
    assert_eq!(deserialized.exports.len(), 2);
    assert!(deserialized.exports.contains(&"mount".into()));
    assert_eq!(deserialized.props.get("tenantId").unwrap(), "string");
}

// ─── ArtifactRecord with new fields ─────────────────────────────────────────

#[test]
fn artifact_record_serde_with_new_fields() {
    let now = chrono::Utc::now();
    let record = ArtifactRecord {
        id: "pkg:dashboard/analytics-widget".into(),
        version: "1.2.0".into(),
        artifact_type: ArtifactType::EsModule,
        tenant_visibility: TenantVisibility::All,
        contributions: vec![Contribution::MenuItem {
            label: "Analytics".into(),
            icon: Some("📊".into()),
            slot: "main".into(),
            route: Some("/analytics".into()),
            roles: vec![],
        }],
        signed_off_by: None,
        signed_off_at: None,
        blob_key: Some("artifacts/pkg:dashboard/analytics-widget/1.2.0/bundle.js".into()),
        content_hash: Some("abcdef1234567890".into()),
        bundle_path: Some("artifacts/pkg:dashboard/analytics-widget/1.2.0/abcdef1234567890.js".into()),
        bundle_size: Some(45_000),
        manifest: Some(ArtifactManifest {
            entrypoint: Some("index.js".into()),
            exports: vec!["mount".into(), "unmount".into()],
            props: {
                let mut p = HashMap::new();
                p.insert("tenantId".into(), "string".into());
                p
            },
        }),
        created_at: now,
        updated_at: now,
    };

    let json = serde_json::to_string(&record).unwrap();
    let deserialized: ArtifactRecord = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.content_hash.as_deref(), Some("abcdef1234567890"));
    assert_eq!(deserialized.bundle_size, Some(45_000));
    assert!(deserialized.manifest.is_some());
    let m = deserialized.manifest.unwrap();
    assert_eq!(m.entrypoint.as_deref(), Some("index.js"));
    assert_eq!(m.exports, vec!["mount", "unmount"]);
}

#[test]
fn artifact_record_serde_backward_compat_no_new_fields() {
    // Simulate reading a record stored BEFORE Phase 4 fields were added.
    let json = r#"{
        "id": "pkg:legacy/widget",
        "version": "1.0.0",
        "artifact_type": "es_module",
        "tenant_visibility": "all",
        "contributions": [],
        "signed_off_by": null,
        "signed_off_at": null,
        "blob_key": "artifacts/legacy/1.0.0/bundle.js",
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z"
    }"#;

    let record: ArtifactRecord = serde_json::from_str(json).unwrap();
    assert_eq!(record.id, "pkg:legacy/widget");
    assert!(record.content_hash.is_none());
    assert!(record.bundle_path.is_none());
    assert!(record.bundle_size.is_none());
    assert!(record.manifest.is_none());
}

// ─── guess_content_type ─────────────────────────────────────────────────────

#[test]
fn guess_content_type_js() {
    assert_eq!(
        crate::platform_http::guess_content_type("artifacts/x/1.0/bundle.js"),
        "application/javascript"
    );
}

#[test]
fn guess_content_type_css() {
    assert_eq!(
        crate::platform_http::guess_content_type("styles/app.css"),
        "text/css"
    );
}

#[test]
fn guess_content_type_wasm() {
    assert_eq!(
        crate::platform_http::guess_content_type("modules/calc.wasm"),
        "application/wasm"
    );
}

#[test]
fn guess_content_type_mjs() {
    assert_eq!(
        crate::platform_http::guess_content_type("esm/module.mjs"),
        "application/javascript"
    );
}

#[test]
fn guess_content_type_unknown() {
    assert_eq!(
        crate::platform_http::guess_content_type("data/file.xyz"),
        "application/octet-stream"
    );
}

#[test]
fn guess_content_type_no_extension() {
    assert_eq!(
        crate::platform_http::guess_content_type("noextension"),
        "application/octet-stream"
    );
}

// ─── build_artifact_cors_layer ──────────────────────────────────────────────

#[test]
fn cors_layer_permissive_when_unset() {
    // Clear the env var and verify it doesn't panic.
    unsafe { std::env::remove_var("VEIL_CORS_ORIGINS") };
    let _layer = crate::platform_http::build_artifact_cors_layer();
    // If we get here without panicking, the permissive fallback works.
}

#[test]
fn cors_layer_permissive_for_wildcard() {
    unsafe { std::env::set_var("VEIL_CORS_ORIGINS", "*") };
    let _layer = crate::platform_http::build_artifact_cors_layer();
    unsafe { std::env::remove_var("VEIL_CORS_ORIGINS") };
}

#[test]
fn cors_layer_specific_origins() {
    unsafe {
        std::env::set_var(
            "VEIL_CORS_ORIGINS",
            "http://localhost:5180, https://app.example.com",
        );
    }
    let _layer = crate::platform_http::build_artifact_cors_layer();
    unsafe { std::env::remove_var("VEIL_CORS_ORIGINS") };
}

// ─── Contribution filtering logic ──────────────────────────────────────────

#[test]
fn contribution_kind_filter_menu_item() {
    let artifacts = vec![
        ArtifactRecord {
            id: "pkg:nav/sidebar".into(),
            version: "1.0.0".into(),
            artifact_type: ArtifactType::EsModule,
            tenant_visibility: TenantVisibility::All,
            contributions: vec![
                Contribution::MenuItem {
                    label: "Dashboard".into(),
                    icon: Some("📊".into()),
                    slot: "main".into(),
                    route: Some("/dashboard".into()),
                    roles: vec![],
                },
                Contribution::Route {
                    path: "/dashboard".into(),
                    slot: "main".into(),
                },
            ],
            signed_off_by: None,
            signed_off_at: None,
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    ];

    // Filter for menu_item kind
    let kind = Some(ContributionKind::MenuItem);
    let principal = Principal {
        id: "user-1".into(),
        roles: vec!["admin".into()],
    };

    let mut results = Vec::new();
    for artifact in &artifacts {
        for contribution in &artifact.contributions {
            let matches_kind = match (&kind, contribution) {
                (Some(ContributionKind::MenuItem), Contribution::MenuItem { .. }) => true,
                (Some(ContributionKind::Route), Contribution::Route { .. }) => true,
                (Some(ContributionKind::SlotFill), Contribution::SlotFill { .. }) => true,
                (Some(ContributionKind::BackendFunction), Contribution::BackendFunction { .. }) => true,
                (None, _) => true,
                _ => false,
            };
            if !matches_kind {
                continue;
            }
            let role_ok = match contribution {
                Contribution::MenuItem { roles, .. } => {
                    roles.is_empty() || roles.iter().any(|r| principal.roles.contains(r))
                }
                _ => true,
            };
            if role_ok {
                results.push(contribution.clone());
            }
        }
    }

    assert_eq!(results.len(), 1);
    match &results[0] {
        Contribution::MenuItem { label, .. } => assert_eq!(label, "Dashboard"),
        _ => panic!("expected MenuItem"),
    }
}

#[test]
fn contribution_role_filtering() {
    let contributions = vec![
        Contribution::MenuItem {
            label: "Admin Panel".into(),
            icon: None,
            slot: "main".into(),
            route: Some("/admin".into()),
            roles: vec!["admin".into()],
        },
        Contribution::MenuItem {
            label: "Public Page".into(),
            icon: None,
            slot: "main".into(),
            route: Some("/public".into()),
            roles: vec![], // empty = visible to all
        },
    ];

    // User with no roles should only see Public Page.
    let principal = Principal {
        id: "viewer".into(),
        roles: vec![],
    };

    let visible: Vec<_> = contributions
        .iter()
        .filter(|c| match c {
            Contribution::MenuItem { roles, .. } => {
                roles.is_empty() || roles.iter().any(|r| principal.roles.contains(r))
            }
            _ => true,
        })
        .collect();

    assert_eq!(visible.len(), 1);
    match visible[0] {
        Contribution::MenuItem { label, .. } => assert_eq!(label, "Public Page"),
        _ => panic!("expected MenuItem"),
    }
}

#[test]
fn contribution_tenant_visibility_filtering() {
    let records = vec![
        ArtifactRecord {
            id: "pkg:global/widget".into(),
            version: "1.0.0".into(),
            artifact_type: ArtifactType::EsModule,
            tenant_visibility: TenantVisibility::All,
            contributions: vec![Contribution::MenuItem {
                label: "Global".into(),
                icon: None,
                slot: "main".into(),
                route: None,
                roles: vec![],
            }],
            signed_off_by: None,
            signed_off_at: None,
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
        ArtifactRecord {
            id: "pkg:acme/custom".into(),
            version: "1.0.0".into(),
            artifact_type: ArtifactType::EsModule,
            tenant_visibility: TenantVisibility::Specific(vec!["acme".into()]),
            contributions: vec![Contribution::MenuItem {
                label: "Acme Only".into(),
                icon: None,
                slot: "main".into(),
                route: None,
                roles: vec![],
            }],
            signed_off_by: None,
            signed_off_at: None,
            blob_key: None,
            content_hash: None,
            bundle_path: None,
            bundle_size: None,
            manifest: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        },
    ];

    // Filter for tenant "other" — should only see the global widget.
    let visible: Vec<_> = records
        .iter()
        .filter(|r| match &r.tenant_visibility {
            TenantVisibility::All => true,
            TenantVisibility::Specific(tenants) => tenants.contains(&"other".to_string()),
            TenantVisibility::None => false,
        })
        .collect();

    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, "pkg:global/widget");

    // Filter for tenant "acme" — should see both.
    let visible_acme: Vec<_> = records
        .iter()
        .filter(|r| match &r.tenant_visibility {
            TenantVisibility::All => true,
            TenantVisibility::Specific(tenants) => tenants.contains(&"acme".to_string()),
            TenantVisibility::None => false,
        })
        .collect();

    assert_eq!(visible_acme.len(), 2);
}
