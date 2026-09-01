//! Tests for the trigger layer.
//!
//! Model-level tests (payload merge, filter matching, declaration promotion)
//! need no DDB. Store CRUD is gated behind `VEIL_SKIP_DDB_TESTS` / live creds.

use super::*;
use crate::execution_topology::ExecutionTopology;
use serde_json::json;

fn base_record(kind: TriggerKind) -> TriggerRecord {
    TriggerRecord {
        id: "trg-1".into(),
        tenant_id: "acme".into(),
        artifact_id: "wf:acme/onboarding".into(),
        kind,
        schedule_expr: None,
        timezone: None,
        event_type: None,
        filter: None,
        payload_template: None,
        enabled: true,
        topology: crate::execution_topology::ExecutionTopology::Shared,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

#[test]
fn payload_template_provides_defaults_fire_wins() {
    let mut rec = base_record(TriggerKind::OnDemand);
    rec.payload_template = Some(json!({ "env": "prod", "retries": 3 }));
    let merged = rec.resolve_payload(json!({ "retries": 5, "user": "x" }));
    // template default kept, fire overrides `retries`, adds `user`.
    assert_eq!(merged, json!({ "env": "prod", "retries": 5, "user": "x" }));
}

#[test]
fn payload_template_used_when_fire_is_null() {
    let mut rec = base_record(TriggerKind::Schedule);
    rec.payload_template = Some(json!({ "tick": true }));
    assert_eq!(rec.resolve_payload(json!(null)), json!({ "tick": true }));
}

#[test]
fn payload_without_template_passes_through() {
    let rec = base_record(TriggerKind::OnDemand);
    assert_eq!(rec.resolve_payload(json!({ "a": 1 })), json!({ "a": 1 }));
}

#[test]
fn filter_none_matches_any_event() {
    let rec = base_record(TriggerKind::Event);
    assert!(rec.matches_filter(&json!({ "anything": true })));
}

#[test]
fn filter_requires_shallow_equality() {
    let mut rec = base_record(TriggerKind::Event);
    rec.filter = Some(json!({ "type": "order.created", "region": "us" }));
    assert!(rec.matches_filter(&json!({ "type": "order.created", "region": "us", "id": 9 })));
    assert!(!rec.matches_filter(&json!({ "type": "order.created", "region": "eu" })));
    assert!(!rec.matches_filter(&json!({ "type": "order.created" })));
}

#[test]
fn malformed_filter_fails_safe() {
    let mut rec = base_record(TriggerKind::Event);
    rec.filter = Some(json!("not-an-object"));
    assert!(!rec.matches_filter(&json!({ "type": "x" })));
}

#[test]
fn declaration_promotes_and_mints_id() {
    let decl = TriggerDeclaration {
        id: None,
        kind: TriggerKind::Schedule,
        schedule_expr: Some("rate(5 minutes)".into()),
        timezone: Some("UTC".into()),
        event_type: None,
        filter: None,
        payload_template: Some(json!({ "job": "sync" })),
        enabled: true,
    };
    let rec = decl.into_record("acme", "wf:acme/sync", ExecutionTopology::Shared);
    assert_eq!(rec.tenant_id, "acme");
    assert_eq!(rec.artifact_id, "wf:acme/sync");
    assert_eq!(rec.kind, TriggerKind::Schedule);
    assert_eq!(rec.schedule_expr.as_deref(), Some("rate(5 minutes)"));
    assert!(!rec.id.is_empty(), "id should be minted");
    assert!(rec.topology.is_shared());
}

#[test]
fn declaration_preserves_supplied_id() {
    let decl = TriggerDeclaration {
        id: Some("my-trigger".into()),
        kind: TriggerKind::OnDemand,
        schedule_expr: None,
        timezone: None,
        event_type: None,
        filter: None,
        payload_template: None,
        enabled: false,
    };
    let rec = decl.into_record(
        "t",
        "a",
        ExecutionTopology::Dedicated {
            slug: "a".into(),
            sizing: crate::execution_topology::DedicatedSizing::default(),
        },
    );
    assert_eq!(rec.id, "my-trigger");
    assert!(!rec.enabled);
    // Topology is persisted onto the record for fire-routing.
    assert!(rec.topology.is_dedicated());
    assert_eq!(
        rec.topology.dedicated_service_name().as_deref(),
        Some("veil-a-executor")
    );
}

#[test]
fn trigger_record_serde_round_trips() {
    let mut rec = base_record(TriggerKind::Event);
    rec.event_type = Some("order.created".into());
    rec.filter = Some(json!({ "region": "us" }));
    let s = serde_json::to_string(&rec).unwrap();
    let back: TriggerRecord = serde_json::from_str(&s).unwrap();
    assert_eq!(rec, back);
}

// ─── DDB-gated store tests ──────────────────────────────────────────────────

fn skip_if_no_ddb() -> bool {
    std::env::var("VEIL_SKIP_DDB_TESTS").is_ok()
        || (std::env::var("VEIL_DDB_TABLE").is_err()
            && std::env::var("AWS_PROFILE").is_err()
            && std::env::var("VEIL_PLATFORM_LOCAL").ok().as_deref() != Some("1"))
}

#[tokio::test]
async fn store_crud_round_trip() {
    if skip_if_no_ddb() {
        eprintln!("SKIP: store_crud_round_trip (no DDB)");
        return;
    }
    let store = TriggerStore::from_env().await;
    let tenant = format!("test-tenant-{}", uuid::Uuid::new_v4());
    let mut rec = base_record(TriggerKind::OnDemand);
    rec.tenant_id = tenant.clone();
    rec.id = uuid::Uuid::new_v4().to_string();

    store.put(&rec).await.unwrap();
    let got = store.get(&tenant, &rec.id).await.unwrap();
    assert_eq!(got.artifact_id, rec.artifact_id);

    let listed = store.list_for_tenant(&tenant).await.unwrap();
    assert_eq!(listed.len(), 1);

    store.delete(&tenant, &rec.id).await.unwrap();
    assert!(store.get(&tenant, &rec.id).await.is_err());
}

// ─── Full resolve→invoke through the resolver (rustc + DDB gated) ────────────
//
// Compiles a real cdylib, registers it as an Ffi artifact with a matching
// toolchain fingerprint + an on-demand trigger, then fires the trigger through
// the TriggerResolver and asserts the artifact ran and returned JSON. This is
// the spec's "trigger resolve→invoke" acceptance path.

/// Build a minimal cdylib exporting the workflow C ABI (doubles `x`). Returns
/// the `.so` path, or `None` if rustc/deps are unavailable.
fn build_doubler_cdylib() -> Option<std::path::PathBuf> {
    const SRC: &str = r#"
use std::ffi::{c_char, CStr, CString};
#[no_mangle]
pub extern "C" fn veil_workflow_run(input_json: *const c_char) -> *mut c_char {
    let input = unsafe { CStr::from_ptr(input_json) }.to_string_lossy().into_owned();
    let v: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
    let x = v.get("x").and_then(|n| n.as_i64()).unwrap_or(0);
    let out = serde_json::json!({ "doubled": x * 2 }).to_string();
    CString::new(out).unwrap().into_raw()
}
#[no_mangle]
pub extern "C" fn veil_workflow_free(ptr: *mut c_char) {
    if !ptr.is_null() { unsafe { drop(CString::from_raw(ptr)); } }
}
"#;
    let dir = std::env::temp_dir().join(format!("veil-trg-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join("wf.rs");
    std::fs::write(&src, SRC).ok()?;

    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let deps = [
        std::path::Path::new(&manifest).join("../../target/debug/deps"),
        std::path::Path::new(&manifest).join("../../target/release/deps"),
    ]
    .into_iter()
    .find(|p| p.exists())?;
    let find_rlib = |name: &str| -> Option<std::path::PathBuf> {
        let prefix = format!("lib{name}-");
        let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
        for e in std::fs::read_dir(&deps).ok()? {
            let e = e.ok()?;
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&prefix) && n.ends_with(".rlib") {
                let m = e.metadata().ok()?.modified().ok()?;
                if newest.as_ref().map(|(t, _)| m > *t).unwrap_or(true) {
                    newest = Some((m, e.path()));
                }
            }
        }
        newest.map(|(_, p)| p)
    };
    let serde_json_rlib = find_rlib("serde_json")?;
    let serde_rlib = find_rlib("serde")?;
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
        .arg(format!("dependency={}", deps.display()))
        .status()
        .ok()?;
    (status.success() && out.exists()).then_some(out)
}

#[tokio::test]
async fn trigger_resolve_then_invoke_runs_artifact() {
    let Some(so) = build_doubler_cdylib() else {
        eprintln!("skipping: rustc unavailable");
        return;
    };
    if skip_if_no_ddb() {
        eprintln!("SKIP: trigger_resolve_then_invoke_runs_artifact (no DDB)");
        return;
    }
    use crate::artifact_registry::*;
    use crate::function_invoke::FunctionRegistry;
    use crate::triggers::resolver::TriggerResolver;
    use std::sync::Arc;

    let store = Arc::new(ArtifactRegistryStore::from_env().await);
    let registry = FunctionRegistry::new(store.clone());
    let trigger_store = Arc::new(TriggerStore::new(store.ddb.clone(), store.table.clone()));
    let resolver = TriggerResolver::new(registry, trigger_store.clone(), 8);

    let tenant = format!("test-tenant-{}", uuid::Uuid::new_v4());
    let artifact_id = format!("wf:test/trg_{}", uuid::Uuid::new_v4());

    // Register the compiled cdylib as an Ffi artifact with matching fingerprint.
    let bytes = std::fs::read(&so).unwrap();
    let content_hash: String = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    };
    let blob_key = store
        .put_blob(&artifact_id, &content_hash, "lib.so", bytes)
        .await
        .unwrap();
    let now = chrono::Utc::now();
    store
        .put_artifact(&ArtifactRecord {
            id: artifact_id.clone(),
            version: content_hash.clone(),
            artifact_type: ArtifactType::Cdylib,
            tenant_visibility: TenantVisibility::All,
            contributions: vec![Contribution::BackendFunction {
                name: artifact_id.clone(),
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
            bundle_size: None,
            manifest: None,
            toolchain_fingerprint: Some(crate::toolchain::host_fingerprint().to_wire()),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    // Register an on-demand trigger with a payload template supplying x=21.
    let decl = TriggerDeclaration {
        id: None,
        kind: TriggerKind::OnDemand,
        schedule_expr: None,
        timezone: None,
        event_type: None,
        filter: None,
        payload_template: Some(json!({ "x": 21 })),
        enabled: true,
    };
    let trigger = decl.into_record(&tenant, &artifact_id, ExecutionTopology::Shared);
    let trigger_id = trigger.id.clone();
    trigger_store.put(&trigger).await.unwrap();

    // Fire the trigger with an empty payload → template supplies x=21 → doubled.
    let outcome = resolver
        .fire_trigger(&tenant, &trigger_id, json!(null))
        .await
        .expect("fire ok");
    assert_eq!(outcome.artifact_id, artifact_id);
    assert_eq!(outcome.result["doubled"], json!(42));

    // Cleanup.
    trigger_store.delete(&tenant, &trigger_id).await.unwrap();
    store.delete_artifact(&artifact_id, &content_hash).await.unwrap();
}
