//! Domain types for the Artifact Registry (Phase 1).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── Core Identifiers ────────────────────────────────────────────────────────

pub type TenantId = String;

/// Principal extracted from auth context. Lightweight for Phase 1;
/// full principal model arrives in Phase 2 (Tenant Resolution Framework).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub roles: Vec<String>,
}

// ─── Artifact Types ──────────────────────────────────────────────────────────

/// What kind of artifact the registry entry represents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// Custom element bundle (`<veil-app-name>`)
    WebComponent,
    /// ES module with `export function mount(el, props): Disposable`
    EsModule,
    /// Native FFI shared library
    Cdylib,
    /// WebAssembly module (sandboxed)
    Wasm,
}

/// Controls which tenants can see/resolve this artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TenantVisibility {
    /// Available to all tenants.
    All,
    /// Available only to the listed tenants.
    Specific(Vec<TenantId>),
    /// Not visible to any tenant (draft / unpublished).
    None,
}

// ─── Contributions ───────────────────────────────────────────────────────────

/// What a registered artifact contributes to the platform.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Contribution {
    /// A navigable menu entry in the shell chrome.
    MenuItem {
        label: String,
        icon: Option<String>,
        slot: String,
        route: Option<String>,
        roles: Vec<String>,
    },
    /// A routable path that loads this artifact.
    Route {
        path: String,
        slot: String,
    },
    /// Fill a named slot at an optional position.
    SlotFill {
        slot: String,
        position: Option<u32>,
    },
    /// A callable backend function.
    BackendFunction {
        name: String,
        abi: Abi,
        capabilities: Vec<String>,
        /// How this function is invoked. Defaults to `InProcess` for backward
        /// compatibility with records written before the Lambda substrate.
        #[serde(default)]
        invoke_kind: InvokeKind,
        /// For `invoke_kind = lambda`: the deployed Lambda function name or ARN
        /// the runtime invokes. `None` for in-process functions.
        #[serde(default)]
        function_name: Option<String>,
    },
}

/// How a registered backend function is executed when resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InvokeKind {
    /// A closure compiled into the runtime process (registered at startup/tests).
    #[default]
    InProcess,
    /// A deployed VEIL app running as an AWS Lambda, invoked by name/ARN.
    Lambda,
    /// A compiled workflow cdylib artifact, dynamically loaded (dlopen) into the
    /// daemon and invoked over the stable C ABI (`veil_workflow_run`). The
    /// artifact's `.so` is fetched from the artifact store by content hash.
    Ffi,
}

/// Calling convention for backend functions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Abi {
    /// JSON-in / JSON-out (serde_json::Value)
    Json,
    /// WASM component model (future)
    WasmComponent,
    /// C ABI (libloading)
    Ffi,
}

/// Discriminant for querying contributions by kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContributionKind {
    MenuItem,
    Route,
    SlotFill,
    BackendFunction,
}

// ─── Artifact Record ─────────────────────────────────────────────────────────

/// Manifest metadata for a frontend artifact — everything a harness needs to load it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactManifest {
    /// Primary entry point file (e.g. "index.js").
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Exported symbols (e.g. ["mount", "unmount"]).
    #[serde(default)]
    pub exports: Vec<String>,
    /// Props interface expected by the artifact (name → type hint).
    #[serde(default)]
    pub props: std::collections::HashMap<String, String>,
}

/// A registered, versioned artifact in the platform registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    /// Package-style identifier, e.g. "pkg:orders/process_order"
    pub id: String,
    /// Semver-ish version, content-hash based (e.g. "2.1.0")
    pub version: String,
    /// What kind of deployable artifact this is.
    pub artifact_type: ArtifactType,
    /// Which tenants can see this artifact.
    pub tenant_visibility: TenantVisibility,
    /// What this artifact contributes to the platform.
    pub contributions: Vec<Contribution>,
    /// Who signed off (human or automated policy).
    pub signed_off_by: Option<String>,
    /// When sign-off occurred.
    pub signed_off_at: Option<DateTime<Utc>>,
    /// S3 key for the blob (set after upload).
    pub blob_key: Option<String>,
    /// SHA-256 hex hash of the bundle content (enables immutable caching).
    #[serde(default)]
    pub content_hash: Option<String>,
    /// S3 key for the compiled bundle (may differ from blob_key for multi-file artifacts).
    #[serde(default)]
    pub bundle_path: Option<String>,
    /// Size of the bundle in bytes.
    #[serde(default)]
    pub bundle_size: Option<u64>,
    /// Manifest metadata (entrypoint, exports, props interface).
    #[serde(default)]
    pub manifest: Option<ArtifactManifest>,
    /// Toolchain fingerprint the artifact was compiled with, wire form
    /// `"{rustc_version}/{target_triple}"`. Load-bearing for the FFI/cdylib path:
    /// the execution host refuses to `dlopen` a cdylib whose fingerprint does not
    /// match the host's, because Rust has no stable ABI. `None` on legacy records
    /// written before this field existed (permitted but logged on load).
    #[serde(default)]
    pub toolchain_fingerprint: Option<String>,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last updated.
    pub updated_at: DateTime<Utc>,
}

// ─── Resolve Results ─────────────────────────────────────────────────────────

/// A contribution resolved for a specific tenant + principal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedContribution {
    /// Which artifact this contribution belongs to.
    pub artifact_id: String,
    pub artifact_version: String,
    /// The contribution itself.
    pub contribution: Contribution,
}

/// A resolved URL pointing to an artifact blob in S3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactUrl {
    /// Pre-signed or direct S3 URL for the artifact blob.
    pub url: String,
    pub artifact_id: String,
    pub version: String,
    pub artifact_type: ArtifactType,
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum RegistryError {
    NotFound(String),
    Storage(String),
    InvalidInput(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NotFound(msg) => write!(f, "not found: {msg}"),
            RegistryError::Storage(msg) => write!(f, "storage error: {msg}"),
            RegistryError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}
