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
}
