//! Durable coding sessions: local git checkout + S3 origin + DDB META.
//!
//! See `docs/DURABLE_SESSIONS.md`.
//!
//! - **L1** Source bytes → S3
//! - **L2** Session META → DDB `SESSION#{id}/META`
//! - **L3** Agent turns → DDB `SESSION#{id}/TURN#{ulid}` (optional S3 blob)

mod ddb;
mod workspace;

pub use ddb::{
    append_turn, delete_session_meta, get_session_commit, get_session_meta, list_session_commits,
    list_sessions_for_user, list_turns, merge_session_focus_intents, put_session_commit,
    put_session_meta, touch_session, HostCheckSnapshot, SessionCommit, SessionMeta, SessionTurn,
};
pub use workspace::{
    materialize_policy, path_jail, resolve_under_root, MaterializePolicy, WorkspaceFs,
    WorkspaceFsImpl,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::provider::s3_workspace::{
    ide_source_mode, materialize_repo, resolve_project_identity, resolve_repo_id, IdeSourceMode,
    ProjectIdentity, S3WorkspaceProvider,
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
    /// Agent/operator preferred work line per project slug (feature branch or main).
    /// When set, MCP tools and middleware without `X-Veil-Session-Id` resolve here
    /// instead of the sticky mainline session — so `create_branch` sticks for the turn.
    active_by_project: Mutex<HashMap<String, String>>,
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
            active_by_project: Mutex::new(HashMap::new()),
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
        self.create_branch(slug, branch, draft_mode, None)
    }

    /// Create a session. When `branch_name` is set (or draft_mode), this is a
    /// **feature branch**: new local checkout of origin, `git checkout -b`.
    pub fn create_branch(
        &self,
        slug: &str,
        base_branch: Option<&str>,
        draft_mode: bool,
        branch_name: Option<&str>,
    ) -> Result<Arc<SessionHandle>, String> {
        let user_id = current_user_id();
        let session_id = Uuid::new_v4().to_string();
        let base = base_branch.unwrap_or(&default_branch()).to_string();
        let is_branch = draft_mode || branch_name.is_some();
        let draft_mode = is_branch; // feature branches always isolate writes
        let branch_name = branch_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if draft_mode {
                    format!("work/{}", &session_id[..8.min(session_id.len())])
                } else {
                    base.clone()
                }
            });
        // Always store product slug (not raw UUID from /projects/{id}/ide).
        let ident = resolve_project_identity(slug)?;
        let slug = ident.slug.as_str();
        let repo_id = ident.repo_id.clone();
        let work_dir = session_work_dir(&user_id, &session_id, slug);
        let work_prefix = if crate::git_origin::origin_enabled() {
            format!("git/{repo_id}#refs/heads/{branch_name}")
        } else if draft_mode {
            format!("repos/{repo_id}/drafts/{session_id}/")
        } else {
            format!("repos/{repo_id}/{base}/")
        };

        // Local checkout of origin (real git) or legacy S3 tree.
        materialize_repo_to(
            &repo_id,
            &work_dir,
            MaterializePolicy::SyncIncremental,
            Some(&base),
        )?;
        if crate::git_origin::origin_enabled()
            && draft_mode
            && branch_name != base
        {
            crate::git_origin::GitOrigin::new(&repo_id)
                .create_branch(&work_dir, &branch_name)?;
        }

        let now = chrono_now();
        let meta = SessionMeta {
            session_id: session_id.clone(),
            user_id: user_id.clone(),
            slug: slug.to_string(),
            repo_id: repo_id.clone(),
            branch: base.clone(),
            work_prefix: work_prefix.clone(),
            revision: 0,
            active_file: None,
            open_files: vec![],
            etags: HashMap::new(),
            dirty: vec![],
            draft_mode,
            branch_name: Some(branch_name),
            base_branch: Some(base),
            head_commit: None,
            committed_revision: Some(0),
            created_at: now.clone(),
            updated_at: now.clone(),
            last_activity_at: now,
            agent_thread_id: None,
            last_focus: None,
            intent_log: vec![],
            active_pr_id: None,
            writes_since_commit: 0,
            last_host_check: None,
            rationales: HashMap::new(),
        };
        put_session_meta(&meta)?;
        write_session_marker(&work_dir, &meta)?;

        // Sticky only for mainline sessions — feature branches are explicit.
        // Dual-write product slug + repo_id so UUID and slug routes share one session.
        if !draft_mode {
            write_sticky_aliases(&user_id, &ident, &session_id);
        }
        let handle = Arc::new(SessionHandle::open(meta, work_dir)?);
        self.handles
            .lock()
            .unwrap()
            .insert(session_id.clone(), handle.clone());
        // Feature branches become the active work line for this project immediately
        if draft_mode {
            self.set_active_for_identity(&ident, &session_id);
        }
        Ok(handle)
    }

    /// Remember which session the agent/operator is working on for `slug`.
    pub fn set_active_for_project(&self, slug: &str, session_id: &str) {
        self.active_by_project
            .lock()
            .unwrap()
            .insert(slug.to_string(), session_id.to_string());
        // Dual-key when we can resolve identity (best-effort; no AWS fail path).
        if let Ok(ident) = resolve_project_identity(slug) {
            if ident.repo_id != ident.slug {
                self.active_by_project
                    .lock()
                    .unwrap()
                    .insert(ident.repo_id.clone(), session_id.to_string());
            }
            if ident.slug != slug {
                self.active_by_project
                    .lock()
                    .unwrap()
                    .insert(ident.slug, session_id.to_string());
            }
        }
    }

    fn set_active_for_identity(&self, ident: &ProjectIdentity, session_id: &str) {
        self.active_by_project
            .lock()
            .unwrap()
            .insert(ident.slug.clone(), session_id.to_string());
        if ident.repo_id != ident.slug {
            self.active_by_project
                .lock()
                .unwrap()
                .insert(ident.repo_id.clone(), session_id.to_string());
        }
    }

    /// Preferred session for a project (feature branch or explicit switch), if any.
    pub fn active_for_project(&self, slug: &str) -> Option<String> {
        self.active_by_project
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
    }

    /// Clear preferred session (e.g. after switch back to main sticky).
    pub fn clear_active_for_project(&self, slug: &str) {
        self.active_by_project.lock().unwrap().remove(slug);
    }

    /// Resolve the coding session for a project when no header was provided:
    /// active_by_project → sticky mainline default.
    pub fn resolve_for_project(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let ident = resolve_project_identity(slug).ok();
        let keys = identity_keys(slug, ident.as_ref());
        for key in &keys {
            if let Some(sid) = self.active_for_project(key) {
                if let Ok(h) = self.attach(&sid) {
                    if session_matches_current_repo(&h, slug) {
                        if let Some(ref id) = ident {
                            self.set_active_for_identity(id, &h.session_id());
                        }
                        return Ok(h);
                    }
                    // Stale active (e.g. orphan repo_id after re-create) — drop preference
                    self.clear_active_for_project(key);
                }
            }
        }
        self.get_or_create_default(slug)
    }

    /// Open (or create) the **mainline** sticky session for a slug — ignores
    /// feature-branch active preference and open draft handles.
    pub fn open_mainline(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let user = current_user_id();
        let ident = resolve_project_identity(slug)?;
        for key in identity_keys(slug, Some(&ident)) {
            self.clear_active_for_project(&key);
        }
        // Local sticky pointer (product slug **or** repo UUID alias)
        for key in identity_keys(slug, Some(&ident)) {
            if let Some(sid) = read_sticky_session(&user, &key) {
                if let Ok(h) = self.attach(&sid) {
                    if !h.snapshot_meta().draft_mode && session_matches_current_repo(&h, slug) {
                        write_sticky_aliases(&user, &ident, &h.session_id());
                        self.set_active_for_identity(&ident, &h.session_id());
                        return Ok(h);
                    }
                    // Wrong repo (re-created project) — clear sticky
                    clear_sticky_session(&user, &key);
                }
            }
        }
        // DDB recent non-draft for **current** repo only (match by repo_id, not string slug)
        if let Ok(list) = list_sessions_for_user(&user) {
            if let Some(m) = list.into_iter().find(|m| {
                !m.draft_mode && m.repo_id == ident.repo_id
            }) {
                let h = self.attach(&m.session_id)?;
                write_sticky_aliases(&user, &ident, &m.session_id);
                self.set_active_for_identity(&ident, &h.session_id());
                return Ok(h);
            }
        }
        let h = self.create(&ident.slug, None)?;
        write_sticky_aliases(&user, &ident, &h.session_id());
        self.set_active_for_identity(&ident, &h.session_id());
        Ok(h)
    }

    /// After `create_project`, drop sessions/sticky that point at a **different**
    /// repo_id for this slug (orphan from a prior same-slug product) and open a
    /// fresh mainline session on the current DDB repo.
    pub fn rebind_after_repo_create(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let ident = resolve_project_identity(slug)?;
        let want = ident.repo_id.clone();
        let user = current_user_id();
        for key in identity_keys(slug, Some(&ident)) {
            self.clear_active_for_project(&key);
            clear_sticky_session(&user, &key);
        }
        // Drop process handles that claim this product slug but wrong repo_id
        {
            let mut map = self.handles.lock().unwrap();
            map.retain(|_, h| {
                let m = h.meta.lock().unwrap();
                let claims_product =
                    m.slug == ident.slug || m.slug == slug || m.repo_id == slug;
                if claims_product {
                    m.repo_id == want
                } else {
                    true
                }
            });
        }
        let inherit_pr = list_sessions_for_user(&user).ok().and_then(|list| {
            list.into_iter()
                .find_map(|m| m.active_pr_id.filter(|s| !s.is_empty()))
        });
        let h = self.create(&ident.slug, None)?;
        if let Some(pr) = inherit_pr {
            let _ = h.set_active_pr_id(Some(&pr));
        }
        write_sticky_aliases(&user, &ident, &h.session_id());
        self.set_active_for_identity(&ident, &h.session_id());
        tracing::info!(
            slug = %ident.slug,
            repo_id = %want,
            session_id = %h.session_id(),
            "rebound coding session after create_project"
        );
        Ok(h)
    }

    /// Attach existing session: load DDB META, ensure workdir (rematerialize if missing).
    pub fn attach(&self, session_id: &str) -> Result<Arc<SessionHandle>, String> {
        {
            let map = self.handles.lock().unwrap();
            if let Some(h) = map.get(session_id) {
                // Hot path: no AWS. Caller must `pull_remote` / merge refresh when S3 advanced.
                h.touch_local();
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
        // Always incremental sync on cold attach so mainline picks up merges from
        // feature branches (S3 is source of truth). Skipping when "warm" left IDE
        // on scaffold after merge_branch promoted domain code to main.
        let attach_branch = meta
            .branch_name
            .clone()
            .unwrap_or_else(|| meta.branch.clone());
        if let Err(e) = materialize_repo_to(
            &meta.repo_id,
            &work_dir,
            MaterializePolicy::SyncIncremental,
            Some(&attach_branch),
        )
        {
            // Only hard-fail if workdir is empty; otherwise serve last local copy.
            if !work_dir.join("veil.toml").is_file() && !has_veil_file(&work_dir) {
                return Err(e);
            }
            tracing::warn!(session_id, error = %e, "attach: S3 sync failed; using existing workdir");
        }
        // Prefer product slug on META when session was created under raw UUID.
        let mut meta = meta;
        if let Ok(ident) = resolve_project_identity(&meta.repo_id) {
            if meta.slug != ident.slug && ident.slug != ident.repo_id {
                tracing::info!(
                    session_id,
                    from = %meta.slug,
                    to = %ident.slug,
                    "normalize session.slug to product slug"
                );
                meta.slug = ident.slug.clone();
                let _ = put_session_meta(&meta);
            }
            write_sticky_aliases(&meta.user_id, &ident, session_id);
        } else {
            write_sticky_session(&meta.user_id, &meta.slug, session_id);
            if meta.repo_id != meta.slug {
                write_sticky_session(&meta.user_id, &meta.repo_id, session_id);
            }
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

    /// After `merge_branch` promotes a draft to S3 main: rematerialize mainline
    /// workdirs for this slug and drop stale in-memory handles so the IDE sees
    /// the merged source (not the pre-merge scaffold).
    pub fn refresh_mainline_after_merge(&self, slug: &str, repo_id: &str) -> Result<(), String> {
        let user = current_user_id();
        let ident = resolve_project_identity(slug).unwrap_or(ProjectIdentity {
            slug: slug.to_string(),
            repo_id: repo_id.to_string(),
        });
        // Drop cached handles for this repo's mainline (wrong memory).
        {
            let mut map = self.handles.lock().unwrap();
            map.retain(|_, h| {
                let m = h.meta.lock().unwrap();
                !(m.repo_id == repo_id && !m.draft_mode)
            });
        }
        // Sync sticky aliases + known mainline workdirs from S3 main.
        for key in identity_keys(slug, Some(&ident)) {
            if let Some(sid) = read_sticky_session(&user, &key) {
                if let Ok(meta) = get_session_meta(&sid) {
                    if meta.repo_id == repo_id && !meta.draft_mode {
                        let work = session_work_dir(&meta.user_id, &meta.session_id, &meta.slug);
                        materialize_repo_to(
                            &meta.repo_id,
                            &work,
                            MaterializePolicy::SyncDelete,
                            Some("main"),
                        )?;
                        write_session_marker(&work, &meta)?;
                        write_sticky_aliases(&user, &ident, &sid);
                    }
                }
            }
        }
        if let Ok(list) = list_sessions_for_user(&user) {
            for meta in list
                .into_iter()
                .filter(|m| m.repo_id == repo_id && !m.draft_mode)
            {
                let work = session_work_dir(&meta.user_id, &meta.session_id, &meta.slug);
                let _ = materialize_repo_to(
                    &meta.repo_id,
                    &work,
                    MaterializePolicy::SyncDelete,
                    Some("main"),
                );
                let _ = write_session_marker(&work, &meta);
            }
        }
        tracing::info!(%slug, %repo_id, "refreshed mainline workdirs after merge");
        Ok(())
    }

    /// Snapshot of open in-memory sessions (for status / health).
    pub fn open_handles_summary(&self) -> Vec<serde_json::Value> {
        let map = self.handles.lock().unwrap();
        map.values()
            .map(|h| {
                let m = h.meta.lock().unwrap();
                serde_json::json!({
                    "session_id": m.session_id,
                    "slug": m.slug,
                    "revision": m.revision,
                    "draft_mode": m.draft_mode,
                    "work_dir": h.work_dir.to_string_lossy(),
                    "active_pr_id": m.active_pr_id,
                })
            })
            .collect()
    }

    /// Get or create a default sticky session for user+slug (compat when no header).
    /// Prefers: active_by_project → process handle → local sticky file → DDB → create.
    ///
    /// Sessions whose `repo_id` no longer matches [`resolve_repo_id`] for the slug
    /// (orphan after re-create) are skipped.
    ///
    /// Product slug and repo UUID are treated as the **same** project so IDE
    /// (`/projects/{uuid}/ide`) and agent (`agent-registry`) share one mainline.
    pub fn get_or_create_default(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let user = current_user_id();
        let ident = resolve_project_identity(slug)?;
        let want_repo = Some(ident.repo_id.clone());
        let keys = identity_keys(slug, Some(&ident));
        // Agent-selected work line (feature branch) wins over sticky main
        for key in &keys {
            if let Some(sid) = self.active_for_project(key) {
                if let Ok(h) = self.attach(&sid) {
                    if session_matches_current_repo(&h, slug) {
                        h.touch_local();
                        self.set_active_for_identity(&ident, &h.session_id());
                        return Ok(h);
                    }
                    self.clear_active_for_project(key);
                }
            }
        }
        // Prefer most recent open handle for **current** repo
        // (draft feature branches preferred over mainline when more recent)
        {
            let map = self.handles.lock().unwrap();
            let mut candidates: Vec<_> = map
                .values()
                .filter(|h| {
                    let m = h.meta.lock().unwrap();
                    m.user_id == user
                        && want_repo
                            .as_ref()
                            .map(|r| &m.repo_id == r)
                            .unwrap_or(false)
                })
                .cloned()
                .collect();
            // Prefer active feature branch (draft) when recent; else mainline.
            // Was: non-draft first — that reattached orphan mainline over create_branch.
            candidates.sort_by_key(|h| {
                let m = h.meta.lock().unwrap();
                std::cmp::Reverse(parse_ts(&m.updated_at))
            });
            if let Some(h) = candidates.into_iter().next() {
                h.touch_local();
                self.set_active_for_identity(&ident, &h.session_id());
                write_sticky_aliases(&user, &ident, &h.session_id());
                return Ok(h);
            }
        }
        // Local sticky pointer (survives process restart without DDB scan cost)
        for key in &keys {
            if let Some(sid) = read_sticky_session(&user, key) {
                if let Ok(h) = self.attach(&sid) {
                    if session_matches_current_repo(&h, slug) {
                        write_sticky_aliases(&user, &ident, &h.session_id());
                        self.set_active_for_identity(&ident, &h.session_id());
                        return Ok(h);
                    }
                    clear_sticky_session(&user, key);
                }
            }
        }
        // Try DDB list for recent non-draft session on current repo (by repo_id)
        if let Ok(list) = list_sessions_for_user(&user) {
            if let Some(m) = list.into_iter().find(|m| {
                !m.draft_mode && m.repo_id == ident.repo_id
            }) {
                let h = self.attach(&m.session_id)?;
                write_sticky_aliases(&user, &ident, &m.session_id);
                self.set_active_for_identity(&ident, &h.session_id());
                return Ok(h);
            }
        }
        let h = self.create(&ident.slug, None)?;
        write_sticky_aliases(&user, &ident, &h.session_id());
        self.set_active_for_identity(&ident, &h.session_id());
        Ok(h)
    }

    /// After a successful mainline source write: bump the writing session's
    /// revision (so Uncommitted is true) and rematerialize **other** mainline
    /// workdirs for the same repo so a second sticky clone cannot stay stale.
    pub fn record_source_write(
        &self,
        project_key: &str,
        path: &str,
        writing_session_id: Option<&str>,
    ) -> u64 {
        let ident = resolve_project_identity(project_key).ok();
        let repo_id = ident
            .as_ref()
            .map(|i| i.repo_id.clone())
            .or_else(|| resolve_repo_id(project_key).ok());

        let mut rev = 0u64;
        // Prefer the active feature work line over a stale mainline CURRENT_SESSION
        // (IDE still sending the old session header after create_branch).
        let active_feature = ident
            .as_ref()
            .and_then(|i| self.active_for_project(&i.slug))
            .or_else(|| self.active_for_project(project_key))
            .and_then(|sid| {
                self.attach(&sid).ok().and_then(|h| {
                    if h.snapshot_meta().draft_mode {
                        Some(h.session_id())
                    } else {
                        None
                    }
                })
            });

        let writer = active_feature
            .or_else(|| writing_session_id.map(|s| s.to_string()))
            .or_else(current_session_id)
            .or_else(|| {
                ident
                    .as_ref()
                    .and_then(|i| self.active_for_project(&i.slug))
                    .or_else(|| self.active_for_project(project_key))
            });

        if let Some(ref sid) = writer {
            if let Ok(h) = self.attach(sid) {
                rev = h.record_write(path);
                if let Some(ref id) = ident {
                    // Never rewrite sticky mainline pointer to a feature branch.
                    if !h.snapshot_meta().draft_mode {
                        write_sticky_aliases(&current_user_id(), id, sid);
                    }
                    self.set_active_for_identity(id, sid);
                }
            }
        } else if let Ok(h) = self.resolve_for_project(project_key) {
            rev = h.record_write(path);
        }

        // Invalidate other mainline clones for this repo (stale local workdirs).
        if let Some(repo) = repo_id.as_deref() {
            self.refresh_other_mainline_workdirs(repo, writer.as_deref());
        }
        rev
    }

    /// Drop + rematerialize mainline sessions for `repo_id` except `keep_session`.
    fn refresh_other_mainline_workdirs(&self, repo_id: &str, keep_session: Option<&str>) {
        let user = current_user_id();
        let mut to_sync: Vec<(String, String, String)> = Vec::new(); // sid, slug, work
        {
            let map = self.handles.lock().unwrap();
            for (sid, h) in map.iter() {
                if keep_session == Some(sid.as_str()) {
                    continue;
                }
                let m = h.meta.lock().unwrap();
                if m.repo_id == repo_id && !m.draft_mode {
                    to_sync.push((
                        sid.clone(),
                        m.slug.clone(),
                        h.work_dir.to_string_lossy().to_string(),
                    ));
                }
            }
        }
        // Also sticky/DDB mainline sessions not currently in memory
        if let Ok(list) = list_sessions_for_user(&user) {
            for m in list
                .into_iter()
                .filter(|m| m.repo_id == repo_id && !m.draft_mode)
            {
                if keep_session == Some(m.session_id.as_str()) {
                    continue;
                }
                if to_sync.iter().any(|(s, _, _)| s == &m.session_id) {
                    continue;
                }
                let work = session_work_dir(&m.user_id, &m.session_id, &m.slug);
                to_sync.push((
                    m.session_id.clone(),
                    m.slug.clone(),
                    work.to_string_lossy().to_string(),
                ));
            }
        }
        for (sid, _slug, work) in to_sync {
            // Drop warm handle so next attach reloads from disk after sync.
            self.drop_handle(&sid);
            let work_path = PathBuf::from(&work);
            if let Err(e) =
                materialize_repo_to(
                    repo_id,
                    &work_path,
                    MaterializePolicy::SyncIncremental,
                    Some("main"),
                )
            {
                tracing::warn!(
                    session_id = %sid,
                    %repo_id,
                    error = %e,
                    "refresh peer mainline workdir after write failed"
                );
            } else {
                tracing::info!(
                    session_id = %sid,
                    %repo_id,
                    "refreshed peer mainline workdir after write"
                );
            }
        }
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
    branch: Option<&str>,
) -> Result<(), String> {
    if crate::git_origin::origin_enabled() {
        let origin = crate::git_origin::GitOrigin::new(repo_id);
        let br = branch.unwrap_or("main");
        let mode = match policy {
            MaterializePolicy::SyncDelete => crate::git_origin::CheckoutMode::ResetHard,
            MaterializePolicy::SyncIncremental => crate::git_origin::CheckoutMode::FetchKeepDirty,
        };
        match origin.checkout(work, br, mode) {
            Ok(_) => return Ok(()),
            Err(e) => {
                // First open of a pre-git product: import the legacy S3 tree.
                if let Ok(Some(_)) = origin.import_legacy_tree(br) {
                    origin.checkout(work, br, mode)?;
                    return Ok(());
                }
                tracing::warn!(%repo_id, error = %e, "git origin checkout failed; falling back to tree sync");
            }
        }
    }
    match policy {
        MaterializePolicy::SyncDelete => materialize_repo(repo_id, work),
        MaterializePolicy::SyncIncremental => {
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
        "draft_mode": meta.draft_mode,
        "branch_name": meta.branch_name,
        "base_branch": meta.base_branch,
        "head_commit": meta.head_commit,
        "committed_revision": meta.committed_revision,
    });
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| format!("write session marker: {e}"))
}

fn sticky_path(user_id: &str, slug: &str) -> PathBuf {
    ws_root()
        .join(".sticky")
        .join(user_id)
        .join(format!("{slug}.session"))
}

fn write_sticky_session(user_id: &str, slug: &str, session_id: &str) {
    let p = sticky_path(user_id, slug);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(p, session_id);
}

/// Sticky under product slug **and** repo UUID so either route reopens the same session.
fn write_sticky_aliases(user_id: &str, ident: &ProjectIdentity, session_id: &str) {
    write_sticky_session(user_id, &ident.slug, session_id);
    if ident.repo_id != ident.slug {
        write_sticky_session(user_id, &ident.repo_id, session_id);
    }
}

fn read_sticky_session(user_id: &str, slug: &str) -> Option<String> {
    let p = sticky_path(user_id, slug);
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn clear_sticky_session(user_id: &str, slug: &str) {
    let p = sticky_path(user_id, slug);
    let _ = std::fs::remove_file(p);
}

/// Lookup keys for sticky / active maps: raw input, product slug, repo id.
fn identity_keys(raw: &str, ident: Option<&ProjectIdentity>) -> Vec<String> {
    let mut keys = Vec::new();
    let push = |keys: &mut Vec<String>, s: &str| {
        let s = s.trim();
        if !s.is_empty() && !keys.iter().any(|k| k == s) {
            keys.push(s.to_string());
        }
    };
    push(&mut keys, raw);
    if let Some(id) = ident {
        push(&mut keys, &id.slug);
        push(&mut keys, &id.repo_id);
    }
    keys
}

/// True when session.repo_id matches the live product repo for `slug`.
fn session_matches_current_repo(h: &SessionHandle, slug: &str) -> bool {
    match resolve_project_identity(slug) {
        Ok(want) => h.snapshot_meta().repo_id == want.repo_id,
        // If we cannot resolve (offline), accept the session rather than dead-end.
        Err(_) => match resolve_repo_id(slug) {
            Ok(want) => h.snapshot_meta().repo_id == want,
            Err(_) => true,
        },
    }
}

/// RFC3339 UTC timestamp without extra crates.
pub fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Approximate RFC3339 (good enough for sort + display)
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    // Civil date from days since epoch (algorithm from civil_from_days)
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Parse session timestamp (RFC3339 or unix seconds) → unix secs.
pub fn parse_ts(s: &str) -> u64 {
    if let Ok(n) = s.parse::<u64>() {
        return n;
    }
    // Minimal RFC3339: YYYY-MM-DDTHH:MM:SSZ
    if s.len() >= 19 {
        // Fallback: use file mtime style — just hash length for sort stability
        // Prefer numeric prefix if any
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).take(14).collect();
        if let Ok(n) = digits.parse::<u64>() {
            return n;
        }
    }
    0
}

pub fn session_ttl_secs() -> u64 {
    std::env::var("VEIL_SESSION_TTL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(86_400)
}

/// Drop idle in-memory handles older than TTL (META left for resume).
pub fn reap_idle_handles(mgr: &SessionManager) -> usize {
    let ttl = session_ttl_secs();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut drop_ids = Vec::new();
    {
        let map = mgr.handles.lock().unwrap();
        for (id, h) in map.iter() {
            let last = parse_ts(&h.meta.lock().unwrap().last_activity_at);
            // If timestamp is RFC3339-ish large number from digits, compare carefully
            let age = if last > 1_000_000_000_000 {
                // packed yyyymmddhhmmss — skip precise reap
                0
            } else if last > 0 && now > last {
                now - last
            } else {
                0
            };
            if age > ttl {
                drop_ids.push(id.clone());
            }
        }
    }
    for id in &drop_ids {
        mgr.drop_handle(id);
        tracing::info!(%id, "reaped idle session handle");
    }
    drop_ids.len()
}

/// Start background reaper (once).
pub fn spawn_session_reaper() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        if !sessions_enabled() {
            return;
        }
        std::thread::Builder::new()
            .name("veil-session-reaper".into())
            .spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(300));
                let n = reap_idle_handles(SessionManager::global());
                if n > 0 {
                    tracing::info!(reaped = n, "session reaper tick");
                }
            })
            .ok();
    });
}

/// Response headers for durable writes.
pub fn durable_headers(
    session_id: Option<&str>,
    revision: Option<u64>,
    etag: Option<&str>,
) -> Vec<(axum::http::HeaderName, axum::http::HeaderValue)> {
    use axum::http::{HeaderName, HeaderValue};
    let mut out = Vec::new();
    if let Some(sid) = session_id {
        if let Ok(v) = HeaderValue::from_str(sid) {
            out.push((HeaderName::from_static("x-veil-session-id"), v));
        }
    }
    if let Some(r) = revision {
        if let Ok(v) = HeaderValue::from_str(&r.to_string()) {
            out.push((HeaderName::from_static("x-veil-revision"), v));
        }
    }
    if let Some(e) = etag {
        let clean = e.trim_matches('"');
        if let Ok(v) = HeaderValue::from_str(clean) {
            out.push((HeaderName::from_static("x-veil-etag"), v));
        }
    }
    out
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
        let handle = Self {
            meta: Mutex::new(meta),
            work_dir,
            provider,
            fs,
        };
        // PR Wizard intents: reload durable rationales into process cache.
        crate::api::rehydrate_rationales_from_session(&handle);
        Ok(handle)
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

    /// Cheap activity bump (no AWS). Durable flush is debounced separately.
    pub fn touch_local(&self) {
        let mut m = self.meta.lock().unwrap();
        m.last_activity_at = chrono_now();
        // Do not put_session_meta here — that is multi-second AWS CLI on every /ir hit.
    }

    /// Persist META to DDB (slow). Call sparingly (create, bump_revision, idle flush).
    pub fn flush_meta(&self) {
        let snap = self.meta.lock().unwrap().clone();
        let _ = put_session_meta(&snap);
    }

    pub fn bump_revision(&self, path: &str, etag: Option<String>) -> u64 {
        let mut m = self.meta.lock().unwrap();
        m.revision = m.revision.saturating_add(1);
        m.updated_at = chrono_now();
        m.last_activity_at = m.updated_at.clone();
        if let Some(e) = etag {
            m.etags.insert(path.to_string(), e);
        }
        // `dirty` used to mean "needs S3 flush"; write-through already put S3.
        // Keep path listed so IDE Uncommitted / dirty_files reflects working tree
        // changes since last `session_commit` (cleared on commit).
        if !path.is_empty() && !m.dirty.iter().any(|p| p == path) {
            m.dirty.push(path.to_string());
        }
        m.writes_since_commit = m.writes_since_commit.saturating_add(1);
        let rev = m.revision;
        let snap = m.clone();
        drop(m);
        // Debounced durable flush (not every keystroke / tool write)
        schedule_meta_flush(snap);
        rev
    }

    /// Record an agent/operator source write: bump revision + dirty file list.
    /// Returns the new revision (0 if unchanged path empty — still bumps).
    pub fn record_write(&self, path: &str) -> u64 {
        let p = if path.is_empty() { "main.veil" } else { path };
        self.bump_revision(p, None)
    }

    /// Persist host check result used by coding gates (submit/PR messaging).
    pub fn set_last_host_check(&self, snap: HostCheckSnapshot) {
        let mut m = self.meta.lock().unwrap();
        m.last_host_check = Some(snap);
        m.last_activity_at = chrono_now();
        let meta = m.clone();
        drop(m);
        schedule_meta_flush(meta);
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
        schedule_meta_flush(snap);
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
        let br = m.branch_name.clone().unwrap_or(m.branch.clone());
        materialize_repo_to(
            &m.repo_id,
            &self.work_dir,
            MaterializePolicy::SyncIncremental,
            Some(&br),
        )
    }

    /// Hard reset workdir from remote.
    pub fn reset_to_remote(&self) -> Result<(), String> {
        let m = self.meta.lock().unwrap().clone();
        let br = m.branch_name.clone().unwrap_or(m.branch.clone());
        materialize_repo_to(
            &m.repo_id,
            &self.work_dir,
            MaterializePolicy::SyncDelete,
            Some(&br),
        )
    }

    /// **commit**: native `git commit` + push branch to S3 origin.
    /// Autosaves continue; this is an explicit checkpoint.
    /// Refuses when there is nothing uncommitted (coding gate).
    pub fn commit(&self, message: &str) -> Result<SessionCommit, String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("commit message required".into());
        }
        if !self.has_uncommitted() {
            return Err(
                "nothing to commit — working tree clean (no writes since last session_commit). \
                 Edit with write_source first, or skip commit on explore turns."
                    .into(),
            );
        }
        let m = self.meta.lock().unwrap().clone();
        let branch = m
            .branch_name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| m.branch.clone());

        let (commit_id, snapshot_prefix, files, parent) =
            if crate::git_origin::origin_enabled() {
                let origin = crate::git_origin::GitOrigin::new(&m.repo_id);
                let info = origin.commit_and_push(&self.work_dir, message, &branch)?;
                let prefix = format!(
                    "git/{}/refs/heads/{}/{}.bundle",
                    m.repo_id, info.branch, info.sha
                );
                (info.sha, prefix, info.files, info.parent)
            } else {
                let commit_id = Uuid::new_v4().to_string();
                let short = &commit_id[..8.min(commit_id.len())];
                let snapshot_prefix =
                    format!("repos/{}/commits/{}/{}/", m.repo_id, m.session_id, short);
                let bucket = std::env::var("BUCKET")
                    .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
                    .unwrap_or_else(|_| "veil-runtime-dev".into());
                let dest = format!("s3://{bucket}/{snapshot_prefix}");
                let mut cmd = std::process::Command::new("aws");
                if let Ok(p) = std::env::var("AWS_PROFILE") {
                    cmd.env("AWS_PROFILE", p);
                }
                let out = cmd
                    .args([
                        "s3",
                        "sync",
                        &self.work_dir.to_string_lossy(),
                        &dest,
                        "--exclude",
                        ".veil-session.json",
                        "--exclude",
                        ".git/*",
                        "--exclude",
                        "target/*",
                        "--exclude",
                        "generated/*",
                    ])
                    .output()
                    .map_err(|e| format!("aws s3 sync commit: {e}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "commit snapshot failed: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
                (
                    commit_id,
                    snapshot_prefix,
                    list_work_files(&self.work_dir),
                    m.head_commit.clone(),
                )
            };

        let now = chrono_now();
        let commit = SessionCommit {
            commit_id: commit_id.clone(),
            session_id: m.session_id.clone(),
            message: message.to_string(),
            parent,
            snapshot_prefix: snapshot_prefix.clone(),
            revision: m.revision,
            files,
            branch_name: Some(branch),
            created_at: now.clone(),
            author: Some(m.user_id.clone()),
        };
        // Git is the commit graph. DDB COMMIT# is not a second history.
        if !crate::git_origin::origin_enabled() {
            put_session_commit(&commit)?;
        }

        {
            let mut meta = self.meta.lock().unwrap();
            meta.head_commit = Some(commit_id);
            meta.committed_revision = Some(meta.revision);
            meta.dirty.clear();
            meta.writes_since_commit = 0;
            meta.updated_at = now;
            let snap = meta.clone();
            drop(meta);
            let _ = put_session_meta(&snap);
            let _ = write_session_marker(&self.work_dir, &snap);
        }
        Ok(commit)
    }

    /// Push this session's branch to origin (and refresh the checkout cache).
    /// Does **not** merge to main — use PR Wizard → Approve → Merge.
    pub fn publish_to_branch(&self, branch: &str) -> Result<serde_json::Value, String> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err("branch name required".into());
        }
        if branch == "main" || branch == "master" {
            return Err(
                "refuse publish to main/master — open a PR and merge via PR Wizard".into(),
            );
        }
        let m = self.meta.lock().unwrap().clone();
        if crate::git_origin::origin_enabled() {
            let origin = crate::git_origin::GitOrigin::new(&m.repo_id);
            if self.work_dir.join(".git").is_dir() {
                let _ = origin.create_branch(&self.work_dir, branch);
            }
            let sha = origin.push(&self.work_dir, branch)?;
            return Ok(serde_json::json!({
                "ok": true,
                "published_to": branch,
                "dest_prefix": format!("git/{}/refs/heads/{}/", m.repo_id, branch),
                "repo_id": m.repo_id,
                "session_id": m.session_id,
                "revision": m.revision,
                "head_commit": sha,
            }));
        }
        let bucket = std::env::var("BUCKET")
            .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
            .unwrap_or_else(|_| "veil-runtime-dev".into());
        let dest_prefix = format!("repos/{}/{}/", m.repo_id, branch);
        let dest = format!("s3://{bucket}/{dest_prefix}");

        let mut cmd = std::process::Command::new("aws");
        if let Ok(p) = std::env::var("AWS_PROFILE") {
            cmd.env("AWS_PROFILE", p);
        }
        let out = cmd
            .args([
                "s3",
                "sync",
                &self.work_dir.to_string_lossy(),
                &dest,
                "--exclude",
                ".veil-session.json",
                "--exclude",
                ".git/*",
                "--exclude",
                "target/*",
                "--exclude",
                "generated/*",
            ])
            .output()
            .map_err(|e| format!("aws s3 sync publish: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "publish to branch {branch} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(serde_json::json!({
            "ok": true,
            "published_to": branch,
            "dest_prefix": dest_prefix,
            "repo_id": m.repo_id,
            "session_id": m.session_id,
            "revision": m.revision,
            "head_commit": m.head_commit,
        }))
    }

    /// Remember open PR for agent reply writeback.
    pub fn set_active_pr_id(&self, pr_id: Option<&str>) -> Result<(), String> {
        let mut meta = self.meta.lock().unwrap();
        meta.active_pr_id = pr_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        meta.updated_at = chrono_now();
        let snap = meta.clone();
        drop(meta);
        put_session_meta(&snap)?;
        let _ = write_session_marker(&self.work_dir, &snap);
        Ok(())
    }

    /// Merge construct rationales into durable session meta (PR Wizard intents).
    pub fn merge_rationales(&self, map: &HashMap<String, String>) -> Result<(), String> {
        if map.is_empty() {
            return Ok(());
        }
        let mut meta = self.meta.lock().unwrap();
        for (k, v) in map {
            let name = k.trim();
            let intent = v.trim();
            if name.is_empty() || intent.is_empty() {
                continue;
            }
            meta.rationales.insert(name.to_string(), intent.to_string());
        }
        meta.updated_at = chrono_now();
        let snap = meta.clone();
        drop(meta);
        put_session_meta(&snap)?;
        let _ = write_session_marker(&self.work_dir, &snap);
        Ok(())
    }

    /// Snapshot of durable rationales for cache rehydrate.
    pub fn snapshot_rationales(&self) -> HashMap<String, String> {
        self.meta.lock().unwrap().rationales.clone()
    }

    /// Promote this branch's working tree onto the product **base** branch in S3
    /// (git-shaped merge). Only for draft/feature branch sessions.
    ///
    /// **Gate:** blocked by default so humans use the PR Wizard. Set
    /// `VEIL_ALLOW_SESSION_MERGE=1` or pass `force: true` via API for escape hatch.
    pub fn merge_to_base(&self) -> Result<serde_json::Value, String> {
        self.merge_to_base_gated(false)
    }

    pub fn merge_to_base_gated(&self, force: bool) -> Result<serde_json::Value, String> {
        let allow_env = std::env::var("VEIL_ALLOW_SESSION_MERGE")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        if !force && !allow_env {
            return Err(
                "Session merge to main is disabled. Open the PR Wizard (Review), \
                 approve structural changes, then Merge from the PR. \
                 Escape hatch: VEIL_ALLOW_SESSION_MERGE=1 or force=true."
                    .into(),
            );
        }
        let m = self.meta.lock().unwrap().clone();
        if !m.draft_mode {
            return Err("already on base (mainline) session — nothing to merge".into());
        }
        let base = m
            .base_branch
            .clone()
            .unwrap_or_else(default_branch);
        let source = m
            .branch_name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("work/{}", &m.session_id[..8.min(m.session_id.len())]));
        let dest_prefix = if crate::git_origin::origin_enabled() {
            let origin = crate::git_origin::GitOrigin::new(&m.repo_id);
            origin.merge_and_push(&self.work_dir, &source, &base)?;
            format!("git/{}/refs/heads/{}/", m.repo_id, base)
        } else {
            let bucket = std::env::var("BUCKET")
                .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
                .unwrap_or_else(|_| "veil-runtime-dev".into());
            let dest_prefix = format!("repos/{}/{}/", m.repo_id, base);
            let dest = format!("s3://{bucket}/{dest_prefix}");

            let mut cmd = std::process::Command::new("aws");
            if let Ok(p) = std::env::var("AWS_PROFILE") {
                cmd.env("AWS_PROFILE", p);
            }
            let out = cmd
                .args([
                    "s3",
                    "sync",
                    &self.work_dir.to_string_lossy(),
                    &dest,
                    "--exclude",
                    ".veil-session.json",
                    "--exclude",
                    ".git/*",
                    "--exclude",
                    "target/*",
                    "--exclude",
                    "generated/*",
                ])
                .output()
                .map_err(|e| format!("aws s3 sync merge: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "merge to {base} failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            dest_prefix
        };
        // Mainline sessions still hold pre-merge scaffold in their workdirs.
        // Rematerialize so IDE open shows the merged product tree.
        if let Err(e) =
            SessionManager::global().refresh_mainline_after_merge(&m.slug, &m.repo_id)
        {
            tracing::warn!(error = %e, slug = %m.slug, "post-merge mainline refresh failed");
        }
        Ok(serde_json::json!({
            "ok": true,
            "merged_to": base,
            "dest_prefix": dest_prefix,
            "from_branch": m.branch_name,
            "session_id": m.session_id,
            "head_commit": m.head_commit,
            "refreshed_mainline": true,
        }))
    }

    pub fn has_uncommitted(&self) -> bool {
        if crate::git_origin::origin_enabled() && self.work_dir.join(".git").is_dir() {
            return crate::git_origin::status_dirty(&self.work_dir).unwrap_or(false);
        }
        let m = self.meta.lock().unwrap();
        let committed = m.committed_revision.unwrap_or(0);
        m.revision > committed || !m.dirty.is_empty()
    }

    /// Commit log from **git**, not DDB.
    pub fn git_log(&self, n: usize) -> Result<Vec<crate::git_origin::LogEntry>, String> {
        if !self.work_dir.join(".git").is_dir() {
            return Ok(vec![]);
        }
        let repo = self.meta.lock().unwrap().repo_id.clone();
        crate::git_origin::GitOrigin::new(repo).log(&self.work_dir, n)
    }

    pub fn git_status_files(&self) -> Vec<crate::git_origin::StatusFile> {
        crate::git_origin::GitOrigin::status_files(&self.work_dir).unwrap_or_default()
    }

    pub fn git_working_diff(&self) -> String {
        crate::git_origin::GitOrigin::working_diff(&self.work_dir).unwrap_or_default()
    }
}

fn list_work_files(work: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git"
                || name == "target"
                || name == "generated"
                || name == "node_modules"
                || name == ".veil-session.json"
            {
                continue;
            }
            if p.is_dir() {
                walk(&p, root, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let s = rel.to_string_lossy().replace('\\', "/");
                if s.ends_with(".veil")
                    || s.ends_with(".layer")
                    || s == "veil.toml"
                    || s == "MISSION.md"
                {
                    out.push(s);
                }
            }
        }
    }
    walk(work, work, &mut out);
    out.sort();
    out
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

/// Debounce DDB META puts — AWS CLI + SSO is multi-second and was on every IDE request.
fn schedule_meta_flush(meta: SessionMeta) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    struct Slot {
        pending: Option<SessionMeta>,
        last_flush: Instant,
        thread_started: bool,
    }
    static SLOT: std::sync::OnceLock<Mutex<Slot>> = std::sync::OnceLock::new();
    let slot = SLOT.get_or_init(|| {
        Mutex::new(Slot {
            pending: None,
            last_flush: Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            thread_started: false,
        })
    });
    let mut g = match slot.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    g.pending = Some(meta);
    if !g.thread_started {
        g.thread_started = true;
        drop(g);
        let _ = std::thread::Builder::new()
            .name("veil-session-meta-flush".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(5));
                let to_write = {
                    let mut g = match slot.lock() {
                        Ok(g) => g,
                        Err(_) => continue,
                    };
                    // Flush at most every 15s unless process is idle-ish
                    if g.last_flush.elapsed() < Duration::from_secs(15) {
                        continue;
                    }
                    g.pending.take()
                };
                if let Some(m) = to_write {
                    if put_session_meta(&m).is_ok() {
                        if let Ok(mut g) = slot.lock() {
                            g.last_flush = Instant::now();
                        }
                    }
                }
            });
    }
}

pub fn current_session_id() -> Option<String> {
    CURRENT_SESSION.try_with(|s| s.clone()).ok()
}
