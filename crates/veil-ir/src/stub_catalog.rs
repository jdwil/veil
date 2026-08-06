//! Stub resolution catalog: project-local → platform (`VEIL_STUBS_DIR` / monorepo).
//!
//! Ownership model:
//! - **Platform catalog** — shared common SDKs (runtime/src/stubs, VEIL_STUBS_DIR, DDB seed).
//! - **Project `stubs/`** — pins, overrides, product-specific crates (wins on conflict).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layer::{parse_stub_file, StubCrate};

/// Where a resolved stub body came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StubOrigin {
    /// `project/name.stub` or package-adjacent.
    ProjectAdjacent,
    /// `project/stubs/name.stub`
    ProjectStubsDir,
    /// `VEIL_STUBS_DIR` or monorepo / installed platform catalog.
    Platform,
    /// In-memory / DDB content passed by the host (not on local disk).
    RemoteCatalog,
}

impl StubOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProjectAdjacent => "project_adjacent",
            Self::ProjectStubsDir => "project_stubs",
            Self::Platform => "platform",
            Self::RemoteCatalog => "remote_catalog",
        }
    }
}

/// One catalog entry (list view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StubCatalogEntry {
    pub name: String,
    pub version: String,
    pub origin: StubOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub sparse: bool,
    pub version_unpinned: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    pub generated: bool,
}

/// Full resolve result with body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedStub {
    pub entry: StubCatalogEntry,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<StubCrate>,
}

fn entry_from_parsed(
    stub: &StubCrate,
    origin: StubOrigin,
    path: Option<PathBuf>,
) -> StubCatalogEntry {
    StubCatalogEntry {
        name: stub.name.clone(),
        version: stub.version.clone(),
        origin,
        path: path.map(|p| p.display().to_string()),
        sparse: stub.is_sparse(),
        version_unpinned: stub.version_unpinned(),
        notes: stub.freshness_notes(),
        surface: stub.provenance.surface.clone(),
        generated_at: stub.provenance.generated_at.clone(),
        generated: stub.provenance.generated,
    }
}

/// Normalize crate use-name for file lookup (`aws-sdk-s3` ↔ `aws_sdk_s3`).
pub fn stub_file_stems(name: &str) -> Vec<String> {
    let n = name.trim();
    if n.is_empty() {
        return Vec::new();
    }
    let mut out = vec![n.to_string()];
    let unders = n.replace('-', "_");
    let dashed = n.replace('_', "-");
    if unders != n {
        out.push(unders);
    }
    if dashed != n && !out.iter().any(|s| s == &dashed) {
        out.push(dashed);
    }
    out
}

/// Platform catalog directories to scan (existing + env).
pub fn platform_stub_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("VEIL_STUBS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            dirs.push(p);
        }
    }
    let cache = std::env::temp_dir().join("veil-platform-stubs");
    if cache.is_dir() && !dirs.iter().any(|d| d == &cache) {
        dirs.push(cache);
    }
    if let Ok(layers) = std::env::var("VEIL_LAYERS_DIR") {
        let p = Path::new(&layers).join("../runtime/src/stubs");
        if p.is_dir() {
            dirs.push(p);
        }
        let p = Path::new(&layers).join("../examples");
        if p.is_dir() {
            dirs.push(p);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for anc in cwd.ancestors() {
            for rel in ["runtime/src/stubs", "examples", "stubs"] {
                let p = anc.join(rel);
                if p.is_dir() && !dirs.iter().any(|d| d == &p) {
                    dirs.push(p);
                }
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for rel in ["../runtime/src/stubs", "stubs", "../stubs"] {
                let p = exe_dir.join(rel);
                if p.is_dir() && !dirs.iter().any(|d| d == &p) {
                    dirs.push(p);
                }
            }
        }
    }
    dirs
}

fn read_stub_file(path: &Path) -> Option<(String, StubCrate)> {
    let content = std::fs::read_to_string(path).ok()?;
    let parsed = parse_stub_file(&content)?;
    Some((content, parsed))
}

/// Resolve a stub by crate use-name. Project root first, then platform.
pub fn resolve_stub(project_root: Option<&Path>, name: &str) -> Option<ResolvedStub> {
    let stems = stub_file_stems(name);
    if stems.is_empty() {
        return None;
    }

    if let Some(root) = project_root {
        for stem in &stems {
            let adj = root.join(format!("{stem}.stub"));
            if let Some((content, parsed)) = read_stub_file(&adj) {
                return Some(ResolvedStub {
                    entry: entry_from_parsed(&parsed, StubOrigin::ProjectAdjacent, Some(adj)),
                    content,
                    parsed: Some(parsed),
                });
            }
            let sub = root.join("stubs").join(format!("{stem}.stub"));
            if let Some((content, parsed)) = read_stub_file(&sub) {
                return Some(ResolvedStub {
                    entry: entry_from_parsed(&parsed, StubOrigin::ProjectStubsDir, Some(sub)),
                    content,
                    parsed: Some(parsed),
                });
            }
        }
    }

    for dir in platform_stub_dirs() {
        for stem in &stems {
            let p = dir.join(format!("{stem}.stub"));
            if let Some((content, parsed)) = read_stub_file(&p) {
                return Some(ResolvedStub {
                    entry: entry_from_parsed(&parsed, StubOrigin::Platform, Some(p)),
                    content,
                    parsed: Some(parsed),
                });
            }
        }
    }
    None
}

/// List project stubs (adjacent + stubs/) without platform entries.
pub fn list_project_stubs(project_root: &Path) -> Vec<StubCatalogEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // stubs/ directory
    let stubs_dir = project_root.join("stubs");
    if stubs_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&stubs_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("stub") {
                    continue;
                }
                if let Some((_, parsed)) = read_stub_file(&p) {
                    if seen.insert(parsed.name.clone()) {
                        out.push(entry_from_parsed(
                            &parsed,
                            StubOrigin::ProjectStubsDir,
                            Some(p),
                        ));
                    }
                }
            }
        }
    }

    // package-adjacent at project root
    if let Ok(rd) = std::fs::read_dir(project_root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("stub") {
                continue;
            }
            if let Some((_, parsed)) = read_stub_file(&p) {
                if seen.insert(parsed.name.clone()) {
                    out.push(entry_from_parsed(
                        &parsed,
                        StubOrigin::ProjectAdjacent,
                        Some(p),
                    ));
                }
            }
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// List platform catalog stubs (may include monorepo `runtime/src/stubs`).
pub fn list_platform_stubs() -> Vec<StubCatalogEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in platform_stub_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("stub") {
                continue;
            }
            if let Some((_, parsed)) = read_stub_file(&p) {
                if seen.insert(parsed.name.clone()) {
                    out.push(entry_from_parsed(&parsed, StubOrigin::Platform, Some(p)));
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Combined catalog: project entries first (override), then platform-only names.
pub fn list_catalog(project_root: Option<&Path>) -> Vec<StubCatalogEntry> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(root) = project_root {
        for e in list_project_stubs(root) {
            seen.insert(e.name.clone());
            out.push(e);
        }
    }
    for e in list_platform_stubs() {
        if seen.insert(e.name.clone()) {
            out.push(e);
        }
    }
    out
}

/// Build the standard generated header block for `veil stub-gen` output.
pub fn generated_stub_header(
    crate_name: &str,
    version: &str,
    features: &[String],
    fingerprint: Option<&str>,
) -> String {
    let now = chrono_like_now();
    let mut h = format!("stub {crate_name} {version}\n");
    h.push_str("  # @generated veil-stub-gen 1\n");
    h.push_str("  # source crates.io\n");
    h.push_str("  # surface full\n");
    h.push_str(&format!("  # cargo_name {crate_name}\n"));
    h.push_str(&format!("  # generated_at {now}\n"));
    if let Some(fp) = fingerprint {
        if !fp.is_empty() {
            h.push_str(&format!("  # rustdoc_fingerprint {fp}\n"));
        }
    }
    if !features.is_empty() {
        h.push_str(&format!("  # features {}\n", features.join(",")));
    }
    h.push_str(
        "  # Auto-inferred codegen policy from rustdoc (do not hand-edit; re-run veil stub-gen)\n",
    );
    h
}

/// Cheap content fingerprint (not crypto-strong — identity for staleness hints).
pub fn content_fingerprint(s: &str) -> String {
    // FNV-1a 64-bit
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn chrono_like_now() -> String {
    // Avoid chrono dep in veil-ir: UTC-ish via SystemTime
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format as approximate ISO date from unix seconds (good enough for provenance)
    const DAY: u64 = 86400;
    let days = secs / DAY;
    let mut y = 1970u64;
    let mut rem = days;
    loop {
        let diy = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if rem < diy {
            break;
        }
        rem -= diy;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let md = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1u64;
    for &dim in &md {
        if rem < dim {
            break;
        }
        rem -= dim;
        m += 1;
    }
    let d = rem + 1;
    let tod = secs % DAY;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Path where install/gen should write: `project/stubs/<name>.stub`.
pub fn project_stub_write_path(project_root: &Path, crate_name: &str) -> PathBuf {
    let stem = crate_name.replace('-', "_");
    project_root.join("stubs").join(format!("{stem}.stub"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meta_from_generated_header() {
        let src = r#"stub reqwest 0.13.4
  # @generated veil-stub-gen 1
  # source crates.io
  # surface full
  # cargo_name reqwest
  # generated_at 2026-08-06T12:00:00Z
  # rustdoc_fingerprint deadbeef
  cargo_features default

  struct Client
    fn new() -> Client
"#;
        let s = parse_stub_file(src).expect("parse");
        assert_eq!(s.name, "reqwest");
        assert_eq!(s.version, "0.13.4");
        assert!(s.provenance.generated);
        assert_eq!(s.provenance.surface.as_deref(), Some("full"));
        assert_eq!(s.provenance.source.as_deref(), Some("crates.io"));
        assert_eq!(
            s.provenance.rustdoc_fingerprint.as_deref(),
            Some("deadbeef")
        );
        assert!(!s.version_unpinned());
    }

    #[test]
    fn curated_not_sparse() {
        let src = r#"stub tiny 1.0.0
  surface curated
  struct Client
    fn new() -> Client
"#;
        let s = parse_stub_file(src).expect("parse");
        assert!(!s.is_sparse());
        assert_eq!(s.provenance.surface.as_deref(), Some("curated"));
    }
}
