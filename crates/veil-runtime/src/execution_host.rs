//! VEIL Execution Host — a run-mode of `veil-runtime`.
//!
//! ONE long-lived process that any VEIL project registers a compiled artifact +
//! trigger into; the host dynamically loads the artifact via FFI and runs it
//! on-demand / on schedule / on domain event. The execution analog of the UI
//! harness. Booted via `VEIL_ROLE=execution-host` (see `main.rs`).
//!
//! It COMPOSES the existing substrate — it does not rebuild it:
//! - [`FunctionRegistry`](crate::function_invoke::FunctionRegistry) owns
//!   resolution + the `FfiLibraryCache` (warm/lazy load by content hash, LRU
//!   eviction, toolchain-fingerprint gate, hash verification).
//! - [`TriggerResolver`](crate::triggers::TriggerResolver) applies the
//!   concurrency bound + feedback seam and maps a fire → resolve → invoke.
//! - [`TriggerStore`](crate::triggers::TriggerStore) persists trigger rows in the
//!   `applications` table (`TRIGGER#` PK space).
//! - [`compile_and_register`](crate::compile_workflow::compile_and_register)
//!   stamps the toolchain fingerprint at compile time.
//!
//! ## HTTP surface (all JSON)
//! - `GET  /health`                      — liveness + host fingerprint + capacity
//! - `POST /register`                    — upsert artifact record + trigger rows
//! - `POST /invoke`                      — on-demand invoke by artifact id
//! - `POST /triggers/{tenant}/{id}/fire` — fire a stored trigger
//! - `GET  /triggers/{tenant}`           — list a tenant's triggers
//! - `GET  /registered`                  — list registered artifacts

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::artifact_registry::ArtifactRegistryStore;
use crate::function_invoke::FunctionRegistry;
use crate::triggers::{
    resolver::default_max_concurrency, TriggerDeclaration, TriggerResolver, TriggerStore,
};

/// Shared, cheaply-cloneable execution-host state.
#[derive(Clone)]
pub struct ExecutionHost {
    pub store: Arc<ArtifactRegistryStore>,
    pub registry: FunctionRegistry,
    pub resolver: TriggerResolver,
    pub trigger_store: Arc<TriggerStore>,
}

impl ExecutionHost {
    /// Build the host from ambient AWS config, wiring the shared registry
    /// (with a Lambda client), the FFI cache (inside the registry), and the
    /// trigger store + resolver. Reuses the artifact store's DDB client for the
    /// trigger store so the process holds a single AWS config.
    pub async fn from_env() -> Self {
        let store = Arc::new(ArtifactRegistryStore::from_env().await);
        let registry = FunctionRegistry::from_env(store.clone()).await;
        let trigger_store = Arc::new(TriggerStore::new(
            store.ddb.clone(),
            store.table.clone(),
        ));
        let resolver = TriggerResolver::new(
            registry.clone(),
            trigger_store.clone(),
            default_max_concurrency(),
        );
        Self {
            store,
            registry,
            resolver,
            trigger_store,
        }
    }

    /// Build the axum router for the execution-host HTTP surface.
    pub fn router(self) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/register", post(register))
            .route("/invoke", post(invoke))
            .route("/triggers/{tenant}/{id}/fire", post(fire_trigger))
            .route("/triggers/{tenant}", get(list_triggers))
            .route("/registered", get(list_registered))
            .with_state(self)
    }
}

// ─── /health ────────────────────────────────────────────────────────────────

async fn health(State(host): State<ExecutionHost>) -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "service": "veil-runtime",
        "role": "execution-host",
        "toolchain_fingerprint": crate::toolchain::host_fingerprint().to_wire(),
        "ffi_cache_len": host.registry.ffi_cache_len(),
        "invoke_permits_available": host.resolver.available_permits(),
    }))
}

// ─── /register ────────────────────────────────────────────────────────────────

/// Registration request: an already-compiled artifact (id, content hash, cdylib
/// in S3 by hash) plus its trigger declarations. Upserts the `ArtifactRecord`
/// (InvokeKind::Ffi, stamped with the host fingerprint) AND the trigger rows.
#[derive(Debug, Deserialize)]
struct RegisterRequest {
    /// Owning tenant.
    tenant_id: String,
    /// Artifact / function id (e.g. `"wf:acme/onboarding"`).
    artifact_id: String,
    /// Content hash of the cdylib already uploaded to S3 (== version).
    content_hash: String,
    /// Optional explicit blob key; defaults to `artifacts/{id}/{hash}/lib.so`.
    #[serde(default)]
    blob_key: Option<String>,
    /// Toolchain fingerprint the artifact was compiled with. Defaults to the
    /// host's fingerprint (the compile pipeline runs the same toolchain); a
    /// caller may override for a cross-built artifact.
    #[serde(default)]
    toolchain_fingerprint: Option<String>,
    /// Trigger declarations flowing into registration.
    #[serde(default)]
    triggers: Vec<TriggerDeclaration>,
}

#[derive(Debug, Serialize)]
struct RegisterResponse {
    artifact_id: String,
    version: String,
    triggers_registered: usize,
}

async fn register(
    State(host): State<ExecutionHost>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (axum::http::StatusCode, String)> {
    use crate::artifact_registry::{
        Abi, ArtifactRecord, ArtifactType, Contribution, InvokeKind, TenantVisibility,
    };
    use axum::http::StatusCode;

    let now = chrono::Utc::now();
    let fingerprint = req
        .toolchain_fingerprint
        .clone()
        .unwrap_or_else(|| crate::toolchain::host_fingerprint().to_wire());
    let blob_key = req
        .blob_key
        .clone()
        .unwrap_or_else(|| format!("artifacts/{}/{}/lib.so", req.artifact_id, req.content_hash));

    // Upsert the artifact record (InvokeKind::Ffi). Auto-signed so it is
    // resolvable — host artifacts have no VEIL review gate (see design page).
    let record = ArtifactRecord {
        id: req.artifact_id.clone(),
        version: req.content_hash.clone(),
        artifact_type: ArtifactType::Cdylib,
        tenant_visibility: TenantVisibility::All,
        contributions: vec![Contribution::BackendFunction {
            name: req.artifact_id.clone(),
            abi: Abi::Ffi,
            capabilities: vec![],
            invoke_kind: InvokeKind::Ffi,
            function_name: None,
        }],
        signed_off_by: Some("execution-host-register".to_string()),
        signed_off_at: Some(now),
        blob_key: Some(blob_key),
        content_hash: Some(req.content_hash.clone()),
        bundle_path: None,
        bundle_size: None,
        manifest: None,
        toolchain_fingerprint: Some(fingerprint),
        created_at: now,
        updated_at: now,
    };
    host.store
        .put_artifact(&record)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("put_artifact: {e}")))?;

    // Promote + upsert trigger rows.
    let records: Vec<_> = req
        .triggers
        .into_iter()
        .map(|d| d.into_record(&req.tenant_id, &req.artifact_id))
        .collect();
    let n = records.len();
    host.trigger_store
        .put_many(&records)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("put triggers: {e}")))?;

    // A new artifact version invalidates any cached resolution for this id.
    host.registry.invalidate_function(&req.artifact_id).await;

    Ok(Json(RegisterResponse {
        artifact_id: req.artifact_id,
        version: req.content_hash,
        triggers_registered: n,
    }))
}

// ─── /invoke ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    tenant_id: String,
    artifact_id: String,
    #[serde(default)]
    payload: Value,
}

async fn invoke(
    State(host): State<ExecutionHost>,
    Json(req): Json<InvokeRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let outcome = host
        .resolver
        .invoke_on_demand(&req.tenant_id, &req.artifact_id, req.payload)
        .await
        .map_err(trigger_status)?;
    Ok(Json(json!({
        "artifact_id": outcome.artifact_id,
        "result": outcome.result,
    })))
}

// ─── /triggers/{tenant}/{id}/fire ─────────────────────────────────────────────

async fn fire_trigger(
    State(host): State<ExecutionHost>,
    Path((tenant, id)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let outcome = host
        .resolver
        .fire_trigger(&tenant, &id, payload)
        .await
        .map_err(trigger_status)?;
    Ok(Json(json!({
        "artifact_id": outcome.artifact_id,
        "result": outcome.result,
    })))
}

// ─── /triggers/{tenant} ───────────────────────────────────────────────────────

async fn list_triggers(
    State(host): State<ExecutionHost>,
    Path(tenant): Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let triggers = host
        .trigger_store
        .list_for_tenant(&tenant)
        .await
        .map_err(trigger_status)?;
    Ok(Json(json!({ "triggers": triggers })))
}

// ─── /registered ──────────────────────────────────────────────────────────────

async fn list_registered(
    State(host): State<ExecutionHost>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    let records = host
        .store
        .list_all()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("list_all: {e}")))?;
    // Project down to the execution-relevant fields.
    let items: Vec<_> = records
        .into_iter()
        .filter(|r| {
            r.contributions.iter().any(|c| {
                matches!(
                    c,
                    crate::artifact_registry::Contribution::BackendFunction {
                        invoke_kind: crate::artifact_registry::InvokeKind::Ffi,
                        ..
                    }
                )
            })
        })
        .map(|r| {
            json!({
                "id": r.id,
                "version": r.version,
                "content_hash": r.content_hash,
                "toolchain_fingerprint": r.toolchain_fingerprint,
            })
        })
        .collect();
    Ok(Json(json!({ "registered": items })))
}

// ─── Error mapping ────────────────────────────────────────────────────────────

fn trigger_status(e: crate::triggers::TriggerError) -> (axum::http::StatusCode, String) {
    use axum::http::StatusCode;
    use crate::triggers::TriggerError;
    let code = match &e {
        TriggerError::NotFound(_) => StatusCode::NOT_FOUND,
        TriggerError::Disabled(_) => StatusCode::CONFLICT,
        TriggerError::Invalid(_) => StatusCode::BAD_REQUEST,
        TriggerError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        TriggerError::Invoke(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

/// Boot the execution-host run-mode: build state from env and serve the HTTP
/// surface on `VEIL_PORT` (default 8090 for the host role).
pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = std::env::var("VEIL_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8090);

    let host = ExecutionHost::from_env().await;
    let fp = crate::toolchain::host_fingerprint().to_wire();
    tracing::info!(port, toolchain = %fp, "booting VEIL execution-host run-mode");

    let app = host.router();
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("execution-host listening on 0.0.0.0:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}
