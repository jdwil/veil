//! Durable coding sessions: DDB META + session-keyed workdirs + S3 write-through.
//!
//! See `runtime/docs/DURABLE_SESSIONS.md`.
//!
//! - **L1** Source bytes → S3
//! - **L2** Session META → DDB `SESSION#{id}/META`
//! - **L3** Agent turns → DDB `SESSION#{id}/TURN#{ulid}` (optional S3 blob)

mod ddb;
mod workspace;

pub use ddb::{
    append_turn, delete_session_meta, get_session_meta, list_sessions_for_user, list_turns,
    put_session_meta, touch_session, SessionMeta, SessionTurn,
};
pub use workspace::{
    materialize_policy, path_jail, resolve_under_root, MaterializePolicy, WorkspaceFs,
    WorkspaceFsImpl,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::provider::s3_workspace::{
    ide_source_mode, materialize_repo, resolve_repo_id, IdeSourceMode, S3WorkspaceProvider,
};
use uuid::Uuid;

/// Feature flag: durable sessions enabled (default on for s3 modes).
pub fn sessions_enabled() -> bool {
    match std::env::var("VEIL_SESSIONS")
        .unwrap_or_else(|_| "auto".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "false" | "off" | "no" => false,
        "1" | "true" | "on" | "yes" => true,
        _ => !matches!(ide_source_mode(), IdeSourceMode::Disk),
    }
}

pub fn current_user_id() -> String {
    std::env::var("VEIL_DEV_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "local-dev".into())
}

pub fn ws_root() -> PathBuf {
    std::env::var("VEIL_WS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("veil-ws"))
}

pub fn default_branch() -> String {
    std::env::var("VEIL_SOURCE_BRANCH").unwrap_or_else(|_| "main".into())
}

/// Local path for a session workspace (isolated per user+session+slug).
pub fn session_work_dir(user_id: &str, session_id: &str, slug: &str) -> PathBuf {
    ws_root().join(user_id).join(session_id).join(slug)
}

/// Process-local open session handles.
pub struct SessionManager {
    handles: Mutex<HashMap<String, Arc<SessionHandle>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            handles: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static SessionManager {
        static M: std::sync::OnceLock<SessionManager> = std::sync::OnceLock::new();
        M.get_or_init(SessionManager::new)
    }

    /// Create a new durable session, materialize once, persist META.
    pub fn create(&self, slug: &str, branch: Option<&str>) -> Result<Arc<SessionHandle>, String> {
        self.create_with_opts(slug, branch, false)
    }

    pub fn create_with_opts(
        &self,
        slug: &str,
        branch: Option<&str>,
        draft_mode: bool,
    ) -> Result<Arc<SessionHandle>, String> {
        let user_id = current_user_id();
        let session_id = Uuid::new_v4().to_string();
        let branch = branch.unwrap_or(&default_branch()).to_string();
        let repo_id = resolve_repo_id(slug)?;
        let work_dir = session_work_dir(&user_id, &session_id, slug);
        let work_prefix = if draft_mode {
            format!("repos/{repo_id}/drafts/{session_id}/")
        } else {
            format!("repos/{repo_id}/{branch}/")
        };

        // Materialize once from branch tree (drafts start as a copy of branch)
        materialize_repo_to(&repo_id, &work_dir, MaterializePolicy::SyncDelete)?;

        let now = chrono_now();
        let meta = SessionMeta {
            session_id: session_id.clone(),
            user_id: user_id.clone(),
            slug: slug.to_string(),
            repo_id: repo_id.clone(),
            branch: branch.clone(),
            work_prefix: work_prefix.clone(),
            revision: 0,
            active_file: None,
            open_files: vec![],
            etags: HashMap::new(),
            dirty: vec![],
            draft_mode,
            created_at: now.clone(),
            updated_at: now.clone(),
            last_activity_at: now,
            agent_thread_id: None,
        };
        put_session_meta(&meta)?;
        write_session_marker(&work_dir, &meta)?;

        let handle = Arc::new(SessionHandle::open(meta, work_dir)?);
        self.handles
            .lock()
            .unwrap()
            .insert(session_id, handle.clone());
        Ok(handle)
    }

    /// Attach existing session: load DDB META, ensure workdir (rematerialize if missing).
    pub fn attach(&self, session_id: &str) -> Result<Arc<SessionHandle>, String> {
        {
            let map = self.handles.lock().unwrap();
            if let Some(h) = map.get(session_id) {
                let _ = touch_session(session_id);
                return Ok(h.clone());
            }
        }
        let meta = get_session_meta(session_id)?;
        if meta.user_id != current_user_id()
            && std::env::var("VEIL_SESSION_ALLOW_CROSS_USER").ok().as_deref() != Some("1")
        {
            // Local dev often shares one user; still allow same machine
            tracing::warn!(
                session_user = %meta.user_id,
                current = %current_user_id(),
                "attaching session owned by another user id"
            );
        }
        let work_dir = session_work_dir(&meta.user_id, &meta.session_id, &meta.slug);
        if !work_dir.join("veil.toml").is_file() && !has_veil_file(&work_dir) {
            materialize_repo_to(&meta.repo_id, &work_dir, MaterializePolicy::SyncIncremental)?;
        }
        write_session_marker(&work_dir, &meta)?;
        let handle = Arc::new(SessionHandle::open(meta, work_dir)?);
        self.handles
            .lock()
            .unwrap()
            .insert(session_id.to_string(), handle.clone());
        let _ = touch_session(session_id);
        Ok(handle)
    }

    /// Get or create a default sticky session for user+slug (compat when no header).
    pub fn get_or_create_default(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let user = current_user_id();
        // Prefer most recent open handle for slug
        {
            let map = self.handles.lock().unwrap();
            if let Some(h) = map.values().find(|h| {
                let m = h.meta.lock().unwrap();
                m.slug == slug && m.user_id == user
            }) {
                return Ok(h.clone());
            }
        }
        // Try DDB list for recent session
        if let Ok(list) = list_sessions_for_user(&user) {
            if let Some(m) = list.into_iter().find(|m| m.slug == slug) {
                return self.attach(&m.session_id);
            }
        }
        self.create(slug, None)
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<SessionHandle>> {
        self.handles.lock().unwrap().get(session_id).cloned()
    }

    pub fn drop_handle(&self, session_id: &str) {
        self.handles.lock().unwrap().remove(session_id);
    }
}

fn has_veil_file(root: &Path) -> bool {
    root.is_dir()
        && std::fs::read_dir(root)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x == "veil")
                    .unwrap_or(false)
            })
}

fn materialize_repo_to(
    repo_id: &str,
    work: &Path,
    policy: MaterializePolicy,
) -> Result<(), String> {
    match policy {
        MaterializePolicy::SyncDelete => materialize_repo(repo_id, work),
        MaterializePolicy::SyncIncremental => {
            // Same as materialize but without --delete (see s3_workspace helper)
            crate::provider::s3_workspace::materialize_repo_incremental(repo_id, work)
        }
    }
}

fn write_session_marker(work_dir: &Path, meta: &SessionMeta) -> Result<(), String> {
    let p = work_dir.join(".veil-session.json");
    let v = serde_json::json!({
        "session_id": meta.session_id,
        "repo_id": meta.repo_id,
        "branch": meta.branch,
        "slug": meta.slug,
        "revision": meta.revision,
        "user_id": meta.user_id,
    });
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| format!("write session marker: {e}"))
}

fn chrono_now() -> String {
    // RFC3339-ish without chrono crate
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Live session: provider + workspace FS + mutable META.
pub struct SessionHandle {
    pub meta: Mutex<SessionMeta>,
    pub work_dir: PathBuf,
    /// S3-backed IDE provider (serve set).
    pub provider: Arc<S3WorkspaceProvider>,
    pub fs: WorkspaceFsImpl,
}

impl SessionHandle {
    fn open(meta: SessionMeta, work_dir: PathBuf) -> Result<Self, String> {
        // Open provider bound to this work_dir (not global slug tmp)
        let provider = open_s3_project_at(
            &meta.slug,
            &meta.repo_id,
            &work_dir,
            false,
            meta.draft_mode,
            &meta.session_id,
        )?;
        let fs = WorkspaceFsImpl::new(
            work_dir.clone(),
            meta.repo_id.clone(),
            meta.branch.clone(),
            meta.session_id.clone(),
            meta.draft_mode,
        );
        Ok(Self {
            meta: Mutex::new(meta),
            work_dir,
            provider,
            fs,
        })
    }

    pub fn session_id(&self) -> String {
        self.meta.lock().unwrap().session_id.clone()
    }

    pub fn slug(&self) -> String {
        self.meta.lock().unwrap().slug.clone()
    }

    pub fn revision(&self) -> u64 {
        self.meta.lock().unwrap().revision
    }

    pub fn bump_revision(&self, path: &str, etag: Option<String>) -> u64 {
        let mut m = self.meta.lock().unwrap();
        m.revision = m.revision.saturating_add(1);
        m.updated_at = chrono_now();
        m.last_activity_at = m.updated_at.clone();
        if let Some(e) = etag {
            m.etags.insert(path.to_string(), e);
        }
        m.dirty.retain(|p| p != path);
        let rev = m.revision;
        let snap = m.clone();
        drop(m);
        let _ = put_session_meta(&snap);
        rev
    }

    pub fn set_active_file(&self, name: &str) {
        let mut m = self.meta.lock().unwrap();
        m.active_file = Some(name.to_string());
        if !m.open_files.iter().any(|f| f == name) {
            m.open_files.push(name.to_string());
        }
        m.last_activity_at = chrono_now();
        let snap = m.clone();
        drop(m);
        let _ = put_session_meta(&snap);
    }

    pub fn etag_for(&self, path: &str) -> Option<String> {
        self.meta.lock().unwrap().etags.get(path).cloned()
    }

    pub fn snapshot_meta(&self) -> SessionMeta {
        self.meta.lock().unwrap().clone()
    }

    /// Pull remote into workdir (incremental).
    pub fn pull_remote(&self) -> Result<(), String> {
        let m = self.meta.lock().unwrap().clone();
        materialize_repo_to(&m.repo_id, &self.work_dir, MaterializePolicy::SyncIncremental)
    }

    /// Hard reset workdir from remote.
    pub fn reset_to_remote(&self) -> Result<(), String> {
        let m = self.meta.lock().unwrap().clone();
        materialize_repo_to(&m.repo_id, &self.work_dir, MaterializePolicy::SyncDelete)
    }
}

/// Open S3 provider against an explicit work directory (session-scoped).
pub fn open_s3_project_at(
    slug: &str,
    repo_id: &str,
    work: &Path,
    show_core_layers: bool,
    draft_mode: bool,
    session_id: &str,
) -> Result<Arc<S3WorkspaceProvider>, String> {
    use crate::project_layout::{collect_project_files, is_source_editable};
    use crate::provider::filesystem::FilesystemProvider;
    use veil_ir::LayerRegistry;

    if !has_veil_file(work) && !work.join("veil.toml").is_file() {
        materialize_repo(repo_id, work)?;
    }
    let paths = collect_project_files(work, show_core_layers)
        .map_err(|e| format!("session workspace {slug}: {e}"))?;
    let entries: Vec<(PathBuf, String, bool)> = paths
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            let editable = is_source_editable(&path, &source);
            (path, source, editable)
        })
        .collect();
    if entries.is_empty() {
        return Err(format!("session workspace {slug} empty"));
    }
    let reg =
        LayerRegistry::for_veil_file(&entries[0].0).unwrap_or_else(|_| LayerRegistry::builtin());
    let inner = FilesystemProvider::with_files_in_project(entries, reg, Some(work.to_path_buf()));
    Ok(S3WorkspaceProvider::from_parts(
        Arc::new(inner),
        repo_id.to_string(),
        work.to_path_buf(),
        slug.to_string(),
        draft_mode,
        session_id.to_string(),
    ))
}

/// Request-scoped coding session id (HTTP header `X-Veil-Session-Id`).
tokio::task_local! {
    pub static CURRENT_SESSION: String;
}

pub fn current_session_id() -> Option<String> {
    CURRENT_SESSION.try_with(|s| s.clone()).ok()
}
