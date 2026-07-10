//! File-backed metadata store MVP (RT-011).
//!
//! Schema: one JSON document per entity under `{root}/meta/{kind}/{id}.json`.
//! Migration: wipe-ok for MVP (document in README).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

use crate::StorageError;

#[derive(Debug, Clone)]
pub struct FileMetaStore {
    root: PathBuf,
}

impl FileMetaStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let root = root.as_ref().join("meta");
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn default_local() -> Result<Self, StorageError> {
        if let Ok(dir) = std::env::var("VEIL_DATA_DIR") {
            return Self::open(dir);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self::open(PathBuf::from(home).join(".veil"))
    }

    fn path(&self, kind: &str, id: &str) -> Result<PathBuf, StorageError> {
        if kind.contains("..") || id.contains("..") || kind.contains('/') || id.contains('/') {
            return Err(StorageError::InvalidKey(format!("{kind}/{id}")));
        }
        Ok(self.root.join(kind).join(format!("{id}.json")))
    }

    pub fn put<T: Serialize>(&self, kind: &str, id: &str, value: &T) -> Result<(), StorageError> {
        let path = self.path(kind, id)?;
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|e| StorageError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        fs::write(path, bytes)?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<T, StorageError> {
        let path = self.path(kind, id)?;
        let bytes = fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(format!("{kind}/{id}"))
            } else {
                StorageError::Io(e)
            }
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|e| StorageError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    pub fn delete(&self, kind: &str, id: &str) -> Result<(), StorageError> {
        let path = self.path(kind, id)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Io(e)),
        }
    }

    pub fn list_ids(&self, kind: &str) -> Result<Vec<String>, StorageError> {
        let dir = self.root.join(kind);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_suffix(".json") {
                ids.push(id.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct RepoMeta {
        name: String,
        branch: String,
    }

    #[test]
    fn crud_roundtrip() {
        let dir = tempdir().unwrap();
        let store = FileMetaStore::open(dir.path()).unwrap();
        let m = RepoMeta {
            name: "app".into(),
            branch: "main".into(),
        };
        store.put("repo", "r1", &m).unwrap();
        let got: RepoMeta = store.get("repo", "r1").unwrap();
        assert_eq!(got, m);
        assert_eq!(store.list_ids("repo").unwrap(), vec!["r1".to_string()]);
        store.delete("repo", "r1").unwrap();
        assert!(store.get::<RepoMeta>("repo", "r1").is_err());
    }
}
