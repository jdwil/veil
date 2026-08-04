use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use acr_core::ir::Algorithm;
use acr_core::trace::ExecutionTrace;

use crate::error::LibraryError;
use crate::metadata::{
    AlgorithmEntry, LibraryQuery, PromotionStatus, ScoreRecord, VersionInfo,
};

/// Trait for algorithm storage backends.
#[async_trait]
pub trait AlgorithmStore: Send + Sync {
    /// Store a new algorithm (creates entry + first version).
    async fn create(&self, algorithm: &Algorithm) -> Result<AlgorithmEntry, LibraryError>;
    /// Store a new version of an existing algorithm.
    async fn update(&self, algorithm: &Algorithm) -> Result<AlgorithmEntry, LibraryError>;
    /// Get the latest version of an algorithm.
    async fn get(&self, id: Uuid) -> Result<Algorithm, LibraryError>;
    /// Get a specific version.
    async fn get_version(&self, id: Uuid, version: u32) -> Result<Algorithm, LibraryError>;
    /// Get metadata entry.
    async fn get_entry(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError>;
    /// List all entries matching a query.
    async fn list(&self, query: &LibraryQuery) -> Result<Vec<AlgorithmEntry>, LibraryError>;
    /// Promote an algorithm to the active library.
    async fn promote(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError>;
    /// Retire an algorithm.
    async fn retire(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError>;
    /// Record a score for an algorithm version.
    async fn record_score(&self, id: Uuid, score: ScoreRecord) -> Result<(), LibraryError>;
    /// Store an execution trace.
    async fn store_trace(&self, trace: &ExecutionTrace) -> Result<String, LibraryError>;
    /// Get an execution trace by ID.
    async fn get_trace(&self, trace_id: &str) -> Result<ExecutionTrace, LibraryError>;
    /// Delete an algorithm and all versions.
    async fn delete(&self, id: Uuid) -> Result<(), LibraryError>;
    /// Get library status summary.
    async fn status(&self) -> Result<LibraryStatus, LibraryError>;
}

/// Summary of the library's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStatus {
    pub total_algorithms: usize,
    pub candidates: usize,
    pub promoted: usize,
    pub retired: usize,
    pub total_traces: usize,
}

/// Filesystem-backed implementation of `AlgorithmStore`.
///
/// Directory layout:
/// - `{base}/algorithms/{uuid}/entry.json` — AlgorithmEntry
/// - `{base}/algorithms/{uuid}/v{N}.json` — Algorithm at version N
/// - `{base}/traces/{trace_id}.json` — ExecutionTrace
pub struct FsStore {
    base_path: PathBuf,
}

impl FsStore {
    /// Create a new `FsStore`, creating directories if they don't exist.
    pub async fn new(base_path: PathBuf) -> Result<Self, LibraryError> {
        let algorithms_dir = base_path.join("algorithms");
        let traces_dir = base_path.join("traces");

        tokio::fs::create_dir_all(&algorithms_dir)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to create algorithms dir: {e}")))?;
        tokio::fs::create_dir_all(&traces_dir)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to create traces dir: {e}")))?;

        Ok(Self { base_path })
    }

    fn algorithm_dir(&self, id: Uuid) -> PathBuf {
        self.base_path.join("algorithms").join(id.to_string())
    }

    fn entry_path(&self, id: Uuid) -> PathBuf {
        self.algorithm_dir(id).join("entry.json")
    }

    fn version_path(&self, id: Uuid, version: u32) -> PathBuf {
        self.algorithm_dir(id).join(format!("v{version}.json"))
    }

    fn trace_path(&self, trace_id: &str) -> PathBuf {
        self.base_path.join("traces").join(format!("{trace_id}.json"))
    }

    async fn read_entry(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError> {
        let path = self.entry_path(id);
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LibraryError::NotFound(id)
                } else {
                    LibraryError::Storage(format!("Failed to read entry: {e}"))
                }
            })?;
        serde_json::from_str(&data)
            .map_err(|e| LibraryError::Serialization(format!("Failed to parse entry: {e}")))
    }

    async fn write_entry(&self, entry: &AlgorithmEntry) -> Result<(), LibraryError> {
        let path = self.entry_path(entry.id);
        let data = serde_json::to_string_pretty(entry)
            .map_err(|e| LibraryError::Serialization(format!("Failed to serialize entry: {e}")))?;
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to write entry: {e}")))
    }

    async fn write_algorithm(&self, algorithm: &Algorithm) -> Result<(), LibraryError> {
        let path = self.version_path(algorithm.id, algorithm.version);
        let data = serde_json::to_string_pretty(algorithm)
            .map_err(|e| LibraryError::Serialization(format!("Failed to serialize algorithm: {e}")))?;
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to write algorithm: {e}")))
    }
}

#[async_trait]
impl AlgorithmStore for FsStore {
    async fn create(&self, algorithm: &Algorithm) -> Result<AlgorithmEntry, LibraryError> {
        let dir = self.algorithm_dir(algorithm.id);

        // Check if already exists
        if dir.exists() {
            return Err(LibraryError::AlreadyExists(algorithm.id));
        }

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to create algorithm dir: {e}")))?;

        let now = Utc::now();
        let entry = AlgorithmEntry {
            id: algorithm.id,
            name: algorithm.name.clone(),
            description: algorithm.description.clone(),
            domain: algorithm.metadata.domain.clone(),
            tags: algorithm.metadata.tags.clone(),
            status: PromotionStatus::Candidate,
            current_version: algorithm.version,
            versions: vec![VersionInfo {
                version: algorithm.version,
                created_at: now,
                change_summary: "Initial version".to_string(),
            }],
            created_at: now,
            updated_at: now,
            score_history: Vec::new(),
        };

        self.write_entry(&entry).await?;
        self.write_algorithm(algorithm).await?;

        Ok(entry)
    }

    async fn update(&self, algorithm: &Algorithm) -> Result<AlgorithmEntry, LibraryError> {
        let mut entry = self.read_entry(algorithm.id).await?;

        let now = Utc::now();
        entry.current_version = algorithm.version;
        entry.name = algorithm.name.clone();
        entry.description = algorithm.description.clone();
        entry.domain = algorithm.metadata.domain.clone();
        entry.tags = algorithm.metadata.tags.clone();
        entry.updated_at = now;
        entry.versions.push(VersionInfo {
            version: algorithm.version,
            created_at: now,
            change_summary: format!("Updated to version {}", algorithm.version),
        });

        self.write_entry(&entry).await?;
        self.write_algorithm(algorithm).await?;

        Ok(entry)
    }

    async fn get(&self, id: Uuid) -> Result<Algorithm, LibraryError> {
        let entry = self.read_entry(id).await?;
        self.get_version(id, entry.current_version).await
    }

    async fn get_version(&self, id: Uuid, version: u32) -> Result<Algorithm, LibraryError> {
        let path = self.version_path(id, version);
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LibraryError::VersionNotFound { id, version }
                } else {
                    LibraryError::Storage(format!("Failed to read algorithm version: {e}"))
                }
            })?;
        serde_json::from_str(&data)
            .map_err(|e| LibraryError::Serialization(format!("Failed to parse algorithm: {e}")))
    }

    async fn get_entry(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError> {
        self.read_entry(id).await
    }

    async fn list(&self, query: &LibraryQuery) -> Result<Vec<AlgorithmEntry>, LibraryError> {
        let algorithms_dir = self.base_path.join("algorithms");
        let mut entries = Vec::new();

        let mut read_dir = tokio::fs::read_dir(&algorithms_dir)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to read algorithms dir: {e}")))?;

        while let Some(dir_entry) = read_dir.next_entry().await
            .map_err(|e| LibraryError::Storage(format!("Failed to read dir entry: {e}")))?
        {
            let entry_path = dir_entry.path().join("entry.json");
            if !entry_path.exists() {
                continue;
            }

            let data = tokio::fs::read_to_string(&entry_path)
                .await
                .map_err(|e| LibraryError::Storage(format!("Failed to read entry: {e}")))?;

            let entry: AlgorithmEntry = match serde_json::from_str(&data) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Apply filters
            if let Some(ref domain) = query.domain {
                if &entry.domain != domain {
                    continue;
                }
            }

            if let Some(ref status) = query.status {
                if &entry.status != status {
                    continue;
                }
            }

            if let Some(ref name_contains) = query.name_contains {
                if !entry.name.to_lowercase().contains(&name_contains.to_lowercase()) {
                    continue;
                }
            }

            if !query.tags.is_empty() {
                let has_all_tags = query.tags.iter().all(|t| entry.tags.contains(t));
                if !has_all_tags {
                    continue;
                }
            }

            entries.push(entry);
        }

        Ok(entries)
    }

    async fn promote(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError> {
        let mut entry = self.read_entry(id).await?;
        entry.status = PromotionStatus::Promoted;
        entry.updated_at = Utc::now();
        self.write_entry(&entry).await?;
        Ok(entry)
    }

    async fn retire(&self, id: Uuid) -> Result<AlgorithmEntry, LibraryError> {
        let mut entry = self.read_entry(id).await?;
        entry.status = PromotionStatus::Retired;
        entry.updated_at = Utc::now();
        self.write_entry(&entry).await?;
        Ok(entry)
    }

    async fn record_score(&self, id: Uuid, score: ScoreRecord) -> Result<(), LibraryError> {
        let mut entry = self.read_entry(id).await?;
        entry.score_history.push(score);
        entry.updated_at = Utc::now();
        self.write_entry(&entry).await
    }

    async fn store_trace(&self, trace: &ExecutionTrace) -> Result<String, LibraryError> {
        let trace_id = Uuid::new_v4().to_string();
        let path = self.trace_path(&trace_id);
        let data = serde_json::to_string_pretty(trace)
            .map_err(|e| LibraryError::Serialization(format!("Failed to serialize trace: {e}")))?;
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to write trace: {e}")))?;
        Ok(trace_id)
    }

    async fn get_trace(&self, trace_id: &str) -> Result<ExecutionTrace, LibraryError> {
        let path = self.trace_path(trace_id);
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LibraryError::Storage(format!("Trace not found: {trace_id}"))
                } else {
                    LibraryError::Storage(format!("Failed to read trace: {e}"))
                }
            })?;
        serde_json::from_str(&data)
            .map_err(|e| LibraryError::Serialization(format!("Failed to parse trace: {e}")))
    }

    async fn delete(&self, id: Uuid) -> Result<(), LibraryError> {
        let dir = self.algorithm_dir(id);
        if !dir.exists() {
            return Err(LibraryError::NotFound(id));
        }
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| LibraryError::Storage(format!("Failed to delete algorithm: {e}")))
    }

    async fn status(&self) -> Result<LibraryStatus, LibraryError> {
        let algorithms_dir = self.base_path.join("algorithms");
        let traces_dir = self.base_path.join("traces");

        let mut total_algorithms = 0;
        let mut candidates = 0;
        let mut promoted = 0;
        let mut retired = 0;

        if algorithms_dir.exists() {
            let mut read_dir = tokio::fs::read_dir(&algorithms_dir)
                .await
                .map_err(|e| LibraryError::Storage(format!("Failed to read algorithms dir: {e}")))?;

            while let Some(dir_entry) = read_dir.next_entry().await
                .map_err(|e| LibraryError::Storage(format!("Failed to read dir entry: {e}")))?
            {
                let entry_path = dir_entry.path().join("entry.json");
                if !entry_path.exists() {
                    continue;
                }

                let data = match tokio::fs::read_to_string(&entry_path).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let entry: AlgorithmEntry = match serde_json::from_str(&data) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                total_algorithms += 1;
                match entry.status {
                    PromotionStatus::Candidate => candidates += 1,
                    PromotionStatus::Promoted => promoted += 1,
                    PromotionStatus::Retired => retired += 1,
                }
            }
        }

        let mut total_traces = 0;
        if traces_dir.exists() {
            let mut read_dir = tokio::fs::read_dir(&traces_dir)
                .await
                .map_err(|e| LibraryError::Storage(format!("Failed to read traces dir: {e}")))?;

            while let Some(dir_entry) = read_dir.next_entry().await
                .map_err(|e| LibraryError::Storage(format!("Failed to read dir entry: {e}")))?
            {
                if dir_entry.path().extension().is_some_and(|ext| ext == "json") {
                    total_traces += 1;
                }
            }
        }

        Ok(LibraryStatus {
            total_algorithms,
            candidates,
            promoted,
            retired,
            total_traces,
        })
    }
}
