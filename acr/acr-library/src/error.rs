use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("Algorithm not found: {0}")]
    NotFound(Uuid),
    #[error("Version not found: {id} v{version}")]
    VersionNotFound { id: Uuid, version: u32 },
    #[error("Algorithm already exists: {0}")]
    AlreadyExists(Uuid),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}
