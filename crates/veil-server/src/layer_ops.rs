//! Platform layer catalog (read-only language packs: ddd, di, …).
//!
//! Mirrors [`crate::stub_ops`]: S3 body + DDB META pointer, materialize to a
//! process cache. Product packages never edit these; teams fork under a new name.
//!
//! | Store | Key |
//! |-------|-----|
//! | S3 | `layers/platform/{name}/{version}.layer` |
//! | DDB | `PK=LAYER#{name}` `SK=META` with `{ name, version, s3_key, … }` |
//!
//! Local resolve also honors `VEIL_LAYERS_DIR` and monorepo `layers/` (see
//! `veil_ir::platform_layers`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use veil_ir::{content_fingerprint, is_platform_layer_name, platform_layers_cache_dir};

/// Resolved platform layers directory for this process.
static PLATFORM_LAYERS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Ensure platform layers are available for `use ddd` etc.
///
/// Order:
/// 1. Already-set `VEIL_LAYERS_DIR`
/// 2. Monorepo `layers/` (if found)
/// 3. Materialize from DDB META + S3 → `$TMP/veil-platform-layers`
pub fn ensure_platform_layer_cache() {
    let _ = platform_layers_dir();
}

/// Platform catalog directory used by resolution.
pub fn platform_layers_dir() -> Option<PathBuf> {
    if let Some(p) = PLATFORM_LAYERS_DIR.get() {
        return Some(p.clone());
    }
    if let Ok(dir) = std::env::var("VEIL_LAYERS_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            let _ = PLATFORM_LAYERS_DIR.set(p.clone());
            return Some(p);
        }
    }
    if let Some(mono) = find_monorepo_layers_dir() {
        let _ = PLATFORM_LAYERS_DIR.set(mono.clone());
        return Some(mono);
    }
    let cache = platform_layers_cache_dir();
    match materialize_platform_layers(&cache) {
        Ok(n) if n > 0 => {
            // Ensure VEIL_LAYERS_DIR is visible to veil_ir without set_var when possible:
            // platform_layer_dirs() already checks the cache path.
            let _ = PLATFORM_LAYERS_DIR.set(cache.clone());
            Some(cache)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("platform layer materialize failed: {e}");
            None
        }
    }
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

/// Canonical S3 key for a platform layer body.
pub fn platform_layer_s3_key(name: &str, version: &str) -> String {
    let ver = sanitize_version(version);
    format!("layers/platform/{name}/{ver}.layer")
}

fn sanitize_version(version: &str) -> String {
    let ver = version.trim();
    if ver.is_empty() || ver == "*" {
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

fn find_monorepo_layers_dir() -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        for anc in cwd.ancestors() {
            let p = anc.join("layers");
            if p.join("ddd.layer").is_file() {
                return Some(p);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in ["../../../layers", "../../layers", "../layers", "layers"] {
                let p = dir.join(rel);
                if p.join("ddd.layer").is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Pull platform layers: DDB `LAYER#*/META` (s3_key) → S3 get → local cache dir.
pub fn materialize_platform_layers(dest: &Path) -> Result<usize, String> {
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
            r#"{":p":{"S":"LAYER#"},":sk":{"S":"META"}}"#,
            "--output",
            "json",
        ])
        .output()
        .map_err(|e| format!("aws dynamodb scan: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ddb scan LAYER META failed: {}",
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
        let Some(name) = pk.strip_prefix("LAYER#") else {
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
                Some(platform_layer_s3_key(name, ver))
            });

        let dest_path = dest.join(format!("{name}.layer"));
        if dest_path.is_file()
            && std::fs::metadata(&dest_path)
                .map(|m| m.len() > 64)
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
    }
    Ok(n)
}

/// List platform layer stems present in the active catalog dir (for APIs / agents).
pub fn list_platform_layer_names() -> Vec<String> {
    ensure_platform_layer_cache();
    let mut names = Vec::new();
    for dir in veil_ir::platform_layer_dirs() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("layer") {
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|n| n == stem) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    names
}

/// Fingerprint helper for seed scripts / diagnostics.
pub fn layer_file_fingerprint(path: &Path) -> Result<(usize, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let fp = content_fingerprint(text.as_ref());
    Ok((bytes.len(), fp))
}

/// Whether `name` is a platform layer that products must not hand-edit as `layers/{name}.layer`.
pub fn is_readonly_platform_layer(name: &str) -> bool {
    is_platform_layer_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_key_shape() {
        assert_eq!(
            platform_layer_s3_key("ddd", "1.0.0"),
            "layers/platform/ddd/1.0.0.layer"
        );
        assert_eq!(
            platform_layer_s3_key("ddd", ""),
            "layers/platform/ddd/latest.layer"
        );
    }
}
