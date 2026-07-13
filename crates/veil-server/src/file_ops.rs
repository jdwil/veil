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
    let content = content.unwrap_or_else(|| match kind {
        FileKind::Layer => default_layer_source(&stem),
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
}
