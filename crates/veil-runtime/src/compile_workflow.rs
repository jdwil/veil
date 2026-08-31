//! Compile-on-save pipeline: a saved workflow becomes a runnable artifact.
//!
//! `save = runtime concern`: when a workflow is written, the runtime codegen's
//! it to Rust as a **cdylib**, `cargo build --release`s it, content-hashes the
//! resulting `.so`, uploads it to the artifact store at
//! `artifacts/{id}/{version}/lib.so`, and registers a **Pinned** artifact
//! version (`ArtifactType::Cdylib`, `InvokeKind::Ffi`) that the daemon later
//! `dlopen`s via the FFI loader.
//!
//! ANY transpile or compile error FAILS the save — surfaced to the caller — so
//! a workflow version is only "runnable" after a green compile.
//!
//! The compiled cdylib exports the stable C ABI shim
//! (`veil_workflow_run` / `veil_workflow_free`) emitted by
//! `veil-codegen`'s cdylib mode, which the FFI loader
//! (`function_invoke::ffi_loader`) invokes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::artifact_registry::{
    Abi, ArtifactRecord, ArtifactRegistryStore, ArtifactType, Contribution, InvokeKind,
    TenantVisibility,
};

/// Outcome of a successful compile-on-save.
#[derive(Debug, Clone)]
pub struct CompiledWorkflow {
    /// Artifact id (workflow id).
    pub id: String,
    /// Content-hash-derived version (the Pinned version).
    pub version: String,
    /// SHA-256 hex of the `.so`.
    pub content_hash: String,
    /// S3 blob key the `.so` was uploaded to.
    pub blob_key: String,
}

/// Error from the compile-on-save pipeline. Every variant fails the save.
#[derive(Debug)]
pub enum CompileError {
    /// `veil gen -t rust` (transpile) failed.
    Transpile(String),
    /// `cargo build --release` failed.
    Compile(String),
    /// Local filesystem / IO error.
    Io(String),
    /// Artifact store upload / registration failed.
    Registry(String),
    /// The build produced no cdylib artifact.
    NoArtifact(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Transpile(m) => write!(f, "workflow transpile failed: {m}"),
            CompileError::Compile(m) => write!(f, "workflow compile failed: {m}"),
            CompileError::Io(m) => write!(f, "workflow compile io error: {m}"),
            CompileError::Registry(m) => write!(f, "workflow artifact registration failed: {m}"),
            CompileError::NoArtifact(m) => write!(f, "workflow build produced no cdylib: {m}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Resolve the `veil` CLI binary: `VEIL_BIN`, else a sibling
/// `target/release/veil`, else `veil` on PATH. Mirrors `platform::compile_project`.
fn veil_bin() -> String {
    std::env::var("VEIL_BIN").unwrap_or_else(|_| {
        let cand = PathBuf::from("target/release/veil");
        if cand.is_file() {
            cand.to_string_lossy().to_string()
        } else {
            "veil".into()
        }
    })
}

/// Compile a workflow package to a cdylib artifact and register a Pinned version.
///
/// `workflow_id` is the artifact id (e.g. `"wf:tenant/onboarding"`).
/// `veil_source_path` is the path to the workflow's primary `.veil` package on
/// disk (already written to the working tree by the save handler).
/// `work_dir` is a scratch directory for generated Rust + build output.
///
/// On success, uploads `lib.so` to `artifacts/{id}/{hash}/lib.so` and registers
/// the artifact. The Pinned version IS the content hash, so re-saving identical
/// source is idempotent (same hash → same version).
pub async fn compile_and_register(
    store: &Arc<ArtifactRegistryStore>,
    workflow_id: &str,
    veil_source_path: &Path,
    work_dir: &Path,
) -> Result<CompiledWorkflow, CompileError> {
    let gen_dir = work_dir.join("generated");
    tokio::fs::create_dir_all(&gen_dir)
        .await
        .map_err(|e| CompileError::Io(format!("create gen dir: {e}")))?;

    // ── Step 1: transpile (veil gen -t rust, cdylib output) ──────────────
    // The workflow package's veil.toml declares `[codegen] output_type = "cdylib"`
    // so codegen emits the factory + `veil_workflow_run` shim and sets
    // crate-type = ["cdylib"]. Transpile errors fail the save.
    let gen_out = Command::new(veil_bin())
        .args([
            "gen",
            &veil_source_path.to_string_lossy(),
            "-t",
            "rust",
            "-o",
            &gen_dir.to_string_lossy(),
        ])
        .output()
        .await
        .map_err(|e| CompileError::Transpile(format!("veil gen failed to start: {e}")))?;
    if !gen_out.status.success() {
        let stderr = String::from_utf8_lossy(&gen_out.stderr);
        let stdout = String::from_utf8_lossy(&gen_out.stdout);
        return Err(CompileError::Transpile(format!("{stdout}\n{stderr}")));
    }

    // ── Step 2: compile (cargo build --release) ──────────────────────────
    let build_out = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&gen_dir)
        .output()
        .await
        .map_err(|e| CompileError::Compile(format!("cargo build failed to start: {e}")))?;
    if !build_out.status.success() {
        let stderr = String::from_utf8_lossy(&build_out.stderr);
        return Err(CompileError::Compile(stderr.to_string()));
    }

    // ── Step 3: locate the produced cdylib ───────────────────────────────
    let so_path = find_cdylib(&gen_dir.join("target").join("release"))
        .await
        .ok_or_else(|| {
            CompileError::NoArtifact(format!(
                "no .so/.dylib in {}",
                gen_dir.join("target/release").display()
            ))
        })?;

    // ── Step 4: content-hash the .so ─────────────────────────────────────
    let bytes = tokio::fs::read(&so_path)
        .await
        .map_err(|e| CompileError::Io(format!("read .so: {e}")))?;
    let content_hash = sha256_hex(&bytes);
    let version = content_hash.clone();

    // ── Step 5: upload to S3 at artifacts/{id}/{version}/lib.so ──────────
    let blob_key = store
        .put_blob(workflow_id, &version, "lib.so", bytes.clone())
        .await
        .map_err(|e| CompileError::Registry(format!("put_blob: {e}")))?;

    // ── Step 6: register a Pinned Cdylib/Ffi artifact version ────────────
    let now = chrono::Utc::now();
    let record = ArtifactRecord {
        id: workflow_id.to_string(),
        version: version.clone(),
        artifact_type: ArtifactType::Cdylib,
        tenant_visibility: TenantVisibility::All,
        contributions: vec![Contribution::BackendFunction {
            name: workflow_id.to_string(),
            abi: Abi::Ffi,
            capabilities: vec![],
            invoke_kind: InvokeKind::Ffi,
            function_name: None,
        }],
        // Compile-on-save auto-signs the version so it is immediately resolvable;
        // human sign-off gating for promotion is a separate lifecycle concern.
        signed_off_by: Some("compile-on-save".to_string()),
        signed_off_at: Some(now),
        blob_key: Some(blob_key.clone()),
        content_hash: Some(content_hash.clone()),
        bundle_path: Some(blob_key.clone()),
        bundle_size: Some(bytes.len() as u64),
        manifest: None,
        // The compile pipeline runs the SAME toolchain the host loads with, so
        // stamp the host fingerprint. The execution host refuses to dlopen an
        // artifact whose fingerprint drifts from its own (Rust has no stable ABI).
        toolchain_fingerprint: Some(crate::toolchain::host_fingerprint().to_wire()),
        created_at: now,
        updated_at: now,
    };
    store
        .put_artifact(&record)
        .await
        .map_err(|e| CompileError::Registry(format!("put_artifact: {e}")))?;

    Ok(CompiledWorkflow {
        id: workflow_id.to_string(),
        version,
        content_hash,
        blob_key,
    })
}

/// Compile + register an execution artifact **and** its declared triggers.
///
/// Reuses [`compile_and_register`] for the artifact (codegen → cdylib → hash →
/// upload → Pinned Ffi record with toolchain fingerprint), then parses the
/// project's `veil.toml [[triggers]]` block and upserts a [`TriggerRecord`] per
/// declaration for `tenant_id`. This is the registration path the execution
/// host + deploy pipeline call so an artifact and its "when" layer land together.
///
/// A trigger store failure after a successful artifact registration is returned
/// as `CompileError::Registry` — the artifact is registered but triggers may be
/// partial; re-running is safe (puts are idempotent by trigger id when the
/// declaration supplies one).
pub async fn compile_and_register_with_triggers(
    store: &Arc<ArtifactRegistryStore>,
    trigger_store: &Arc<crate::triggers::TriggerStore>,
    tenant_id: &str,
    workflow_id: &str,
    veil_source_path: &Path,
    veil_toml_path: &Path,
    work_dir: &Path,
) -> Result<(CompiledWorkflow, usize), CompileError> {
    let compiled = compile_and_register(store, workflow_id, veil_source_path, work_dir).await?;

    let declarations = crate::triggers::parse_triggers_from_file(veil_toml_path)
        .map_err(CompileError::Registry)?;
    let records: Vec<_> = declarations
        .into_iter()
        .map(|d| d.into_record(tenant_id, workflow_id))
        .collect();
    let n = records.len();
    trigger_store
        .put_many(&records)
        .await
        .map_err(|e| CompileError::Registry(format!("register triggers: {e}")))?;

    Ok((compiled, n))
}

/// Find the first `.so` (Linux) or `.dylib` (macOS) in a directory.
async fn find_cdylib(dir: &Path) -> Option<PathBuf> {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let mut rd = tokio::fs::read_dir(dir).await.ok()?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            return Some(path);
        }
    }
    None
}

/// SHA-256 of `bytes` as lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_stable_and_64_chars() {
        let a = sha256_hex(b"hello");
        let b = sha256_hex(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        // Known SHA-256 of "hello".
        assert_eq!(
            a,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn distinct_bytes_hash_distinctly() {
        assert_ne!(sha256_hex(b"a"), sha256_hex(b"b"));
    }

    #[test]
    fn compile_error_display_labels_stage() {
        assert!(CompileError::Transpile("x".into()).to_string().contains("transpile"));
        assert!(CompileError::Compile("x".into()).to_string().contains("compile"));
        assert!(CompileError::Registry("x".into()).to_string().contains("registration"));
    }
}
