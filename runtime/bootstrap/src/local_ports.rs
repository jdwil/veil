//! CAP-004 / PVR-011: local adapters for generated `storage` ports.
//! Backed by the product projects hub (`projects_dir` git trees + sidecar meta).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use storage::domain::types::*;
use storage::ports::{MetadataStore, ObjectStorage};
use veil_shared::DomainError;

use crate::platform::GitRepo;

/// Object store under `{projects_dir}/.veil-object/` (keys preserved).
pub struct LocalObjectStorage {
    root: PathBuf,
}

impl LocalObjectStorage {
    pub fn new(projects_dir: impl Into<PathBuf>) -> Self {
        let root = projects_dir.into().join(".veil-object");
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        // Flat-safe: replace path seps that would escape
        let safe = key.trim_start_matches('/').replace("..", "_");
        self.root.join(safe)
    }
}

#[async_trait]
impl ObjectStorage for LocalObjectStorage {
    async fn put(&self, key: String, data: Vec<u8>) -> Result<(), DomainError> {
        let p = self.path_for(&key);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DomainError::External(e.to_string()))?;
        }
        // Also mirror repos/{name}/{branch}/{path} into project working tree
        if let Some((repo, rel)) = mirror_repo_key(&key) {
            let proj = projects_parent(&self.root).join(&repo).join(rel);
            if let Some(parent) = proj.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&proj, &data);
        }
        std::fs::write(&p, data).map_err(|e| DomainError::External(e.to_string()))
    }

    async fn get(&self, key: String) -> Result<Vec<u8>, DomainError> {
        let p = self.path_for(&key);
        if p.is_file() {
            return std::fs::read(&p).map_err(|e| DomainError::External(e.to_string()));
        }
        // Fallback: project working tree
        if let Some((repo, rel)) = mirror_repo_key(&key) {
            let proj = projects_parent(&self.root).join(&repo).join(rel);
            if proj.is_file() {
                return std::fs::read(&proj).map_err(|e| DomainError::External(e.to_string()));
            }
        }
        Err(DomainError::NotFound)
    }

    async fn delete(&self, key: String) -> Result<(), DomainError> {
        let p = self.path_for(&key);
        let _ = std::fs::remove_file(p);
        Ok(())
    }

    async fn exists(&self, key: String) -> Result<bool, DomainError> {
        Ok(self.path_for(&key).is_file())
    }

    async fn list(&self, prefix: String) -> Result<Vec<String>, DomainError> {
        let mut out = Vec::new();
        walk_keys(&self.root, &self.root, &prefix, &mut out);
        out.sort();
        Ok(out)
    }

    async fn size(&self, key: String) -> Result<i64, DomainError> {
        let p = self.path_for(&key);
        let meta = std::fs::metadata(p).map_err(|_| DomainError::NotFound)?;
        Ok(meta.len() as i64)
    }
}

fn projects_parent(object_root: &Path) -> PathBuf {
    object_root
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Parse `repos/{repo}/{branch}/{path...}` → (repo, path).
fn mirror_repo_key(key: &str) -> Option<(String, PathBuf)> {
    let parts: Vec<&str> = key.split('/').collect();
    if parts.len() < 4 || parts[0] != "repos" {
        return None;
    }
    let repo = parts[1].to_string();
    let path: PathBuf = parts[3..].iter().collect();
    if path.as_os_str().is_empty() {
        return None;
    }
    Some((repo, path))
}

fn walk_keys(dir: &Path, root: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_keys(&p, root, prefix, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let key = rel.to_string_lossy().replace('\\', "/");
            if key.starts_with(prefix) || prefix.is_empty() {
                out.push(key);
            }
        }
    }
}

/// Metadata store: projects hub + JSON sidecar for branches/commits/artifacts.
pub struct LocalMetadataStore {
    projects_dir: PathBuf,
    meta_path: PathBuf,
    inner: Mutex<MetaDb>,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct MetaDb {
    repos: HashMap<String, Repo>,
    branches: HashMap<String, BranchInfo>, // key: repo_id/name
    commits: HashMap<String, Vec<CommitInfo>>,
    artifacts: HashMap<String, ArtifactMetadata>,
    deployments: Vec<DeploymentRecord>,
    layers: HashMap<String, LayerMetadata>,
    stubs: HashMap<String, StubMetadata>,
    deps: Vec<DependencyEdge>,
}

impl LocalMetadataStore {
    pub fn new(projects_dir: impl Into<PathBuf>) -> Self {
        let projects_dir = projects_dir.into();
        let meta_path = projects_dir.join(".veil-meta.json");
        let inner = if meta_path.is_file() {
            std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            MetaDb::default()
        };
        Self {
            projects_dir,
            meta_path,
            inner: Mutex::new(inner),
        }
    }

    fn save(&self, db: &MetaDb) {
        if let Ok(s) = serde_json::to_string_pretty(db) {
            let _ = std::fs::write(&self.meta_path, s);
        }
    }

    fn branch_key(repo_id: &RepoId, name: &str) -> String {
        format!("{}/{}", repo_id.value, name)
    }

    /// Sync hub projects into repo index (id = name for product IDE).
    fn sync_hub(&self, db: &mut MetaDb) {
        if let Ok(list) = veil_server::list_projects(&self.projects_dir) {
            for p in list {
                let name = p.name.clone();
                db.repos.entry(name.clone()).or_insert_with(|| Repo {
                    id: RepoId {
                        value: name.clone(),
                    },
                    name: name.clone(),
                    description: None,
                    default_branch: "main".into(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                });
                let bk = format!("{name}/main");
                db.branches.entry(bk).or_insert_with(|| BranchInfo {
                    name: "main".into(),
                    head_commit: String::new(),
                    updated_at: Utc::now(),
                });
            }
        }
    }
}

#[async_trait]
impl MetadataStore for LocalMetadataStore {
    async fn create_repo(&self, metadata: Repo) -> Result<(), DomainError> {
        // Product hub: folder name is repo.name
        let _ = veil_server::create_project(&self.projects_dir, &metadata.name)
            .map_err(|e| DomainError::External(e))?;
        let mut db = self.inner.lock().unwrap();
        // Index under both UUID id and name for lookups
        db.repos.insert(metadata.id.value.clone(), metadata.clone());
        db.repos.insert(metadata.name.clone(), metadata.clone());
        self.save(&db);
        Ok(())
    }

    async fn get_repo(&self, id: RepoId) -> Result<Repo, DomainError> {
        let mut db = self.inner.lock().unwrap();
        self.sync_hub(&mut db);
        db.repos
            .get(&id.value)
            .cloned()
            .ok_or(DomainError::NotFound)
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, DomainError> {
        let mut db = self.inner.lock().unwrap();
        self.sync_hub(&mut db);
        // Prefer hub projects (unique by name)
        let mut by_name: HashMap<String, Repo> = HashMap::new();
        for r in db.repos.values() {
            by_name.insert(r.name.clone(), r.clone());
        }
        let mut v: Vec<Repo> = by_name.into_values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(v)
    }

    async fn delete_repo(&self, id: RepoId) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.repos.remove(&id.value);
        self.save(&db);
        Ok(())
    }

    async fn put_branch(&self, repo_id: RepoId, branch: BranchInfo) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.branches
            .insert(Self::branch_key(&repo_id, &branch.name), branch);
        self.save(&db);
        Ok(())
    }

    async fn get_branch(&self, repo_id: RepoId, name: String) -> Result<BranchInfo, DomainError> {
        let db = self.inner.lock().unwrap();
        db.branches
            .get(&Self::branch_key(&repo_id, &name))
            .cloned()
            .ok_or(DomainError::NotFound)
    }

    async fn list_branches(&self, repo_id: RepoId) -> Result<Vec<BranchInfo>, DomainError> {
        // Prefer real git branches when project dir exists
        let root = self.projects_dir.join(&repo_id.value);
        if root.is_dir() {
            if let Ok(list) = crate::platform::LocalGit.branches(&root) {
                return Ok(list
                    .into_iter()
                    .map(|name| BranchInfo {
                        name,
                        head_commit: String::new(),
                        updated_at: Utc::now(),
                    })
                    .collect());
            }
        }
        let db = self.inner.lock().unwrap();
        let prefix = format!("{}/", repo_id.value);
        Ok(db
            .branches
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, b)| b.clone())
            .collect())
    }

    async fn delete_branch(&self, repo_id: RepoId, name: String) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.branches.remove(&Self::branch_key(&repo_id, &name));
        self.save(&db);
        Ok(())
    }

    async fn put_tag(&self, _repo_id: RepoId, _tag: TagInfo) -> Result<(), DomainError> {
        Ok(())
    }
    async fn get_tag(&self, _repo_id: RepoId, _name: String) -> Result<TagInfo, DomainError> {
        Err(DomainError::NotFound)
    }
    async fn list_tags(&self, _repo_id: RepoId) -> Result<Vec<TagInfo>, DomainError> {
        Ok(vec![])
    }
    async fn delete_tag(&self, _repo_id: RepoId, _name: String) -> Result<(), DomainError> {
        Ok(())
    }

    async fn put_commit(&self, repo_id: RepoId, commit: CommitInfo) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.commits
            .entry(repo_id.value.clone())
            .or_default()
            .insert(0, commit);
        self.save(&db);
        Ok(())
    }

    async fn list_commits(
        &self,
        repo_id: RepoId,
        _branch: Option<String>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CommitInfo>, DomainError> {
        let root = self.projects_dir.join(&repo_id.value);
        if root.is_dir() {
            if let Ok(lines) = crate::platform::LocalGit.log(&root, limit.max(1) as usize) {
                let commits: Vec<CommitInfo> = lines
                    .into_iter()
                    .skip(offset.max(0) as usize)
                    .map(|line| {
                        let hash = line.split_whitespace().next().unwrap_or("").to_string();
                        CommitInfo {
                            hash,
                            message: line,
                            author: "git".into(),
                            timestamp: Utc::now(),
                            parent_hashes: vec![],
                            files_changed: vec![],
                        }
                    })
                    .collect();
                if !commits.is_empty() {
                    return Ok(commits);
                }
            }
        }
        let db = self.inner.lock().unwrap();
        let all = db.commits.get(&repo_id.value).cloned().unwrap_or_default();
        Ok(all
            .into_iter()
            .skip(offset.max(0) as usize)
            .take(limit.max(0) as usize)
            .collect())
    }

    async fn file_history(
        &self,
        repo_id: RepoId,
        _path: String,
        limit: i64,
    ) -> Result<Vec<CommitInfo>, DomainError> {
        self.list_commits(repo_id, None, limit, 0).await
    }

    async fn put_artifact(&self, artifact: ArtifactMetadata) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.artifacts.insert(artifact.id.value.clone(), artifact);
        self.save(&db);
        Ok(())
    }

    async fn get_artifact(&self, id: ArtifactId) -> Result<ArtifactMetadata, DomainError> {
        let db = self.inner.lock().unwrap();
        db.artifacts
            .get(&id.value)
            .cloned()
            .ok_or(DomainError::NotFound)
    }

    async fn find_artifact_by_hash(
        &self,
        content_hash: String,
        _target: CompilationTarget,
    ) -> Result<Option<ArtifactMetadata>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .artifacts
            .values()
            .find(|a| a.content_hash == content_hash)
            .cloned())
    }

    async fn list_artifacts(
        &self,
        repo_id: RepoId,
        branch: Option<String>,
    ) -> Result<Vec<ArtifactMetadata>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .artifacts
            .values()
            .filter(|a| {
                a.repo_id.value == repo_id.value
                    && branch.as_ref().map(|b| &a.branch == b).unwrap_or(true)
            })
            .cloned()
            .collect())
    }

    async fn put_deployment(&self, record: DeploymentRecord) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.deployments.push(record);
        self.save(&db);
        Ok(())
    }

    async fn list_deployments(
        &self,
        artifact_id: ArtifactId,
    ) -> Result<Vec<DeploymentRecord>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .deployments
            .iter()
            .filter(|d| d.artifact_id.value == artifact_id.value)
            .cloned()
            .collect())
    }

    async fn put_layer(&self, layer: LayerMetadata) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.layers.insert(layer.name.clone(), layer);
        self.save(&db);
        Ok(())
    }

    async fn get_layer(&self, name: String) -> Result<LayerMetadata, DomainError> {
        let db = self.inner.lock().unwrap();
        db.layers.get(&name).cloned().ok_or(DomainError::NotFound)
    }

    async fn list_layers(&self) -> Result<Vec<LayerMetadata>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db.layers.values().cloned().collect())
    }

    async fn put_stub(&self, stub: StubMetadata) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.stubs.insert(stub.crate_name.clone(), stub);
        self.save(&db);
        Ok(())
    }

    async fn get_stub(&self, crate_name: String) -> Result<StubMetadata, DomainError> {
        let db = self.inner.lock().unwrap();
        db.stubs
            .get(&crate_name)
            .cloned()
            .ok_or(DomainError::NotFound)
    }

    async fn list_stubs(&self) -> Result<Vec<StubMetadata>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db.stubs.values().cloned().collect())
    }

    async fn put_dependency(&self, edge: DependencyEdge) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.deps.push(edge);
        self.save(&db);
        Ok(())
    }

    async fn get_dependencies(&self, repo_id: RepoId) -> Result<Vec<DependencyEdge>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .deps
            .iter()
            .filter(|d| d.dependent.value == repo_id.value)
            .cloned()
            .collect())
    }

    async fn get_dependents(&self, dependency: String) -> Result<Vec<DependencyEdge>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .deps
            .iter()
            .filter(|d| d.dependency == dependency)
            .cloned()
            .collect())
    }
}

/// Build storage Deps wired to local ports under projects_dir.
pub fn storage_deps() -> storage::application::Deps {
    let dir = crate::platform::projects_dir();
    storage::application::Deps {
        metadata_store: std::sync::Arc::new(LocalMetadataStore::new(&dir)),
        object_storage: std::sync::Arc::new(LocalObjectStorage::new(&dir)),
    }
}


// ─── Extensions Deps (VEIL-generated File* adapters — no residual registry) ─
// Product logic: runtime.veil → extensions::application
// IO: FileExtension* → ExtStore (veil_ext_store stub). Backend:
// VEIL_EXTENSIONS_BACKEND=file|ddb (default file). Never raw DdbClient in VEIL.

use extensions::adapters::{
    FileExtensionArtifactStore, FileExtensionExecutor, FileExtensionRegistry,
    FileExtensionSourceStore,
};
use extensions::domain::types::ExtensionRecord;

/// Resolve extensions data dir: `VEIL_EXTENSIONS_DIR` or `{projects_dir}/.veil-extensions`.
pub fn extensions_dir() -> PathBuf {
    if let Ok(d) = std::env::var("VEIL_EXTENSIONS_DIR") {
        return PathBuf::from(d);
    }
    crate::platform::projects_dir().join(".veil-extensions")
}

/// Well-known stock IDs for dual-loop pins (fixtures only).
pub fn stock_activate_members_id() -> uuid::Uuid {
    uuid::Uuid::parse_str("aaaaaaaa-0001-4000-8000-000000000001").unwrap()
}
pub fn stock_guard_end_id() -> uuid::Uuid {
    uuid::Uuid::parse_str("aaaaaaaa-0002-4000-8000-000000000002").unwrap()
}

/// Wire VEIL-generated File* adapters (ExtStore facade — file backend by default).
pub fn extensions_deps() -> extensions::application::Deps {
    let dir = extensions_dir().to_string_lossy().to_string();
    let _ = std::fs::create_dir_all(&dir);
    // Default dual-loop: file backend (no AWS). Deploy sets VEIL_EXTENSIONS_BACKEND=ddb.
    if std::env::var("VEIL_EXTENSIONS_BACKEND").is_err() {
        std::env::set_var("VEIL_EXTENSIONS_BACKEND", "file");
    }
    extensions::application::Deps {
        extension_artifact_store: std::sync::Arc::new(FileExtensionArtifactStore {
            dir: dir.clone(),
        }),
        extension_executor: std::sync::Arc::new(FileExtensionExecutor { dir: dir.clone() }),
        extension_registry: std::sync::Arc::new(FileExtensionRegistry { dir: dir.clone() }),
        extension_source_store: std::sync::Arc::new(FileExtensionSourceStore { dir }),
    }
}

/// Call VEIL EnsureStockCatalog with fixture IDs.
pub async fn ensure_stock_catalog_veil(
    deps: &extensions::application::Deps,
) -> Result<Vec<ExtensionRecord>, DomainError> {
    extensions::application::ensure_stock_catalog(
        deps,
        stock_activate_members_id(),
        stock_guard_end_id(),
        Some("wear_test".into()),
    )
    .await
}
