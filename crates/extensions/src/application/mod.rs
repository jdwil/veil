//! Application services and flow functions.

#![allow(unused_imports, unused_variables, dead_code)]

use crate::domain::messages::*;
use crate::domain::types::*;
use crate::ports::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Injected dependencies (ports).
pub struct Deps {
    pub extension_artifact_store: std::sync::Arc<dyn ExtensionArtifactStore + Send + Sync>,
    pub extension_executor: std::sync::Arc<dyn ExtensionExecutor + Send + Sync>,
    pub extension_registry: std::sync::Arc<dyn ExtensionRegistry + Send + Sync>,
    pub extension_source_store: std::sync::Arc<dyn ExtensionSourceStore + Send + Sync>,
}

/// DomainService: CreateExtension
#[tracing::instrument(skip_all)]
pub async fn create_extension(
    deps: &Deps,
    name: String,
    kind: String,
    scope: String,
    provenance: String,
    product_id: Option<String>,
    tenant_id: Option<Uuid>,
    initiative_id: Option<Uuid>,
    description: Option<String>,
    params_schema: Option<serde_json::Value>,
) -> Result<ExtensionRecord, DomainError> {
    // step: execute
    let eid = Uuid::new_v4();
    let now = Utc::now();
    let mut sc = ExtensionScope::Platform;
    if scope == "Product".to_string() {
        sc = ExtensionScope::Product;
    };
    if scope == "Tenant".to_string() {
        sc = ExtensionScope::Tenant;
    };
    let mut kd = ExtensionKind::Reaction;
    if kind == "Signal".to_string() {
        kd = ExtensionKind::Signal;
    };
    if kind == "Activation".to_string() {
        kd = ExtensionKind::Activation;
    };
    if kind == "UiPanel".to_string() {
        kd = ExtensionKind::UiPanel;
    };
    let mut pv = ExtensionProvenance::Custom;
    if provenance == "Stock".to_string() {
        pv = ExtensionProvenance::Stock;
    };
    let root = deps
        .extension_source_store
        .ensure_package(eid.clone())
        .await?;
    let rec = ExtensionRecord {
        extension_id: eid.clone(),
        scope: sc.clone(),
        product_id: product_id.clone(),
        tenant_id: tenant_id.clone(),
        initiative_id: initiative_id.clone(),
        kind: kd.clone(),
        provenance: pv.clone(),
        name: name.clone(),
        description: description.clone(),
        current_version: 0,
        params_schema: params_schema.clone(),
        capabilities: vec![],
        palette_layer_refs: vec![],
        source_uri: root.clone(),
        created_from: None,
        created_on: now.clone(),
        updated_on: now.clone(),
        archived: false,
    };
    let saved = deps.extension_registry.create(rec.clone()).await?;
    return Ok(saved);
}

/// DomainService: ListExtensions
#[tracing::instrument(skip_all)]
pub async fn list_extensions(
    deps: &Deps,
    scope: Option<String>,
    kind: Option<String>,
    product_id: Option<String>,
    tenant_id: Option<Uuid>,
) -> Result<Vec<ExtensionRecord>, DomainError> {
    // step: execute
    let items = deps
        .extension_registry
        .list(
            scope.clone(),
            kind.clone(),
            product_id.clone(),
            tenant_id.clone(),
        )
        .await?;
    return Ok(items);
}

/// DomainService: GetExtension
#[tracing::instrument(skip_all)]
pub async fn get_extension(deps: &Deps, id: Uuid) -> Result<ExtensionRecord, DomainError> {
    // step: execute
    let rec = deps.extension_registry.get(id.clone()).await?.unwrap();
    return Ok(rec);
}

/// DomainService: ListExtensionVersions
#[tracing::instrument(skip_all)]
pub async fn list_extension_versions(
    deps: &Deps,
    id: Uuid,
) -> Result<Vec<ExtensionVersion>, DomainError> {
    // step: execute
    let items = deps.extension_registry.list_versions(id.clone()).await?;
    return Ok(items);
}

/// DomainService: GetExtensionVersion
#[tracing::instrument(skip_all)]
pub async fn get_extension_version(
    deps: &Deps,
    id: Uuid,
    version: i64,
) -> Result<ExtensionVersion, DomainError> {
    // step: execute
    let ver = deps
        .extension_registry
        .get_version(id.clone(), version.clone())
        .await?
        .unwrap();
    return Ok(ver);
}

/// DomainService: SaveExtensionVersion
#[tracing::instrument(skip_all)]
pub async fn save_extension_version(
    deps: &Deps,
    extension_id: Uuid,
    source_commit: String,
    artifact_uris: serde_json::Value,
    changelog: Option<String>,
) -> Result<ExtensionVersion, DomainError> {
    // step: execute
    let mut rec = deps
        .extension_registry
        .get(extension_id.clone())
        .await?
        .unwrap();
    let next_v = rec.current_version + 1;
    rec.current_version = next_v;
    rec.updated_on = Utc::now();
    deps.extension_registry.update(rec.clone()).await?;
    let now = Utc::now();
    let ver = ExtensionVersion {
        extension_id: extension_id.clone(),
        version: next_v,
        source_commit: source_commit.clone(),
        artifact_uris: artifact_uris.clone(),
        published_on: now.clone(),
        published_by: None,
        changelog: changelog.clone(),
    };
    let saved = deps.extension_registry.save_version(ver.clone()).await?;
    return Ok(saved);
}

/// DomainService: PublishExtension
#[tracing::instrument(skip_all)]
pub async fn publish_extension(deps: &Deps, id: Uuid) -> Result<ExtensionVersion, DomainError> {
    // step: execute
    let mut rec = deps.extension_registry.get(id.clone()).await?.unwrap();
    let next_v = rec.current_version + 1;
    rec.current_version = next_v;
    rec.updated_on = Utc::now();
    deps.extension_registry.update(rec.clone()).await?;
    let uri = deps
        .extension_artifact_store
        .put(
            id.clone(),
            next_v.clone(),
            "rust.marker".to_string(),
            vec![],
        )
        .await?;
    let uris = serde_json::json!({});
    let now = Utc::now();
    let ver = ExtensionVersion {
        extension_id: id.clone(),
        version: next_v,
        source_commit: "local".to_string(),
        artifact_uris: serde_json::json!(uris.clone()),
        published_on: now.clone(),
        published_by: None,
        changelog: None,
    };
    let saved = deps.extension_registry.save_version(ver.clone()).await?;
    return Ok(saved);
}

/// DomainService: InvokeExtension
#[tracing::instrument(skip_all)]
pub async fn invoke_extension(
    deps: &Deps,
    extension_id: Uuid,
    version: i64,
    kind: String,
    params: serde_json::Value,
    context: serde_json::Value,
) -> Result<ExtensionInvokeResult, DomainError> {
    // step: execute
    let _ver = deps
        .extension_registry
        .get_version(extension_id.clone(), version.clone())
        .await?
        .unwrap();
    let req = ExtensionInvokeRequest {
        extension_id: extension_id.clone(),
        version: version.clone(),
        kind: kind.clone(),
        params: params.clone(),
        context: context.clone(),
    };
    let res = deps.extension_executor.invoke(req.clone()).await?;
    return Ok(res);
}

/// DomainService: ListStockExtensions
#[tracing::instrument(skip_all)]
pub async fn list_stock_extensions(
    deps: &Deps,
    scope: Option<String>,
    kind: Option<String>,
    product_id: Option<String>,
) -> Result<Vec<ExtensionRecord>, DomainError> {
    // step: execute
    let items = deps
        .extension_registry
        .list(
            scope.clone(),
            kind.clone(),
            product_id.clone(),
            None.clone(),
        )
        .await?;
    return Ok(items);
}

/// DomainService: UpsertStockExtension
#[tracing::instrument(skip_all)]
pub async fn upsert_stock_extension(
    deps: &Deps,
    extension_id: Uuid,
    name: String,
    description: String,
    product_id: Option<String>,
    params_schema: Option<serde_json::Value>,
    capabilities: Vec<String>,
    palette_layer_refs: Vec<String>,
) -> Result<ExtensionRecord, DomainError> {
    // step: execute
    let now = Utc::now();
    let root = deps
        .extension_source_store
        .ensure_package(extension_id.clone())
        .await?;
    let mut rec = ExtensionRecord {
        extension_id: extension_id.clone(),
        scope: ExtensionScope::Platform.clone(),
        product_id: product_id.clone(),
        tenant_id: None,
        initiative_id: None,
        kind: ExtensionKind::Reaction.clone(),
        provenance: ExtensionProvenance::Stock.clone(),
        name: name.clone(),
        description: None,
        current_version: 1,
        params_schema: params_schema.clone(),
        capabilities: capabilities.clone(),
        palette_layer_refs: palette_layer_refs.clone(),
        source_uri: root.clone(),
        created_from: None,
        created_on: now.clone(),
        updated_on: now.clone(),
        archived: false,
    };
    rec.description = Some(description);
    let saved = deps.extension_registry.create(rec.clone()).await?;
    let ver = ExtensionVersion {
        extension_id: extension_id.clone(),
        version: 1,
        source_commit: "stock-seed".to_string(),
        artifact_uris: serde_json::json!(serde_json::json!({})),
        published_on: now.clone(),
        published_by: None,
        changelog: None,
    };
    deps.extension_registry.save_version(ver.clone()).await?;
    deps.extension_source_store
        .write_file(
            extension_id.clone(),
            "README.md".to_string(),
            "stock".to_string(),
        )
        .await?;
    return Ok(saved);
}

/// DomainService: EnsureStockCatalog
#[tracing::instrument(skip_all)]
pub async fn ensure_stock_catalog(
    deps: &Deps,
    activate_id: Uuid,
    guard_id: Uuid,
    product_id: Option<String>,
) -> Result<Vec<ExtensionRecord>, DomainError> {
    // step: execute
    let now = Utc::now();
    let root_a = deps
        .extension_source_store
        .ensure_package(activate_id.clone())
        .await?;
    let root_g = deps
        .extension_source_store
        .ensure_package(guard_id.clone())
        .await?;
    let caps_a = vec!["activation.invoke".to_string()];
    let caps_g = vec!["enrollment.is_enrolled".to_string()];
    let palette = vec![
        "reaction_guard".to_string(),
        "reaction_activate".to_string(),
        "reaction_end".to_string(),
    ];
    let schema_a = serde_json::json!({});
    let schema_g = serde_json::json!({});
    let rec_a = ExtensionRecord {
        extension_id: activate_id.clone(),
        scope: ExtensionScope::Platform.clone(),
        product_id: product_id.clone(),
        tenant_id: None,
        initiative_id: None,
        kind: ExtensionKind::Reaction.clone(),
        provenance: ExtensionProvenance::Stock.clone(),
        name: "Activate members".to_string(),
        description: Some("Invoke the default Activation for matching members".to_string()),
        current_version: 1,
        params_schema: Some(serde_json::json!(Some(schema_a))),
        capabilities: caps_a.clone(),
        palette_layer_refs: palette.clone(),
        source_uri: root_a.clone(),
        created_from: None,
        created_on: now.clone(),
        updated_on: now.clone(),
        archived: false,
    };
    let rec_g = ExtensionRecord {
        extension_id: guard_id.clone(),
        scope: ExtensionScope::Platform.clone(),
        product_id: product_id.clone(),
        tenant_id: None,
        initiative_id: None,
        kind: ExtensionKind::Reaction.clone(),
        provenance: ExtensionProvenance::Stock.clone(),
        name: "Guard then end".to_string(),
        description: Some("Skip if not enrolled; otherwise end success".to_string()),
        current_version: 1,
        params_schema: Some(serde_json::json!(Some(schema_g))),
        capabilities: caps_g.clone(),
        palette_layer_refs: palette.clone(),
        source_uri: root_g.clone(),
        created_from: None,
        created_on: now.clone(),
        updated_on: now.clone(),
        archived: false,
    };
    let a = deps.extension_registry.create(rec_a.clone()).await?;
    let g = deps.extension_registry.create(rec_g.clone()).await?;
    let ver_a = ExtensionVersion {
        extension_id: activate_id.clone(),
        version: 1,
        source_commit: "stock-seed".to_string(),
        artifact_uris: serde_json::json!(serde_json::json!({})),
        published_on: now.clone(),
        published_by: None,
        changelog: Some("seed".to_string()),
    };
    let ver_g = ExtensionVersion {
        extension_id: guard_id.clone(),
        version: 1,
        source_commit: "stock-seed".to_string(),
        artifact_uris: serde_json::json!(serde_json::json!({})),
        published_on: now.clone(),
        published_by: None,
        changelog: Some("seed".to_string()),
    };
    deps.extension_registry.save_version(ver_a.clone()).await?;
    deps.extension_registry.save_version(ver_g.clone()).await?;
    deps.extension_source_store
        .write_file(
            activate_id.clone(),
            "README.md".to_string(),
            "Activate members stock reaction".to_string(),
        )
        .await?;
    deps.extension_source_store
        .write_file(
            guard_id.clone(),
            "README.md".to_string(),
            "Guard then end stock reaction".to_string(),
        )
        .await?;
    return Ok(vec![a, g]);
}

/// DomainService: ForkExtension
#[tracing::instrument(skip_all)]
pub async fn fork_extension(
    deps: &Deps,
    source_id: Uuid,
    source_version: i64,
    name: String,
    tenant_id: Option<Uuid>,
    initiative_id: Option<Uuid>,
) -> Result<ExtensionRecord, DomainError> {
    // step: execute
    let src = deps
        .extension_registry
        .get(source_id.clone())
        .await?
        .unwrap();
    let eid = Uuid::new_v4();
    let now = Utc::now();
    let root = deps
        .extension_source_store
        .ensure_package(eid.clone())
        .await?;
    let lineage = ExtensionLineage {
        extension_id: source_id.clone(),
        version: source_version,
    };
    let rec = ExtensionRecord {
        extension_id: eid.clone(),
        scope: ExtensionScope::Tenant.clone(),
        product_id: src.product_id.clone(),
        tenant_id: tenant_id.clone(),
        initiative_id: initiative_id.clone(),
        kind: src.kind.clone(),
        provenance: ExtensionProvenance::Custom.clone(),
        name: name.clone(),
        description: src.description.clone(),
        current_version: 0,
        params_schema: Some(serde_json::json!(src.params_schema.clone())),
        capabilities: src.capabilities.clone(),
        palette_layer_refs: src.palette_layer_refs.clone(),
        source_uri: root.clone(),
        created_from: Some(lineage),
        created_on: now.clone(),
        updated_on: now.clone(),
        archived: false,
    };
    let saved = deps.extension_registry.create(rec.clone()).await?;
    deps.extension_source_store
        .write_file(
            eid.clone(),
            "FORK.md".to_string(),
            "forked from stock".to_string(),
        )
        .await?;
    deps.extension_source_store
        .write_file(eid.clone(), "README.md".to_string(), name.clone())
        .await?;
    return Ok(saved);
}

/// DomainService: ListExtensionsByScope
#[tracing::instrument(skip_all)]
pub async fn list_extensions_by_scope(
    deps: &Deps,
    scope: String,
    kind: Option<String>,
    product_id: Option<String>,
    tenant_id: Option<Uuid>,
) -> Result<Vec<ExtensionRecord>, DomainError> {
    // step: execute
    if scope == "Tenant".to_string() {
        if tenant_id.is_none() {
            return Ok(vec![]);
        };
    };
    let items = deps
        .extension_registry
        .list(
            Some(scope),
            kind.clone(),
            product_id.clone(),
            tenant_id.clone(),
        )
        .await?;
    return Ok(items);
}

/// DomainService: PromoteExtension
#[tracing::instrument(skip_all)]
pub async fn promote_extension(
    deps: &Deps,
    extension_id: Uuid,
    target_scope: String,
    allow_promote: bool,
) -> Result<ExtensionRecord, DomainError> {
    // step: execute
    if allow_promote == false {
        let rec = deps
            .extension_registry
            .get(extension_id.clone())
            .await?
            .unwrap();
        return Ok(rec);
    };
    let mut rec = deps
        .extension_registry
        .get(extension_id.clone())
        .await?
        .unwrap();
    if target_scope == "Platform".to_string() {
        rec.scope = ExtensionScope::Platform;
    };
    if target_scope == "Product".to_string() {
        rec.scope = ExtensionScope::Product;
    };
    rec.updated_on = Utc::now();
    let saved = deps.extension_registry.update(rec.clone()).await?;
    return Ok(saved);
}

/// DomainService: ValidateReactionPalette
#[tracing::instrument(skip_all)]
pub async fn validate_reaction_palette(node_kinds: Vec<String>) -> Result<bool, DomainError> {
    // step: execute
    for k in node_kinds {
        let mut ok = false;
        if k == "Guard".to_string() {
            ok = true;
        };
        if k == "Activate".to_string() {
            ok = true;
        };
        if k == "Map".to_string() {
            ok = true;
        };
        if k == "EmitEvent".to_string() {
            ok = true;
        };
        if k == "End".to_string() {
            ok = true;
        };
        if k == "guard".to_string() {
            ok = true;
        };
        if k == "activate".to_string() {
            ok = true;
        };
        if k == "map".to_string() {
            ok = true;
        };
        if k == "emit_event".to_string() {
            ok = true;
        };
        if k == "end".to_string() {
            ok = true;
        };
        if ok == false {
            return Ok(false);
        };
    }
    return Ok(true);
}

/// DomainService: MountUiExtension
#[tracing::instrument(skip_all)]
pub async fn mount_ui_extension(
    extension_id: Uuid,
    version: i64,
    slot: String,
    props: serde_json::Value,
) -> Result<UiMountHandle, DomainError> {
    // step: execute
    let uri = format!("local://extensions/{}/{}@{}", slot, extension_id, version);
    let h = UiMountHandle {
        mount_id: format!("{}", extension_id),
        asset_uri: uri.clone(),
    };
    return Ok(h);
}
