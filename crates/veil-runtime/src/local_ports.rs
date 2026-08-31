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

fn catalog_sqlite_path(projects_dir: &Path) -> PathBuf {
    projects_dir.join(".veil-catalog.sqlite")
}

fn load_metadb(projects_dir: &Path, json_path: &Path) -> MetaDb {
    let sqlite = catalog_sqlite_path(projects_dir);
    if sqlite.is_file() {
        if let Ok(conn) = rusqlite::Connection::open(&sqlite) {
            if let Ok(data) = conn.query_row(
                "SELECT data FROM snapshot WHERE id = 1",
                [],
                |r| r.get::<_, String>(0),
            ) {
                if let Ok(db) = serde_json::from_str(&data) {
                    return db;
                }
            }
        }
    }
    if json_path.is_file() {
        if let Ok(s) = std::fs::read_to_string(json_path) {
            if let Ok(db) = serde_json::from_str(&s) {
                return db;
            }
        }
    }
    MetaDb::default()
}

fn save_metadb(projects_dir: &Path, json_path: &Path, db: &MetaDb) {
    let Ok(json) = serde_json::to_string(db) else {
        return;
    };
    let _ = std::fs::create_dir_all(projects_dir);
    if let Ok(pretty) = serde_json::to_string_pretty(db) {
        let _ = std::fs::write(json_path, pretty);
    }
    let sqlite = catalog_sqlite_path(projects_dir);
    if let Ok(conn) = rusqlite::Connection::open(&sqlite) {
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS snapshot (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                data TEXT NOT NULL
            );",
        );
        let _ = conn.execute(
            "INSERT INTO snapshot (id, data) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            rusqlite::params![json],
        );
    }
}

impl LocalMetadataStore {
    pub fn new(projects_dir: impl Into<PathBuf>) -> Self {
        let projects_dir = projects_dir.into();
        let meta_path = projects_dir.join(".veil-meta.json");
        let inner = load_metadb(&projects_dir, &meta_path);
        Self {
            projects_dir,
            meta_path,
            inner: Mutex::new(inner),
        }
    }

    fn save(&self, db: &MetaDb) {
        save_metadb(&self.projects_dir, &self.meta_path, db);
    }

    fn branch_key(repo_id: &RepoId, name: &str) -> String {
        format!("{}/{}", repo_id.value, name)
    }

    /// Sync hub projects into repo index (id = name for product IDE).
    fn sync_hub(&self, db: &mut MetaDb) {
        if veil_server::platform_local() {
            return;
        }
        if let Ok(list) = veil_server::list_projects(&self.projects_dir) {
            for p in list {
                let name = p.name.clone();
                db.repos.entry(name.clone()).or_insert_with(|| Repo {
                    id: RepoId {
                        value: name.clone(),
                    },
                    name: name.clone(),
                    slug: name.clone(),
                    description: None,
                    default_branch: "main".into(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    origin: None,
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
        // Disk hub scaffold only when not strict remote (VEIL_SOURCE_MODE=s3).
        // Remote product host uses DdbMetadataStore + S3 seed via create_project tool.
        if veil_server::provider::s3_workspace::allow_disk_project_create() {
            let _ = veil_server::create_project(&self.projects_dir, &metadata.name)
                .map_err(|e| DomainError::External(e))?;
        } else {
            tracing::info!(
                name = %metadata.name,
                "LocalMetadataStore::create_repo: skip disk hub (VEIL_SOURCE_MODE=s3)"
            );
        }
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

    async fn update_repo(&self, metadata: Repo) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        // Drop stale index keys (id / old name / old slug) then re-index.
        let stale: Vec<String> = db
            .repos
            .iter()
            .filter(|(_, r)| r.id.value == metadata.id.value)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            db.repos.remove(&k);
        }
        db.repos
            .insert(metadata.id.value.clone(), metadata.clone());
        db.repos.insert(metadata.slug.clone(), metadata.clone());
        if metadata.name != metadata.slug && metadata.name != metadata.id.value {
            db.repos.insert(metadata.name.clone(), metadata.clone());
        }
        self.save(&db);
        Ok(())
    }

    async fn delete_repo(&self, id: RepoId) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        let stale: Vec<String> = db
            .repos
            .iter()
            .filter(|(k, r)| *k == &id.value || r.id.value == id.value)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            db.repos.remove(&k);
        }
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


// ─── Change Management Deps (DDB + S3 adapters) ─────────────────────────────
// SDLC domain: runtime.veil → change_management::application
// IO: DDB adapters (single table), S3 flat-file git adapter

use change_management::adapters::{
    DdbApprovalRepo, DdbPullRequestRepo, DdbCiRunRepo, DdbCommentRepo, S3GitServiceAdapter,
};

/// Build change_management Deps backed by DDB + S3.
/// Uses VEIL_DDB_TABLE for all DDB repos, BUCKET for S3 git.
pub async fn change_management_deps() -> change_management::application::Deps {
    if veil_server::platform_local() {
        return local_change_management_deps();
    }
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let ddb_client = aws_sdk_dynamodb::Client::new(&config);
    let s3_client = aws_sdk_s3::Client::new(&config);

    let table = std::env::var("VEIL_DDB_TABLE").unwrap_or_else(|_| "veil-runtime-dev".into());
    let bucket = std::env::var("BUCKET").unwrap_or_else(|_| "veil-runtime-dev".into());

    change_management::application::Deps {
        git: std::sync::Arc::new(S3GitServiceAdapter {
            bucket,
            s3: s3_client,
        }),
        pr_repo: std::sync::Arc::new(DdbPullRequestRepo {
            client: ddb_client.clone(),
            table: table.clone(),
        }),
        approval_repo: std::sync::Arc::new(DdbApprovalRepo {
            client: ddb_client.clone(),
            table: table.clone(),
        }),
        ci_repo: std::sync::Arc::new(DdbCiRunRepo {
            client: ddb_client.clone(),
            table: table.clone(),
        }),
        comment_repo: std::sync::Arc::new(DdbCommentRepo {
            client: ddb_client,
            table,
        }),
    }
}

fn local_change_management_deps() -> change_management::application::Deps {
    let dir = crate::platform::projects_dir();
    let store = LocalCmStore::new(&dir);
    change_management::application::Deps {
        git: std::sync::Arc::new(LocalGitService),
        pr_repo: std::sync::Arc::new(store.clone()),
        approval_repo: std::sync::Arc::new(store.clone()),
        ci_repo: std::sync::Arc::new(store.clone()),
        comment_repo: std::sync::Arc::new(store),
    }
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct CmDb {
    prs: HashMap<String, change_management::domain::types::PullRequest>,
    approvals: Vec<change_management::domain::types::Approval>,
    ci: Vec<change_management::domain::types::CiRun>,
    comments: Vec<change_management::domain::types::ReviewComment>,
}

#[derive(Clone)]
struct LocalCmStore {
    path: PathBuf,
    inner: std::sync::Arc<Mutex<CmDb>>,
}

impl LocalCmStore {
    fn new(projects_dir: &Path) -> Self {
        let path = projects_dir.join(".veil-cm.json");
        let db = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: std::sync::Arc::new(Mutex::new(db)),
        }
    }

    fn persist(&self, db: &CmDb) {
        if let Ok(s) = serde_json::to_string_pretty(db) {
            let _ = std::fs::write(&self.path, s);
        }
    }
}

#[async_trait]
impl change_management::ports::PullRequestRepo for LocalCmStore {
    async fn find(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<change_management::domain::types::PullRequest>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db.prs.get(&id.to_string()).cloned())
    }
    async fn list_by_repo(
        &self,
        repo_id: uuid::Uuid,
        status: Option<change_management::domain::types::PrStatus>,
    ) -> Result<Vec<change_management::domain::types::PullRequest>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .prs
            .values()
            .filter(|p| p.repo_id == repo_id)
            .filter(|p| status.as_ref().map(|s| &p.status == s).unwrap_or(true))
            .cloned()
            .collect())
    }
    async fn list_open(
        &self,
        repo_id: uuid::Uuid,
    ) -> Result<Vec<change_management::domain::types::PullRequest>, DomainError> {
        use change_management::domain::types::PrStatus;
        let db = self.inner.lock().unwrap();
        Ok(db
            .prs
            .values()
            .filter(|p| p.repo_id == repo_id)
            .filter(|p| {
                !matches!(
                    p.status,
                    PrStatus::Merged | PrStatus::Rejected | PrStatus::Closed
                )
            })
            .cloned()
            .collect())
    }
    async fn list_all(
        &self,
        status: Option<change_management::domain::types::PrStatus>,
    ) -> Result<Vec<change_management::domain::types::PullRequest>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .prs
            .values()
            .filter(|p| status.as_ref().map(|s| &p.status == s).unwrap_or(true))
            .cloned()
            .collect())
    }
    async fn save(
        &self,
        cr: change_management::domain::types::PullRequest,
    ) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.prs.insert(cr.id.to_string(), cr);
        self.persist(&db);
        Ok(())
    }
}

#[async_trait]
impl change_management::ports::ApprovalRepo for LocalCmStore {
    async fn find_for_pr(
        &self,
        pr_id: uuid::Uuid,
    ) -> Result<Vec<change_management::domain::types::Approval>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .approvals
            .iter()
            .filter(|a| a.pr_id == pr_id)
            .cloned()
            .collect())
    }
    async fn save(
        &self,
        approval: change_management::domain::types::Approval,
    ) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.approvals.push(approval);
        self.persist(&db);
        Ok(())
    }
}

#[async_trait]
impl change_management::ports::CiRunRepo for LocalCmStore {
    async fn latest_for_pr(
        &self,
        pr_id: uuid::Uuid,
    ) -> Result<Option<change_management::domain::types::CiRun>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db.ci.iter().filter(|c| c.pr_id == pr_id).cloned().last())
    }
    async fn list_for_pr(
        &self,
        pr_id: uuid::Uuid,
    ) -> Result<Vec<change_management::domain::types::CiRun>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db.ci.iter().filter(|c| c.pr_id == pr_id).cloned().collect())
    }
    async fn save(&self, run: change_management::domain::types::CiRun) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.ci.push(run);
        self.persist(&db);
        Ok(())
    }
}

#[async_trait]
impl change_management::ports::CommentRepo for LocalCmStore {
    async fn list_for_pr(
        &self,
        pr_id: uuid::Uuid,
    ) -> Result<Vec<change_management::domain::types::ReviewComment>, DomainError> {
        let db = self.inner.lock().unwrap();
        Ok(db
            .comments
            .iter()
            .filter(|c| c.pr_id == pr_id)
            .cloned()
            .collect())
    }
    async fn save(
        &self,
        comment: change_management::domain::types::ReviewComment,
    ) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        db.comments.push(comment);
        self.persist(&db);
        Ok(())
    }
    async fn resolve(&self, id: uuid::Uuid) -> Result<(), DomainError> {
        let mut db = self.inner.lock().unwrap();
        if let Some(c) = db.comments.iter_mut().find(|c| c.id == id) {
            c.resolved = true;
        }
        self.persist(&db);
        Ok(())
    }
}

struct LocalGitService;

#[async_trait]
impl change_management::ports::GitService for LocalGitService {
    async fn init_repo(&self, _slug: String) -> Result<(), DomainError> {
        Ok(())
    }
    async fn repo_exists(&self, _slug: String) -> Result<bool, DomainError> {
        Ok(true)
    }
    async fn create_branch(
        &self,
        _slug: String,
        branch_name: String,
        _from_ref: String,
    ) -> Result<String, DomainError> {
        Ok(branch_name)
    }
    async fn delete_branch(&self, _slug: String, _branch: String) -> Result<(), DomainError> {
        Ok(())
    }
    async fn list_branches(&self, _slug: String) -> Result<Vec<String>, DomainError> {
        Ok(vec!["main".into()])
    }
    async fn get_head(&self, _slug: String, _branch: String) -> Result<String, DomainError> {
        Ok("HEAD".into())
    }
    async fn commit_file(
        &self,
        _slug: String,
        _branch: String,
        _path: String,
        _content: String,
        _message: String,
        _author: String,
    ) -> Result<String, DomainError> {
        Ok("local".into())
    }
    async fn read_file(
        &self,
        _slug: String,
        _branch: String,
        _path: String,
    ) -> Result<Option<String>, DomainError> {
        Ok(None)
    }
    async fn list_files(
        &self,
        _slug: String,
        _branch: String,
    ) -> Result<Vec<String>, DomainError> {
        Ok(vec![])
    }
    async fn log(
        &self,
        _slug: String,
        _branch: String,
        _limit: i64,
    ) -> Result<serde_json::Value, DomainError> {
        Ok(serde_json::json!([]))
    }
    async fn merge(
        &self,
        _slug: String,
        _source: String,
        _target: String,
        _message: String,
        _author: String,
    ) -> Result<String, DomainError> {
        Ok("local-merge".into())
    }
    async fn diff_files(
        &self,
        _slug: String,
        _base_ref: String,
        _head_ref: String,
    ) -> Result<serde_json::Value, DomainError> {
        Ok(serde_json::json!([]))
    }
    async fn can_merge(
        &self,
        _slug: String,
        _source: String,
        _target: String,
    ) -> Result<serde_json::Value, DomainError> {
        Ok(serde_json::json!({ "can_merge": true }))
    }
}

/// Empty deploy store + stub exec so local mode never talks to DashLX AWS.
pub fn local_deploy_deps(
    bus: std::sync::Arc<dyn veil_shared::Bus + Send + Sync>,
) -> deploy::application::Deps {
    deploy::application::Deps {
        store: std::sync::Arc::new(LocalDeployStore::default()),
        exec: std::sync::Arc::new(LocalDeployExec),
        executor: std::sync::Arc::new(deploy::adapters::MockActionExecutor {}),
        bus,
    }
}

#[derive(Default)]
struct LocalDeployStore;

#[async_trait]
impl deploy::ports::DeploymentStateStore for LocalDeployStore {
    async fn get_current(
        &self,
        _environment: String,
        _unit_name: String,
    ) -> Result<Option<deploy::domain::types::DeploymentState>, DomainError> {
        Ok(None)
    }
    async fn save_current(
        &self,
        _state: deploy::domain::types::DeploymentState,
    ) -> Result<(), DomainError> {
        Ok(())
    }
    async fn save_version(
        &self,
        _state: deploy::domain::types::DeploymentState,
    ) -> Result<(), DomainError> {
        Ok(())
    }
    async fn get_version(
        &self,
        _environment: String,
        _unit_name: String,
        _version: i64,
    ) -> Result<Option<deploy::domain::types::DeploymentState>, DomainError> {
        Ok(None)
    }
    async fn list_versions(
        &self,
        _environment: String,
        _unit_name: String,
        _limit: i64,
    ) -> Result<Vec<deploy::domain::types::DeploymentState>, DomainError> {
        Ok(vec![])
    }
    async fn append_event(
        &self,
        _environment: String,
        _unit_name: String,
        _event: deploy::domain::types::DeployEvent,
    ) -> Result<(), DomainError> {
        Ok(())
    }
    async fn get_events(
        &self,
        _environment: String,
        _unit_name: String,
        _limit: i64,
    ) -> Result<Vec<deploy::domain::types::DeployEvent>, DomainError> {
        Ok(vec![])
    }
    async fn list_deployments(
        &self,
    ) -> Result<Vec<deploy::domain::types::DeploymentState>, DomainError> {
        Ok(vec![])
    }
}

struct LocalDeployExec;

fn local_deploy_unsupported() -> Result<String, DomainError> {
    Ok(r#"{"ok":false,"error":"local ProductHost — deploy is not connected to AWS"}"#.into())
}

#[async_trait]
impl deploy::ports::DeployExec for LocalDeployExec {
    async fn list_environments(&self) -> Result<String, DomainError> {
        Ok(
            r#"{"default":"dev","environments":[{"name":"dev","region":null,"account_id":null,"has_assume_role":false,"assume_role_arn":null,"lambda_execution_role_arn":null,"gateways":[]}],"config_path":"local"}"#
                .into(),
        )
    }
    async fn read_project_deploy(
        &self,
        _repo_id: String,
        _branch: String,
        _slug: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn sync_hub_to_s3(
        &self,
        _repo_id: String,
        _branch: String,
        _slug: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn plan_provision(
        &self,
        _project_slug: String,
        _environment: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn plan_provision_repo(
        &self,
        _repo_id: String,
        _branch: String,
        _slug: String,
        _environment: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn start_provision(
        &self,
        _project_slug: String,
        _environment: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn start_provision_repo(
        &self,
        _repo_id: String,
        _branch: String,
        _slug: String,
        _environment: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn get_provision_job(&self, _job_id: String) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn provision_unit(
        &self,
        _project_slug: String,
        _environment: String,
        _unit_name: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
    async fn clear_unit_state(
        &self,
        _environment: String,
        _unit_name: String,
    ) -> Result<String, DomainError> {
        local_deploy_unsupported()
    }
}
