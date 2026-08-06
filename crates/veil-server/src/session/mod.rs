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
    append_turn, delete_session_meta, get_session_commit, get_session_meta, list_session_commits,
    list_sessions_for_user, list_turns, merge_session_focus_intents, put_session_commit,
    put_session_meta, touch_session, SessionCommit, SessionMeta, SessionTurn,
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
    /// git-shaped **feature branch**: isolated S3 draft prefix + named work line.
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
        let repo_id = resolve_repo_id(slug)?;
        let work_dir = session_work_dir(&user_id, &session_id, slug);
        let work_prefix = if draft_mode {
            format!("repos/{repo_id}/drafts/{session_id}/")
        } else {
            format!("repos/{repo_id}/{base}/")
        };

        // Materialize from **base** product tree (main), then work in isolation
        materialize_repo_to(&repo_id, &work_dir, MaterializePolicy::SyncIncremental)?;

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
        };
        put_session_meta(&meta)?;
        write_session_marker(&work_dir, &meta)?;

        // Sticky only for mainline sessions — feature branches are explicit
        if !draft_mode {
            write_sticky_session(&user_id, slug, &session_id);
        }
        let handle = Arc::new(SessionHandle::open(meta, work_dir)?);
        self.handles
            .lock()
            .unwrap()
            .insert(session_id.clone(), handle.clone());
        // Feature branches become the active work line for this project immediately
        if draft_mode {
            self.set_active_for_project(slug, &session_id);
        }
        Ok(handle)
    }

    /// Remember which session the agent/operator is working on for `slug`.
    pub fn set_active_for_project(&self, slug: &str, session_id: &str) {
        self.active_by_project
            .lock()
            .unwrap()
            .insert(slug.to_string(), session_id.to_string());
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
        if let Some(sid) = self.active_for_project(slug) {
            if let Ok(h) = self.attach(&sid) {
                if h.slug() == slug {
                    return Ok(h);
                }
            }
        }
        self.get_or_create_default(slug)
    }

    /// Open (or create) the **mainline** sticky session for a slug — ignores
    /// feature-branch active preference and open draft handles.
    pub fn open_mainline(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let user = current_user_id();
        self.clear_active_for_project(slug);
        // Local sticky pointer
        if let Some(sid) = read_sticky_session(&user, slug) {
            if let Ok(h) = self.attach(&sid) {
                if h.slug() == slug && !h.snapshot_meta().draft_mode {
                    self.set_active_for_project(slug, &h.session_id());
                    return Ok(h);
                }
            }
        }
        // DDB recent non-draft
        if let Ok(list) = list_sessions_for_user(&user) {
            if let Some(m) = list.into_iter().find(|m| m.slug == slug && !m.draft_mode) {
                let h = self.attach(&m.session_id)?;
                write_sticky_session(&user, slug, &m.session_id);
                self.set_active_for_project(slug, &h.session_id());
                return Ok(h);
            }
        }
        let h = self.create(slug, None)?;
        write_sticky_session(&user, slug, &h.session_id());
        self.set_active_for_project(slug, &h.session_id());
        Ok(h)
    }

    /// Attach existing session: load DDB META, ensure workdir (rematerialize if missing).
    pub fn attach(&self, session_id: &str) -> Result<Arc<SessionHandle>, String> {
        {
            let map = self.handles.lock().unwrap();
            if let Some(h) = map.get(session_id) {
                // In-memory only — never shell out to AWS on the hot path
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
        // Warm workdir: skip S3 entirely when already materialized (huge load win)
        if !work_dir.join("veil.toml").is_file() && !has_veil_file(&work_dir) {
            materialize_repo_to(&meta.repo_id, &work_dir, MaterializePolicy::SyncIncremental)?;
        }
        write_session_marker(&work_dir, &meta)?;
        write_sticky_session(&meta.user_id, &meta.slug, session_id);
        let handle = Arc::new(SessionHandle::open(meta, work_dir)?);
        self.handles
            .lock()
            .unwrap()
            .insert(session_id.to_string(), handle.clone());
        let _ = touch_session(session_id);
        Ok(handle)
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
                })
            })
            .collect()
    }

    /// Get or create a default sticky session for user+slug (compat when no header).
    /// Prefers: active_by_project → process handle → local sticky file → DDB → create.
    pub fn get_or_create_default(&self, slug: &str) -> Result<Arc<SessionHandle>, String> {
        let user = current_user_id();
        // Agent-selected work line (feature branch) wins over sticky main
        if let Some(sid) = self.active_for_project(slug) {
            if let Ok(h) = self.attach(&sid) {
                if h.slug() == slug {
                    h.touch_local();
                    return Ok(h);
                }
            }
        }
        // Prefer most recent open handle for slug (non-draft preferred)
        {
            let map = self.handles.lock().unwrap();
            let mut candidates: Vec<_> = map
                .values()
                .filter(|h| {
                    let m = h.meta.lock().unwrap();
                    m.slug == slug && m.user_id == user
                })
                .cloned()
                .collect();
            candidates.sort_by_key(|h| {
                let m = h.meta.lock().unwrap();
                (m.draft_mode, std::cmp::Reverse(parse_ts(&m.updated_at)))
            });
            if let Some(h) = candidates.into_iter().next() {
                h.touch_local();
                return Ok(h);
            }
        }
        // Local sticky pointer (survives process restart without DDB scan cost)
        if let Some(sid) = read_sticky_session(&user, slug) {
            if let Ok(h) = self.attach(&sid) {
                if h.slug() == slug {
                    return Ok(h);
                }
            }
        }
        // Try DDB list for recent non-draft session
        if let Ok(list) = list_sessions_for_user(&user) {
            if let Some(m) = list
                .into_iter()
                .find(|m| m.slug == slug && !m.draft_mode)
            {
                let h = self.attach(&m.session_id)?;
                write_sticky_session(&user, slug, &m.session_id);
                return Ok(h);
            }
        }
        let h = self.create(slug, None)?;
        write_sticky_session(&user, slug, &h.session_id());
        Ok(h)
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

fn read_sticky_session(user_id: &str, slug: &str) -> Option<String> {
    let p = sticky_path(user_id, slug);
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
        m.dirty.retain(|p| p != path);
        let rev = m.revision;
        let snap = m.clone();
        drop(m);
        // Debounced durable flush (not every keystroke / tool write)
        schedule_meta_flush(snap);
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
        materialize_repo_to(&m.repo_id, &self.work_dir, MaterializePolicy::SyncIncremental)
    }

    /// Hard reset workdir from remote.
    pub fn reset_to_remote(&self) -> Result<(), String> {
        let m = self.meta.lock().unwrap().clone();
        materialize_repo_to(&m.repo_id, &self.work_dir, MaterializePolicy::SyncDelete)
    }

    /// Git-shaped **commit**: snapshot working tree + named message.
    /// Autosaves continue; this is an explicit checkpoint.
    pub fn commit(&self, message: &str) -> Result<SessionCommit, String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("commit message required".into());
        }
        let m = self.meta.lock().unwrap().clone();
        let commit_id = Uuid::new_v4().to_string();
        let short = &commit_id[..8.min(commit_id.len())];
        let snapshot_prefix = format!("repos/{}/commits/{}/{}/", m.repo_id, m.session_id, short);
        let bucket = std::env::var("BUCKET")
            .or_else(|_| std::env::var("VEIL_S3_BUCKET"))
            .unwrap_or_else(|_| "veil-runtime-dev".into());
        let dest = format!("s3://{bucket}/{snapshot_prefix}");

        // Snapshot workdir → S3 (exclude session marker noise)
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

        let files = list_work_files(&self.work_dir);
        let now = chrono_now();
        let commit = SessionCommit {
            commit_id: commit_id.clone(),
            session_id: m.session_id.clone(),
            message: message.to_string(),
            parent: m.head_commit.clone(),
            snapshot_prefix: snapshot_prefix.clone(),
            revision: m.revision,
            files,
            branch_name: m.branch_name.clone(),
            created_at: now.clone(),
            author: Some(m.user_id.clone()),
        };
        put_session_commit(&commit)?;

        {
            let mut meta = self.meta.lock().unwrap();
            meta.head_commit = Some(commit_id);
            meta.committed_revision = Some(meta.revision);
            meta.dirty.clear();
            meta.updated_at = now;
            let snap = meta.clone();
            drop(meta);
            let _ = put_session_meta(&snap);
            let _ = write_session_marker(&self.work_dir, &snap);
        }
        Ok(commit)
    }

    /// Promote this branch's working tree onto the product **base** branch in S3
    /// (git-shaped merge). Only for draft/feature branch sessions.
    pub fn merge_to_base(&self) -> Result<serde_json::Value, String> {
        let m = self.meta.lock().unwrap().clone();
        if !m.draft_mode {
            return Err("already on base (mainline) session — nothing to merge".into());
        }
        let base = m
            .base_branch
            .clone()
            .unwrap_or_else(default_branch);
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
        Ok(serde_json::json!({
            "ok": true,
            "merged_to": base,
            "dest_prefix": dest_prefix,
            "from_branch": m.branch_name,
            "session_id": m.session_id,
            "head_commit": m.head_commit,
        }))
    }

    pub fn has_uncommitted(&self) -> bool {
        let m = self.meta.lock().unwrap();
        let committed = m.committed_revision.unwrap_or(0);
        m.revision > committed || !m.dirty.is_empty()
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
