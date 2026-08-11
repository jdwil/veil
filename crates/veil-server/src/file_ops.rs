//! Shared project file create/list helpers for the HTTP API and agent tools.

use std::path::{Path, PathBuf};

use crate::provider::{FileInfo, FileKind, SourceProvider};

/// Result of creating a package or layer in the project.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CreatedFile {
    pub index: usize,
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub content: String,
    pub files: Vec<FileInfo>,
}

#[derive(Debug)]
pub enum CreateFileError {
    BadRequest(String),
    Conflict(String),
    Forbidden(String),
    Internal(String),
}

impl CreateFileError {
    pub fn message(&self) -> &str {
        match self {
            Self::BadRequest(s) | Self::Conflict(s) | Self::Forbidden(s) | Self::Internal(s) => s,
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Conflict(_) => 409,
            Self::Forbidden(_) => 403,
            Self::Internal(_) => 500,
        }
    }
}

pub fn sanitize_new_file_name(
    raw: &str,
    kind_hint: Option<&str>,
) -> Result<(String, FileKind), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("name is required".into());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err("name must be a single file name (no path separators)".into());
    }
    let lower = trimmed.to_ascii_lowercase();
    let (stem, kind) = if lower.ends_with(".veil") {
        (trimmed[..trimmed.len() - 5].to_string(), FileKind::Package)
    } else if lower.ends_with(".layer") {
        (trimmed[..trimmed.len() - 6].to_string(), FileKind::Layer)
    } else {
        let k = match kind_hint.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("layer") => FileKind::Layer,
            _ => FileKind::Package,
        };
        (trimmed.to_string(), k)
    };
    if stem.is_empty() {
        return Err("file name is empty after stripping extension".into());
    }
    if !stem
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("name may only contain letters, digits, _ and -".into());
    }
    let filename = match kind {
        FileKind::Layer => format!("{stem}.layer"),
        FileKind::Package | FileKind::Stub => format!("{stem}.veil"),
    };
    Ok((filename, kind))
}

pub fn default_package_source(stem: &str) -> String {
    format!("pkg {stem}\n  use ddd\n\n  # New package — add constructs here\n")
}

/// Scaffold for reaction-mode packages: bare package that loads reaction.layer.
/// Palette vocabulary comes from the layer via `use reaction` (VEIL design).
pub fn reaction_package_source(stem: &str) -> String {
    format!("pkg {stem}\n  use reaction\n")
}

/// True when this serve root is the reaction hub project (or VEIL_IDE_MODE=reaction).
pub fn is_reaction_ide_context(project_root: Option<&Path>) -> bool {
    if std::env::var("VEIL_IDE_MODE")
        .map(|v| v.eq_ignore_ascii_case("reaction"))
        .unwrap_or(false)
    {
        return true;
    }
    project_root
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("reaction"))
        .unwrap_or(false)
}

fn line_is_use_reaction(line: &str) -> bool {
    let t = line.trim();
    t == "use reaction" || t.starts_with("use reaction ")
}

/// Package source has a `use reaction` line.
pub fn source_has_use_reaction(source: &str) -> bool {
    source.lines().any(line_is_use_reaction)
}

/// Reject removing the locked `use reaction` line (reaction IDE mode).
pub fn check_use_reaction_locked(previous: &str, next: &str) -> Result<(), String> {
    if source_has_use_reaction(previous) && !source_has_use_reaction(next) {
        return Err(
            "use reaction is required for reaction packages and cannot be removed \
             (palette is locked to reaction.layer)"
                .into(),
        );
    }
    Ok(())
}

/// Ensure a package source includes `use reaction` (insert after first `use` / `pkg` block).
pub fn ensure_use_reaction(source: &str) -> String {
    if source_has_use_reaction(source) {
        return source.to_string();
    }
    let mut out = String::new();
    let mut inserted = false;
    for line in source.lines() {
        out.push_str(line);
        out.push('\n');
        if !inserted {
            let t = line.trim();
            if t.starts_with("use ") || t.starts_with("pkg ") {
                // insert after first use or after pkg line before blank
                if t.starts_with("use ") {
                    out.push_str("  use reaction\n");
                    inserted = true;
                }
            }
        }
    }
    if !inserted {
        // No use lines — inject after pkg line if present
        let mut out2 = String::new();
        let mut done = false;
        for line in source.lines() {
            out2.push_str(line);
            out2.push('\n');
            if !done && line.trim().starts_with("pkg ") {
                out2.push_str("  use reaction\n");
                done = true;
            }
        }
        if done {
            return out2;
        }
        return format!("use reaction\n{source}");
    }
    out
}

pub fn default_layer_source(stem: &str) -> String {
    format!(
        "pkg {stem} v1\n  desc \"{stem} language layer\"\n  author \"VEIL\"\n\n  construct Example\n    kw example\n    mt struct\n    desc \"Starter construct — rename me\"\n    visual\n      icon \"📦\"\n      color \"#6366f1\"\n      label \"Example\"\n    group domain\n\n  prompt\n    You are authoring packages that use the `{stem}` layer.\n    Prefer layer keywords; keep platform packages as dependencies.\n"
    )
}

/// Create a package/layer under the project, register it, and select it.
pub async fn create_file_in_project<P: SourceProvider + ?Sized>(
    state: &P,
    name: &str,
    kind_hint: Option<&str>,
    content: Option<String>,
) -> Result<CreatedFile, CreateFileError> {
    let (filename, kind) = sanitize_new_file_name(name, kind_hint)
        .map_err(CreateFileError::BadRequest)?;

    // Platform language packs (ddd, di, …) are read-only — fork under a new name.
    if kind == FileKind::Layer {
        let stem = Path::new(&filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if veil_ir::is_platform_layer_name(stem) {
            return Err(CreateFileError::Forbidden(format!(
                "refusing to create platform layer '{stem}.layer' in a product project — \
                 platform packs are read-only (resolve from VEIL_LAYERS_DIR / platform catalog). \
                 To customize, copy as e.g. 'acme-{stem}.layer' and `use acme-{stem}`."
            )));
        }
    }

    let dir = if let Some(root) = state.project_root() {
        root
    } else {
        let files = state.list_files().await;
        files
            .iter()
            .find_map(|f| {
                let p = Path::new(&f.path);
                p.parent().map(|d| d.to_path_buf())
            })
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let path = dir.join(&filename);
    if path.exists() {
        return Err(CreateFileError::Conflict(format!(
            "{} already exists",
            path.display()
        )));
    }

    if let Some(root) = state.project_root() {
        let root_c = root.canonicalize().unwrap_or(root.clone());
        let parent_c = path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_else(|| dir.clone());
        if !parent_c.starts_with(&root_c) && parent_c != root_c {
            return Err(CreateFileError::Forbidden(
                "refusing to create file outside project root".into(),
            ));
        }
    }

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "pkg".into());
    let reaction_ctx = is_reaction_ide_context(state.project_root().as_deref());
    let content = content.unwrap_or_else(|| match kind {
        FileKind::Layer => default_layer_source(&stem),
        FileKind::Package | FileKind::Stub if reaction_ctx => reaction_package_source(&stem),
        FileKind::Package | FileKind::Stub => default_package_source(&stem),
    });

    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(CreateFileError::Internal(e.to_string()));
    }
    if let Err(e) = std::fs::write(&path, &content) {
        return Err(CreateFileError::Internal(e.to_string()));
    }

    let idx = match state.register_file(path.clone(), content.clone(), true) {
        Ok(i) => i,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(CreateFileError::Internal(e));
        }
    };
    if let Err(e) = state.set_active(idx) {
        return Err(CreateFileError::Internal(e));
    }

    crate::revision::bus().publish(content.len(), &path.to_string_lossy(), "create_file");

    let files = state.list_files().await;
    Ok(CreatedFile {
        index: idx,
        name: filename,
        path: path.to_string_lossy().to_string(),
        kind,
        content,
        files,
    })
}

/// Detect accidental HTTP/tool envelopes written as source file bodies.
///
/// `ws_read` / platform `read_file` return JSON like `{"path":"main.veil","content":"pkg …"}`.
/// Agents sometimes pass that entire JSON to `ws_write` / `write_source`, which produces
/// `parse error at 0-1: expected Sol, got LBrace` on the next IDE `/ir` load.
///
/// Returns the inner `content` when the body is clearly that envelope; otherwise `None`.
pub fn unwrap_tool_content_envelope(body: &str) -> Option<String> {
    let trimmed = body.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let obj = v.as_object()?;
    // Allow only the keys used by read responses (not arbitrary JSON objects).
    const ALLOWED: &[&str] = &["content", "path", "ok", "bytes", "repo"];
    if obj.is_empty() || !obj.keys().all(|k| ALLOWED.contains(&k.as_str())) {
        return None;
    }
    let inner = obj.get("content")?.as_str()?;
    if inner.is_empty() {
        return None;
    }
    // Nested JSON is not VEIL/layer source we want to unwrap further here.
    let inner_trim = inner.trim_start();
    if inner_trim.starts_with('{') {
        return None;
    }
    // Prefer shapes that look like product source (not prose notes mistaken for envelopes).
    let looks_like_source = inner_trim.starts_with("pkg ")
        || inner_trim.starts_with('#')
        || inner_trim.starts_with("name =")
        || inner_trim.starts_with("use ")
        || inner_trim.contains("\npkg ")
        || inner_trim.contains("\n  use ");
    if !looks_like_source {
        // Still unwrap when a path field points at a known source extension.
        let path = obj.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let lower = path.to_ascii_lowercase();
        if !(lower.ends_with(".veil") || lower.ends_with(".layer") || lower.ends_with(".toml")) {
            return None;
        }
    }
    Some(inner.to_string())
}

/// Normalize a write/read body: unwrap tool envelopes when present.
pub fn normalize_source_body(body: &str) -> String {
    match unwrap_tool_content_envelope(body) {
        Some(inner) => {
            tracing::warn!(
                outer_bytes = body.len(),
                inner_bytes = inner.len(),
                "unwrapped accidental {{content,path}} tool envelope from source body"
            );
            inner
        }
        None => body.to_string(),
    }
}

/// True when `rel` is a VEIL product source path that should reject/unwrap envelopes.
pub fn is_veil_source_rel(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".veil") || lower.ends_with(".layer") || lower.ends_with("veil.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_default_extension() {
        let (n, k) = sanitize_new_file_name("AcmeWear", Some("package")).unwrap();
        assert_eq!(n, "AcmeWear.veil");
        assert!(matches!(k, FileKind::Package));
    }

    #[test]
    fn layer_extension() {
        let (n, k) = sanitize_new_file_name("wear_test", Some("layer")).unwrap();
        assert_eq!(n, "wear_test.layer");
        assert!(matches!(k, FileKind::Layer));
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(sanitize_new_file_name("../etc/passwd", None).is_err());
        assert!(sanitize_new_file_name("a/b.veil", None).is_err());
    }

    #[test]
    fn explicit_extension_wins() {
        let (n, k) = sanitize_new_file_name("x.layer", Some("package")).unwrap();
        assert_eq!(n, "x.layer");
        assert!(matches!(k, FileKind::Layer));
    }

    #[test]
    fn unwraps_ws_read_envelope() {
        let envelope = r#"{"path":"main.veil","content":"pkg AgentRegistry\n  use ddd\n"}"#;
        let inner = unwrap_tool_content_envelope(envelope).expect("unwrap");
        assert!(inner.starts_with("pkg AgentRegistry"));
        assert!(!inner.starts_with('{'));
    }

    #[test]
    fn leaves_raw_veil_alone() {
        let src = "pkg Foo\n  use ddd\n";
        assert!(unwrap_tool_content_envelope(src).is_none());
        assert_eq!(normalize_source_body(src), src);
    }

    #[test]
    fn ignores_unrelated_json() {
        let j = r#"{"nodes":[{"id":1}],"edges":[]}"#;
        assert!(unwrap_tool_content_envelope(j).is_none());
    }
}
