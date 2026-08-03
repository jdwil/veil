//! Filesystem helpers for VEIL adapters. Called from generated code via the
//! `veil_local_fs` stub — **not** inlined into the VEIL engine (MISSION).

use std::path::{Path, PathBuf};

/// Error type that converts via `?` into generated `DomainError::External`
/// (Display → External string) when adapters use `Res!` methods.
#[derive(Debug)]
pub struct FsError(pub String);

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FsError {}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> Self {
        FsError(e.to_string())
    }
}

/// Static helpers (associated fns) matching `runtime/src/stubs/veil_local_fs.stub`.
pub struct LocalFs;

impl LocalFs {
    pub fn create_dir_all(path: impl AsRef<str>) -> Result<(), FsError> {
        std::fs::create_dir_all(path.as_ref())?;
        Ok(())
    }

    pub fn write(path: impl AsRef<str>, data: impl AsRef<str>) -> Result<(), FsError> {
        let p = Path::new(path.as_ref());
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, data.as_ref().as_bytes())?;
        Ok(())
    }

    pub fn read(path: impl AsRef<str>) -> Result<String, FsError> {
        Ok(std::fs::read_to_string(path.as_ref())?)
    }

    pub fn path_exists(path: impl AsRef<str>) -> bool {
        Path::new(path.as_ref()).exists()
    }

    pub fn path_is_file(path: impl AsRef<str>) -> bool {
        Path::new(path.as_ref()).is_file()
    }

    pub fn list_dir(path: impl AsRef<str>) -> Result<Vec<String>, FsError> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(path.as_ref())? {
            let e = e?;
            out.push(e.file_name().to_string_lossy().to_string());
        }
        out.sort();
        Ok(out)
    }

    /// List only regular files ending in `.json` (extension record files).
    pub fn list_json_files(path: impl AsRef<str>) -> Result<Vec<String>, FsError> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(path.as_ref())? {
            let e = e?;
            let p = e.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".json") {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn join(a: impl AsRef<str>, b: impl AsRef<str>) -> String {
        let mut p = PathBuf::from(a.as_ref());
        p.push(b.as_ref());
        p.to_string_lossy().to_string()
    }

    /// Clone-friendly wrappers used when generated code moves Strings into calls.
    pub fn join_owned(a: String, b: String) -> String {
        Self::join(a, b)
    }

    /// Get the projects directory from env or config.
    pub fn projects_dir() -> String {
        if let Ok(dir) = std::env::var("VEIL_PROJECTS_DIR") {
            return dir;
        }
        // Try ~/.veil/config.json
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let cfg_path = format!("{home}/.veil/config.json");
        if let Ok(contents) = std::fs::read_to_string(&cfg_path) {
            // Minimal JSON parse for projects_dir field
            if let Some(start) = contents.find("\"projects_dir\"") {
                let rest = &contents[start..];
                if let Some(colon) = rest.find(':') {
                    let after = rest[colon + 1..].trim();
                    if after.starts_with('"') {
                        if let Some(end) = after[1..].find('"') {
                            return after[1..1 + end].to_string();
                        }
                    }
                }
            }
        }
        format!("{home}/veil-projects")
    }

    /// Read a project's deploy.toml as a string.
    pub fn read_project_deploy(slug: impl AsRef<str>) -> Result<String, FsError> {
        let dir = Self::projects_dir();
        let path = format!("{}/{}/deploy.toml", dir, slug.as_ref());
        if Path::new(&path).is_file() {
            return Ok(std::fs::read_to_string(&path)?);
        }
        // Also try config/deploy.toml
        let alt = format!("{}/{}/config/deploy.toml", dir, slug.as_ref());
        Ok(std::fs::read_to_string(&alt)?)
    }

    /// Read a TOML file and return its content as a JSON string.
    pub fn read_toml_json(path: impl AsRef<str>) -> Result<String, FsError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        // Simple pass-through: return the raw TOML content.
        // In production, would convert TOML → JSON via a toml crate.
        Ok(content)
    }

    /// List deploy unit names for a project (directories under deploy/ or names from deploy.toml).
    pub fn project_unit_names(slug: impl AsRef<str>) -> Result<Vec<String>, FsError> {
        let dir = Self::projects_dir();
        let units_dir = format!("{}/{}/deploy", dir, slug.as_ref());
        if Path::new(&units_dir).is_dir() {
            return Self::list_dir(&units_dir);
        }
        // Fallback: return empty list
        Ok(Vec::new())
    }

    /// Get the deploy unit type for a named unit in a project.
    pub fn project_unit_type(slug: impl AsRef<str>, name: impl AsRef<str>) -> Result<String, FsError> {
        let dir = Self::projects_dir();
        let type_file = format!("{}/{}/deploy/{}/type", dir, slug.as_ref(), name.as_ref());
        if Path::new(&type_file).is_file() {
            return Ok(std::fs::read_to_string(&type_file)?.trim().to_string());
        }
        // Default type
        Ok("lambda-api".to_string())
    }
}
