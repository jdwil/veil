//! Platform + project stub catalog operations (list / get / generate / install).
//!
//! Resolution: project-local → platform (`VEIL_STUBS_DIR` / monorepo / remote catalog).
//! **Platform remote catalog:** S3 body + DDB META pointer (not DDB CONTENT — size limits).
//!
//! | Store | Key |
//! |-------|-----|
//! | S3 | `stubs/platform/{name}/{version}.stub` |
//! | DDB | `PK=STUB#{name}` `SK=META` with `{ name, version, s3_key, … }` |
//!
//! Agents must use generate/install — never hand-author full SDK stubs.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde::Serialize;
use veil_ir::{
    content_fingerprint, list_catalog, list_platform_stubs, list_project_stubs, parse_stub_file,
    project_stub_write_path, resolve_stub, stub_file_stems, ResolvedStub, StubCatalogEntry,
    StubOrigin,
};

/// Resolved platform stubs directory for this process (avoids unsafe set_var).
static PLATFORM_STUBS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Ensure platform stubs are available via `VEIL_STUBS_DIR` or process cache.
///
/// Order:
/// 1. Already-set `VEIL_STUBS_DIR`
/// 2. Monorepo `stubs/` (if found)
/// 3. Materialize from DDB META + S3 (`stubs/platform/…`) → `$TMP/veil-platform-stubs`
pub fn ensure_platform_stub_cache() {
    let _ = platform_stubs_dir();
}

/// Platform catalog directory used by resolution (sets process-local cache).
pub fn platform_stubs_dir() -> Option<PathBuf> {
    if let Some(p) = PLATFORM_STUBS_DIR.get() {
        return Some(p.clone());
    }
    if let Ok(dir) = std::env::var("VEIL_STUBS_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            let _ = PLATFORM_STUBS_DIR.set(p.clone());
            return Some(p);
        }
    }
    if let Some(mono) = find_monorepo_stubs_dir() {
        let _ = PLATFORM_STUBS_DIR.set(mono.clone());
        return Some(mono);
    }
    let cache = default_cache_dir();
    match materialize_platform_stubs(&cache) {
        Ok(n) if n > 0 => {
            let _ = PLATFORM_STUBS_DIR.set(cache.clone());
            Some(cache)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("platform stub materialize failed: {e}");
            None
        }
    }
}

fn default_cache_dir() -> PathBuf {
    std::env::temp_dir().join("veil-platform-stubs")
}

fn aws_env() -> (String, String, String) {
    let profile = std::env::var("AWS_PROFILE").unwrap_or_default();
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-west-2".into());
    let table = std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
    (profile, region, table)
}

fn aws_cli() -> Command {
    let mut cmd = Command::new("aws");
    let (profile, region, _) = aws_env();
    if !profile.is_empty() {
        cmd.env("AWS_PROFILE", profile);
    }
    cmd.env("AWS_REGION", region);
    cmd
}

fn s3_bucket() -> Option<String> {
    std::env::var("BUCKET")
        .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
        .ok()
        .filter(|s| !s.is_empty())
}

/// Sanitize version for use as an S3 key segment.
pub fn sanitize_stub_version(version: &str) -> String {
    let ver = version.trim();
    if ver.is_empty() || ver == "*" || ver.starts_with("path:") || ver.starts_with('#') {
        return "latest".into();
    }
    if ver
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        && ver
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        ver.to_string()
    } else {
        "latest".into()
    }
}

/// Canonical S3 key for a platform stub body.
pub fn platform_stub_s3_key(name: &str, version: &str) -> String {
    let ver = sanitize_stub_version(version);
    // Keep name as-is (underscores/dashes from stub header).
    format!("stubs/platform/{name}/{ver}.stub")
}

fn find_monorepo_stubs_dir() -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        for anc in cwd.ancestors() {
            let p = anc.join("stubs");
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["../stubs", "../../stubs"] {
                let p = dir.join(rel);
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Pull platform stubs: DDB `STUB#*/META` (s3_key) → S3 get → local cache dir.
///
/// Fallback: legacy `STUB#*/CONTENT` inline (small stubs only).
pub fn materialize_platform_stubs(dest: &Path) -> Result<usize, String> {
    let table = match std::env::var("VEIL_DDB_TABLE") {
        Ok(t) if !t.is_empty() => t,
        _ => return Ok(0),
    };
    let bucket = s3_bucket();
    std::fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let out = aws_cli()
        .args([
            "dynamodb",
            "scan",
            "--table-name",
            &table,
            "--filter-expression",
            "begins_with(PK, :p) AND SK = :sk",
            "--expression-attribute-values",
            r#"{":p":{"S":"STUB#"},":sk":{"S":"META"}}"#,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws dynamodb scan: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ddb scan META failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("ddb json: {e}"))?;
    let mut n = 0usize;
    for item in v
        .get("Items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let pk = item
            .pointer("/PK/S")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let Some(name) = pk.strip_prefix("STUB#") else {
            continue;
        };
        let data_str = item
            .pointer("/data/S")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        let meta: serde_json::Value = serde_json::from_str(data_str).unwrap_or_default();
        let s3_key = meta
            .get("s3_key")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                let ver = meta
                    .get("version")
                    .and_then(|x| x.as_str())
                    .unwrap_or("latest");
                Some(platform_stub_s3_key(name, ver))
            });

        let dest_path = dest.join(format!("{name}.stub"));
        // Skip if already present and non-empty
        if dest_path.is_file()
            && std::fs::metadata(&dest_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            n += 1;
            continue;
        }

        if let (Some(bucket), Some(key)) = (bucket.as_ref(), s3_key.as_ref()) {
            let uri = format!("s3://{bucket}/{key}");
            let cp = aws_cli()
                .args(["s3", "cp", &uri, dest_path.to_str().unwrap_or("")])
                .output()
                .map_err(|e| format!("aws s3 cp: {e}"))?;
            if cp.status.success() {
                n += 1;
                continue;
            }
            tracing::debug!(
                "s3 cp {} failed: {}",
                uri,
                String::from_utf8_lossy(&cp.stderr)
            );
        }

        // Legacy fallback: inline CONTENT (small stubs only)
        if let Ok(content) = get_legacy_stub_content(name, &table) {
            if !content.is_empty() {
                std::fs::write(&dest_path, &content)
                    .map_err(|e| format!("write {}: {e}", dest_path.display()))?;
                n += 1;
            }
        }
    }
    Ok(n)
}

fn get_legacy_stub_content(name: &str, table: &str) -> Result<String, String> {
    let key = serde_json::json!({
        "PK": {"S": format!("STUB#{name}")},
        "SK": {"S": "CONTENT"},
    });
    let key_s = serde_json::to_string(&key).map_err(|e| e.to_string())?;
    let out = aws_cli()
        .args([
            "dynamodb",
            "get-item",
            "--table-name",
            table,
            "--key",
            &key_s,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("get-item: {e}"))?;
    if !out.status.success() {
        return Ok(String::new());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    Ok(v.pointer("/Item/data/S")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string())
}

/// Publish a stub to the platform catalog: S3 body + DDB META pointer.
pub fn put_platform_stub(crate_name: &str, content: &str) -> Result<String, String> {
    let bucket = s3_bucket().ok_or_else(|| "BUCKET or VEIL_S3_BUCKET required".to_string())?;
    let table = std::env::var("VEIL_DDB_TABLE")
        .map_err(|_| "VEIL_DDB_TABLE required".to_string())?;
    let parsed = parse_stub_file(content);
    let version = parsed
        .as_ref()
        .map(|p| p.version.clone())
        .unwrap_or_else(|| "*".into());
    let name = parsed
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| crate_name.to_string());
    let s3_key = platform_stub_s3_key(&name, &version);
    let uri = format!("s3://{bucket}/{s3_key}");

    // Write temp file then s3 cp (handles large bodies)
    let tmp = std::env::temp_dir().join(format!("veil-stub-put-{name}.stub"));
    std::fs::write(&tmp, content).map_err(|e| format!("temp write: {e}"))?;
    let cp = aws_cli()
        .args(["s3", "cp", tmp.to_str().unwrap_or(""), &uri])
        .output()
        .map_err(|e| format!("aws s3 cp: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !cp.status.success() {
        return Err(format!(
            "s3 put failed: {}",
            String::from_utf8_lossy(&cp.stderr)
        ));
    }

    let fp = content_fingerprint(content);
    let meta = serde_json::json!({
        "name": name,
        "version": version,
        "s3_key": s3_key,
        "bytes": content.len(),
        "fingerprint": fp,
        "generated": parsed.as_ref().map(|p| p.provenance.generated).unwrap_or(false),
        "surface": parsed.as_ref().and_then(|p| p.provenance.surface.clone()),
        "sparse": parsed.as_ref().map(|p| p.is_sparse()).unwrap_or(true),
        "provenance": parsed.as_ref().map(|p| &p.provenance),
    });
    let meta_item = serde_json::json!({
        "PK": {"S": format!("STUB#{name}")},
        "SK": {"S": "META"},
        "data": {"S": meta.to_string()},
    });
    let meta_s = serde_json::to_string(&meta_item).map_err(|e| e.to_string())?;
    let put = aws_cli()
        .args([
            "dynamodb",
            "put-item",
            "--table-name",
            &table,
            "--item",
            &meta_s,
        ])
        .output()
        .map_err(|e| format!("aws put-item META: {e}"))?;
    if !put.status.success() {
        return Err(format!(
            "ddb META put failed: {}",
            String::from_utf8_lossy(&put.stderr)
        ));
    }

    // Drop legacy CONTENT if present (best-effort; avoids stale huge items)
    let del_key = serde_json::json!({
        "PK": {"S": format!("STUB#{name}")},
        "SK": {"S": "CONTENT"},
    });
    if let Ok(del_s) = serde_json::to_string(&del_key) {
        let _ = aws_cli()
            .args([
                "dynamodb",
                "delete-item",
                "--table-name",
                &table,
                "--key",
                &del_s,
            ])
            .output();
    }

    Ok(uri)
}

/// @deprecated name — use [`put_platform_stub`].
pub fn put_stub_to_ddb(crate_name: &str, content: &str) -> Result<(), String> {
    put_platform_stub(crate_name, content).map(|_| ())
}

#[derive(Debug, Serialize)]
pub struct StubCatalogResponse {
    pub project: Vec<StubCatalogEntry>,
    pub platform: Vec<StubCatalogEntry>,
    pub combined: Vec<StubCatalogEntry>,
    pub stubs_dir: Option<String>,
}

fn list_dir_stubs(dir: &Path, origin: StubOrigin) -> Vec<StubCatalogEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("stub") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Some(parsed) = parse_stub_file(&content) else {
            continue;
        };
        out.push(StubCatalogEntry {
            name: parsed.name.clone(),
            version: parsed.version.clone(),
            origin,
            path: Some(p.display().to_string()),
            sparse: parsed.is_sparse(),
            version_unpinned: parsed.version_unpinned(),
            notes: parsed.freshness_notes(),
            surface: parsed.provenance.surface.clone(),
            generated_at: parsed.provenance.generated_at.clone(),
            generated: parsed.provenance.generated,
        });
    }
    out
}

/// Platform entries: env/monorepo walk + process cache (DDB materialize).
fn platform_entries() -> Vec<StubCatalogEntry> {
    let mut out = list_platform_stubs();
    let mut seen: std::collections::HashSet<String> =
        out.iter().map(|e| e.name.clone()).collect();
    if let Some(dir) = PLATFORM_STUBS_DIR.get() {
        for e in list_dir_stubs(dir, StubOrigin::Platform) {
            if seen.insert(e.name.clone()) {
                out.push(e);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn catalog_json(project_root: Option<&Path>) -> StubCatalogResponse {
    ensure_platform_stub_cache();
    let project = project_root.map(list_project_stubs).unwrap_or_default();
    let platform = platform_entries();
    let mut combined = list_catalog(project_root);
    let mut seen: std::collections::HashSet<String> =
        combined.iter().map(|e| e.name.clone()).collect();
    for e in &platform {
        if seen.insert(e.name.clone()) {
            combined.push(e.clone());
        }
    }
    StubCatalogResponse {
        project,
        platform,
        combined,
        stubs_dir: PLATFORM_STUBS_DIR
            .get()
            .map(|p| p.display().to_string())
            .or_else(|| std::env::var("VEIL_STUBS_DIR").ok()),
    }
}

pub fn get_stub(project_root: Option<&Path>, name: &str) -> Result<ResolvedStub, String> {
    ensure_platform_stub_cache();
    if let Some(r) = resolve_stub(project_root, name) {
        return Ok(r);
    }
    // Process platform cache (e.g. DDB-materialized) not always on VEIL_STUBS_DIR env
    if let Some(dir) = PLATFORM_STUBS_DIR.get() {
        for stem in stub_file_stems(name) {
            let p = dir.join(format!("{stem}.stub"));
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Some(parsed) = parse_stub_file(&content) {
                    return Ok(ResolvedStub {
                        entry: StubCatalogEntry {
                            name: parsed.name.clone(),
                            version: parsed.version.clone(),
                            origin: StubOrigin::Platform,
                            path: Some(p.display().to_string()),
                            sparse: parsed.is_sparse(),
                            version_unpinned: parsed.version_unpinned(),
                            notes: parsed.freshness_notes(),
                            surface: parsed.provenance.surface.clone(),
                            generated_at: parsed.provenance.generated_at.clone(),
                            generated: parsed.provenance.generated,
                        },
                        content,
                        parsed: Some(parsed),
                    });
                }
            }
        }
    }
    Err(format!(
        "no stub for '{name}' — run stub_gen / POST /stubs/generate, or install from platform catalog"
    ))
}

/// Copy platform (or resolved) stub into `project/stubs/<name>.stub`.
pub fn install_stub_to_project(project_root: &Path, name: &str) -> Result<ResolvedStub, String> {
    ensure_platform_stub_cache();
    let norm = |s: &str| s.replace('-', "_");
    let name_n = norm(name);

    let mut content = None;
    let mut src_path = String::new();
    for e in platform_entries() {
        if e.name == name || norm(&e.name) == name_n {
            if let Some(path) = &e.path {
                if let Ok(c) = std::fs::read_to_string(path) {
                    src_path = path.clone();
                    content = Some(c);
                    break;
                }
            }
        }
    }
    if content.is_none() {
        if let Ok(r) = get_stub(None, name) {
            if matches!(
                r.entry.origin,
                StubOrigin::Platform | StubOrigin::RemoteCatalog
            ) {
                src_path = r.entry.path.clone().unwrap_or_default();
                content = Some(r.content);
            }
        }
    }
    let content = content.ok_or_else(|| format!("platform stub '{name}' not found"))?;

    let dest = project_stub_write_path(project_root, name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create stubs/: {e}"))?;
    }
    std::fs::write(&dest, &content).map_err(|e| format!("write {}: {e}", dest.display()))?;
    let parsed = parse_stub_file(&content);
    let mut notes = parsed
        .as_ref()
        .map(|p| p.freshness_notes())
        .unwrap_or_default();
    if !src_path.is_empty() {
        notes.push(format!("installed from platform path {src_path}"));
    }
    notes.push(
        "Stub is auto-loaded from project stubs/ on next check (no `use` line required). Run veil_check to verify escape_external_call clears."
            .into(),
    );
    Ok(ResolvedStub {
        entry: StubCatalogEntry {
            name: parsed
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| name.to_string()),
            version: parsed
                .as_ref()
                .map(|p| p.version.clone())
                .unwrap_or_else(|| "*".into()),
            origin: veil_ir::StubOrigin::ProjectStubsDir,
            path: Some(dest.display().to_string()),
            sparse: parsed.as_ref().map(|p| p.is_sparse()).unwrap_or(true),
            version_unpinned: parsed
                .as_ref()
                .map(|p| p.version_unpinned())
                .unwrap_or(true),
            notes,
            surface: parsed.as_ref().and_then(|p| p.provenance.surface.clone()),
            generated_at: parsed
                .as_ref()
                .and_then(|p| p.provenance.generated_at.clone()),
            generated: parsed
                .as_ref()
                .map(|p| p.provenance.generated)
                .unwrap_or(false),
        },
        content,
        parsed,
    })
}

/// Run `veil stub-gen` and write into project stubs/ (or return body only).
pub fn generate_stub(
    project_root: Option<&Path>,
    crate_name: &str,
    features: &[String],
    write: bool,
) -> Result<ResolvedStub, String> {
    ensure_platform_stub_cache();
    let crate_name = crate_name.trim();
    if crate_name.is_empty() {
        return Err("crate_name required".into());
    }
    if !crate_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("crate_name may only contain letters, digits, _ and -".into());
    }

    let tmp_out = std::env::temp_dir().join(format!("veil-stub-gen-{crate_name}.stub"));
    let mut args = vec![
        "stub-gen".to_string(),
        crate_name.to_string(),
        "-o".to_string(),
        tmp_out.display().to_string(),
    ];
    if !features.is_empty() {
        args.push("--features".into());
        args.push(features.join(","));
    }

    run_veil_stub_gen(&args)?;

    let content =
        std::fs::read_to_string(&tmp_out).map_err(|e| format!("read generated stub: {e}"))?;
    let parsed = parse_stub_file(&content);

    let dest_path = if write {
        let root = project_root.ok_or_else(|| {
            "write=true requires an open project (project root)".to_string()
        })?;
        let dest = project_stub_write_path(root, crate_name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create stubs/: {e}"))?;
        }
        std::fs::write(&dest, &content).map_err(|e| format!("write {}: {e}", dest.display()))?;
        Some(dest)
    } else {
        None
    };

    Ok(ResolvedStub {
        entry: StubCatalogEntry {
            name: parsed
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| crate_name.to_string()),
            version: parsed
                .as_ref()
                .map(|p| p.version.clone())
                .unwrap_or_else(|| "*".into()),
            origin: if dest_path.is_some() {
                veil_ir::StubOrigin::ProjectStubsDir
            } else {
                veil_ir::StubOrigin::RemoteCatalog
            },
            path: dest_path.map(|p| p.display().to_string()),
            sparse: parsed.as_ref().map(|p| p.is_sparse()).unwrap_or(true),
            version_unpinned: parsed
                .as_ref()
                .map(|p| p.version_unpinned())
                .unwrap_or(true),
            notes: parsed
                .as_ref()
                .map(|p| p.freshness_notes())
                .unwrap_or_default(),
            surface: parsed.as_ref().and_then(|p| p.provenance.surface.clone()),
            generated_at: parsed
                .as_ref()
                .and_then(|p| p.provenance.generated_at.clone()),
            generated: true,
        },
        content,
        parsed,
    })
}

fn run_veil_stub_gen(args: &[String]) -> Result<(), String> {
    // 1) `veil` on PATH
    if let Ok(out) = Command::new("veil").args(args).output() {
        if out.status.success() {
            return Ok(());
        }
        // fall through if binary exists but failed — show error below after trying cargo
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.contains("No such file") {
            // try cargo anyway for monorepo dev
            if try_cargo_stub_gen(args).is_ok() {
                return Ok(());
            }
            return Err(format!("veil stub-gen failed: {err}"));
        }
    }
    try_cargo_stub_gen(args)
}

fn try_cargo_stub_gen(args: &[String]) -> Result<(), String> {
    // Find monorepo Cargo.toml
    let mut cargo_toml = None;
    if let Ok(cwd) = std::env::current_dir() {
        for anc in cwd.ancestors() {
            let p = anc.join("Cargo.toml");
            if p.is_file() && anc.join("crates/veil-cli").is_dir() {
                cargo_toml = Some(p);
                break;
            }
        }
    }
    let Some(manifest) = cargo_toml else {
        return Err(
            "veil binary not found and monorepo Cargo.toml not located — install veil CLI or set PATH"
                .into(),
        );
    };
    let mut cmd_args = vec![
        "run".to_string(),
        "--quiet".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
        "-p".to_string(),
        "veil-cli".to_string(),
        "--".to_string(),
    ];
    cmd_args.extend(args.iter().cloned());
    let out = Command::new("cargo")
        .args(&cmd_args)
        .output()
        .map_err(|e| format!("cargo run veil-cli: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo veil stub-gen failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Tool-facing text summary for agents.
pub fn tool_list_text(project_root: Option<&Path>) -> String {
    let cat = catalog_json(project_root);
    let mut lines = vec![
        "Stub catalog (project overrides platform)".into(),
        format!("platform dir: {:?}", cat.stubs_dir),
        String::new(),
        "## Project".into(),
    ];
    if cat.project.is_empty() {
        lines.push("(none)".into());
    } else {
        for e in &cat.project {
            lines.push(format_entry(e));
        }
    }
    lines.push(String::new());
    lines.push("## Platform".into());
    if cat.platform.is_empty() {
        lines.push(
            "(none — seed via scripts/seed-stubs-platform.sh → S3 + DDB META)".into(),
        );
    } else {
        for e in &cat.platform {
            lines.push(format_entry(e));
        }
    }
    lines.push(String::new());
    lines.push(
        "Rules: NEVER hand-write full SDK stubs. Use stub_gen (rustdoc) or stub_install from platform."
            .into(),
    );
    lines.join("\n")
}

fn format_entry(e: &StubCatalogEntry) -> String {
    let flags = [
        if e.sparse { "sparse" } else { "" },
        if e.version_unpinned { "unpinned" } else { "" },
        if e.generated { "generated" } else { "" },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(",");
    format!(
        "- {} @ {} [{}] {}{}",
        e.name,
        e.version,
        e.origin.as_str(),
        if flags.is_empty() {
            String::new()
        } else {
            format!("({flags}) ")
        },
        e.path.as_deref().unwrap_or("")
    )
}
