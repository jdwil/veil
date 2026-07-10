//! Local-first platform adapters (RT-010 / RT-012).
//!
//! Default when `VEIL_STORAGE=fs` or no cloud credentials.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid key: {0}")]
    InvalidKey(String),
}

/// Filesystem-backed object storage (RT-010).
#[derive(Debug, Clone)]
pub struct FsObjectStore {
    root: PathBuf,
}

impl FsObjectStore {
    /// Create/open store under `root` (created if missing).
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Default local root: `VEIL_DATA_DIR` or `~/.veil/objects`.
    pub fn default_local() -> Result<Self, StorageError> {
        if let Ok(dir) = std::env::var("VEIL_DATA_DIR") {
            return Self::open(PathBuf::from(dir).join("objects"));
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self::open(PathBuf::from(home).join(".veil/objects"))
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        if key.is_empty() || key.contains("..") || key.starts_with('/') {
            return Err(StorageError::InvalidKey(key.into()));
        }
        Ok(self.root.join(key))
    }

    pub fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let path = self.path_for(key)?;
        fs::read(&path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                StorageError::NotFound(key.into())
            } else {
                StorageError::Io(e)
            }
        })
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        let path = self.path_for(key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Io(e)),
        }
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let mut out = Vec::new();
        let base = self.root.clone();
        fn walk(dir: &Path, base: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
            if !dir.exists() {
                return Ok(());
            }
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, prefix, out)?;
                } else if let Ok(rel) = path.strip_prefix(base) {
                    let key = rel.to_string_lossy().replace('\\', "/");
                    if key.starts_with(prefix) {
                        out.push(key);
                    }
                }
            }
            Ok(())
        }
        walk(&self.root, &base, prefix, &mut out)?;
        out.sort();
        Ok(out)
    }

    /// Put content-addressed blob; returns `sha256:<hex>` key (RT-012).
    pub fn put_addressed(&self, bytes: &[u8]) -> Result<String, StorageError> {
        let hash = content_hash(bytes);
        let key = format!("sha256/{hash}");
        self.put(&key, bytes)?;
        Ok(format!("sha256:{hash}"))
    }
}

/// SHA-256 hex digest of `bytes` (RT-012).
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fs_put_get_list_delete() {
        let dir = tempdir().unwrap();
        let store = FsObjectStore::open(dir.path()).unwrap();
        store.put("a/b.txt", b"hello").unwrap();
        assert_eq!(store.get("a/b.txt").unwrap(), b"hello");
        let keys = store.list("a/").unwrap();
        assert_eq!(keys, vec!["a/b.txt".to_string()]);
        store.delete("a/b.txt").unwrap();
        assert!(matches!(store.get("a/b.txt"), Err(StorageError::NotFound(_))));
    }

    #[test]
    fn content_address_stable() {
        let dir = tempdir().unwrap();
        let store = FsObjectStore::open(dir.path()).unwrap();
        let k1 = store.put_addressed(b"payload").unwrap();
        let k2 = store.put_addressed(b"payload").unwrap();
        assert_eq!(k1, k2);
        assert!(k1.starts_with("sha256:"));
        let hex = k1.strip_prefix("sha256:").unwrap();
        assert_eq!(hex.len(), 64);
        assert_eq!(content_hash(b"payload"), hex);
    }

    #[test]
    fn rejects_path_escape() {
        let dir = tempdir().unwrap();
        let store = FsObjectStore::open(dir.path()).unwrap();
        assert!(store.put("../x", b"no").is_err());
    }
}
