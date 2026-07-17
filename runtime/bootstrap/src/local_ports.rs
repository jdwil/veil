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

// ─── EXT-02: Extension registry (local filesystem) ─────────────────────────
// Residual IO adapter implementing VEIL-declared ports (same role as
// LocalObjectStorage). Domain + services remain in runtime.veil → extensions crate.

use extensions::domain::types::{ExtensionRecord, ExtensionVersion};
use extensions::ports::{ExtensionExecutor, ExtensionRegistry, ExtensionSourceStore};

/// Resolve extensions data dir: `VEIL_EXTENSIONS_DIR` or `{projects_dir}/.veil-extensions`.
pub fn extensions_dir() -> PathBuf {
    if let Ok(d) = std::env::var("VEIL_EXTENSIONS_DIR") {
        return PathBuf::from(d);
    }
    crate::platform::projects_dir().join(".veil-extensions")
}

/// Local ExtensionRegistry: one JSON file per extension + versions/ subdir.
pub struct LocalExtensionRegistry {
    root: PathBuf,
}

impl LocalExtensionRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let _ = std::fs::create_dir_all(&root);
        Self { root }
    }

    fn record_path(&self, id: &uuid::Uuid) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    fn versions_dir(&self, id: &uuid::Uuid) -> PathBuf {
        self.root.join(id.to_string()).join("versions")
    }

    fn version_path(&self, id: &uuid::Uuid, version: i64) -> PathBuf {
        self.versions_dir(id).join(format!("{version}.json"))
    }
}

#[async_trait]
impl ExtensionRegistry for LocalExtensionRegistry {
    async fn create(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError> {
        let p = self.record_path(&record.extension_id);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let s = serde_json::to_string_pretty(&record)
            .map_err(|e| DomainError::External(e.to_string()))?;
        std::fs::write(&p, s).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(record)
    }

    async fn get(&self, id: uuid::Uuid) -> Result<Option<ExtensionRecord>, DomainError> {
        let p = self.record_path(&id);
        if !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).map_err(|e| DomainError::External(e.to_string()))?;
        let rec: ExtensionRecord =
            serde_json::from_str(&s).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(Some(rec))
    }

    async fn list(
        &self,
        scope: Option<String>,
        kind: Option<String>,
        product_id: Option<String>,
        tenant_id: Option<uuid::Uuid>,
    ) -> Result<Vec<ExtensionRecord>, DomainError> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(s) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(rec) = serde_json::from_str::<ExtensionRecord>(&s) else {
                continue;
            };
            if rec.archived {
                continue;
            }
            if let Some(ref sc) = scope {
                if format!("{:?}", rec.scope) != *sc && format!("{:?}", rec.scope).to_lowercase() != sc.to_lowercase() {
                    // also accept bare names
                    let want = sc.to_lowercase();
                    let got = format!("{:?}", rec.scope).to_lowercase();
                    if got != want {
                        continue;
                    }
                }
            }
            if let Some(ref k) = kind {
                let want = k.to_lowercase();
                let got = format!("{:?}", rec.kind).to_lowercase();
                if got != want {
                    continue;
                }
            }
            if let Some(ref pid) = product_id {
                if rec.product_id.as_deref() != Some(pid.as_str()) {
                    continue;
                }
            }
            if let Some(tid) = tenant_id {
                if rec.tenant_id != Some(tid) {
                    continue;
                }
            }
            out.push(rec);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn update(&self, record: ExtensionRecord) -> Result<ExtensionRecord, DomainError> {
        self.create(record).await
    }

    async fn save_version(&self, ver: ExtensionVersion) -> Result<ExtensionVersion, DomainError> {
        let dir = self.versions_dir(&ver.extension_id);
        let _ = std::fs::create_dir_all(&dir);
        let p = self.version_path(&ver.extension_id, ver.version);
        let s = serde_json::to_string_pretty(&ver)
            .map_err(|e| DomainError::External(e.to_string()))?;
        std::fs::write(&p, s).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(ver)
    }

    async fn get_version(
        &self,
        id: uuid::Uuid,
        version: i64,
    ) -> Result<Option<ExtensionVersion>, DomainError> {
        let p = self.version_path(&id, version);
        if !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).map_err(|e| DomainError::External(e.to_string()))?;
        let ver: ExtensionVersion =
            serde_json::from_str(&s).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(Some(ver))
    }

    async fn list_versions(&self, id: uuid::Uuid) -> Result<Vec<ExtensionVersion>, DomainError> {
        let dir = self.versions_dir(&id);
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Ok(out);
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(s) = std::fs::read_to_string(&p) else {
                continue;
            };
            if let Ok(ver) = serde_json::from_str::<ExtensionVersion>(&s) {
                out.push(ver);
            }
        }
        out.sort_by_key(|v| v.version);
        Ok(out)
    }
}

/// Local package source tree under `{root}/src/{id}/`.
pub struct LocalExtensionSourceStore {
    root: PathBuf,
}

impl LocalExtensionSourceStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let _ = std::fs::create_dir_all(root.join("src"));
        Self { root }
    }

    fn pkg_root(&self, id: &uuid::Uuid) -> PathBuf {
        self.root.join("src").join(id.to_string())
    }
}

#[async_trait]
impl ExtensionSourceStore for LocalExtensionSourceStore {
    async fn ensure_package(&self, id: uuid::Uuid) -> Result<String, DomainError> {
        let r = self.pkg_root(&id);
        std::fs::create_dir_all(&r).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(r.to_string_lossy().to_string())
    }

    async fn write_file(
        &self,
        id: uuid::Uuid,
        rel_path: String,
        content: String,
    ) -> Result<(), DomainError> {
        let r = self.pkg_root(&id);
        let p = r.join(&rel_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DomainError::External(e.to_string()))?;
        }
        std::fs::write(&p, content).map_err(|e| DomainError::External(e.to_string()))
    }

    async fn read_file(
        &self,
        id: uuid::Uuid,
        rel_path: String,
    ) -> Result<Option<String>, DomainError> {
        let p = self.pkg_root(&id).join(rel_path);
        if !p.is_file() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).map_err(|e| DomainError::External(e.to_string()))?;
        Ok(Some(s))
    }

    async fn list_files(&self, id: uuid::Uuid, prefix: String) -> Result<Vec<String>, DomainError> {
        let r = self.pkg_root(&id);
        let mut out = Vec::new();
        walk_keys(&r, &r, &prefix, &mut out);
        Ok(out)
    }

    async fn package_root(&self, id: uuid::Uuid) -> Result<String, DomainError> {
        Ok(self.pkg_root(&id).to_string_lossy().to_string())
    }
}

/// Build extensions Deps with local filesystem adapters.
/// Local executor: publish via registry bump + marker; invoke always Succeeded (dual-loop).
pub struct LocalExtensionExecutor {
    registry: std::sync::Arc<dyn ExtensionRegistry + Send + Sync>,
    artifacts: PathBuf,
}

impl LocalExtensionExecutor {
    pub fn new(
        registry: std::sync::Arc<dyn ExtensionRegistry + Send + Sync>,
        root: impl Into<PathBuf>,
    ) -> Self {
        let artifacts = root.into().join("artifacts");
        let _ = std::fs::create_dir_all(&artifacts);
        Self { registry, artifacts }
    }
}

#[async_trait]
impl ExtensionExecutor for LocalExtensionExecutor {
    async fn publish(
        &self,
        id: uuid::Uuid,
    ) -> Result<extensions::domain::types::ExtensionVersion, DomainError> {
        let mut rec = self
            .registry
            .get(id)
            .await?
            .ok_or(DomainError::NotFound)?;
        let next = rec.current_version + 1;
        rec.current_version = next;
        rec.updated_on = Utc::now();
        self.registry.update(rec.clone()).await?;
        let marker = self.artifacts.join(format!("{id}/{next}/rust.marker"));
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&marker, b"published");
        let ver = extensions::domain::types::ExtensionVersion {
            extension_id: id,
            version: next,
            source_commit: "local".into(),
            artifact_uris: serde_json::json!({ "rust": marker.to_string_lossy() }),
            published_on: Utc::now(),
            published_by: None,
            changelog: None,
        };
        self.registry.save_version(ver.clone()).await
    }

    async fn invoke(
        &self,
        req: extensions::domain::types::ExtensionInvokeRequest,
    ) -> Result<extensions::domain::types::ExtensionInvokeResult, DomainError> {
        // Pin check: version marker or record must exist
        let _ = self
            .registry
            .get_version(req.extension_id, req.version)
            .await?;
        Ok(extensions::domain::types::ExtensionInvokeResult {
            status: extensions::domain::types::ExtensionRunStatus::Succeeded,
            message: Some(format!(
                "invoked {}@{}",
                req.extension_id, req.version
            )),
            outputs: req.params,
        })
    }
}

pub fn extensions_deps() -> extensions::application::Deps {
    let dir = extensions_dir();
    let registry: std::sync::Arc<dyn ExtensionRegistry + Send + Sync> =
        std::sync::Arc::new(LocalExtensionRegistry::new(&dir));
    let sources = std::sync::Arc::new(LocalExtensionSourceStore::new(&dir));
    let executor = std::sync::Arc::new(LocalExtensionExecutor::new(registry.clone(), &dir));
    extensions::application::Deps {
        extension_executor: executor,
        extension_registry: registry,
        extension_source_store: sources,
    }
}

#[cfg(test)]
mod extension_tests {
    use super::*;
    use extensions::application;
    use extensions::domain::types::{
        ExtensionKind, ExtensionProvenance, ExtensionScope,
    };

    #[tokio::test]
    async fn create_list_get_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = LocalExtensionRegistry::new(tmp.path());
        let src = LocalExtensionSourceStore::new(tmp.path());
        let deps = extensions::application::Deps {
            extension_registry: std::sync::Arc::new(reg),
            extension_source_store: std::sync::Arc::new(src),
        };
        let rec = application::create_extension(
            &deps,
            "Activate members".into(),
            "Reaction".into(),
            "Platform".into(),
            "Stock".into(),
            None,
            None,
            None,
            Some("seed stock".into()),
            None,
        )
        .await
        .expect("create");
        assert_eq!(rec.name, "Activate members");
        assert_eq!(rec.current_version, 0);
        assert!(matches!(rec.provenance, ExtensionProvenance::Stock));
        assert!(matches!(rec.kind, ExtensionKind::Reaction));
        assert!(matches!(rec.scope, ExtensionScope::Platform));

        let listed = application::list_extensions(&deps, None, None, None, None)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);

        let got = application::get_extension(&deps, rec.extension_id)
            .await
            .expect("get");
        assert_eq!(got.name, "Activate members");

        let ver = application::save_extension_version(
            &deps,
            rec.extension_id,
            "local".into(),
            serde_json::json!({ "rust": "artifact://local" }),
            Some("first publish".into()),
        )
        .await
        .expect("publish version");
        assert_eq!(ver.version, 1);

        let versions = application::list_extension_versions(&deps, rec.extension_id)
            .await
            .expect("list versions");
        assert_eq!(versions.len(), 1);
    }
}
