//! HTTP API for durable coding sessions and workspace tools.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::session::{
    append_turn, current_user_id, delete_session_meta, list_session_commits, list_sessions_for_user,
    list_turns, sessions_enabled, SessionManager, SessionTurn, WorkspaceFs,
};
use crate::provider::hub::MultiProjectProvider;
use std::sync::Arc;

pub fn session_routes() -> Router<Arc<MultiProjectProvider>> {
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}", get(get_session).delete(close_session))
        .route("/api/sessions/{id}/attach", post(attach_session))
        .route("/api/sessions/{id}/pull", post(pull_session))
        .route("/api/sessions/{id}/reset", post(reset_session))
        .route("/api/sessions/{id}/flush", post(flush_session))
        .route("/api/sessions/{id}/turns", get(get_turns).post(post_turn))
        // Git-shaped workflow
        .route(
            "/api/sessions/{id}/commits",
            get(list_commits).post(create_commit),
        )
        .route("/api/sessions/{id}/diff", get(session_git_diff))
        .route("/api/sessions/{id}/merge", post(merge_session))
        .route("/api/sessions/{id}/publish-branch", post(publish_branch))
        .route(
            "/api/sessions/{id}/active-change",
            post(set_active_pr),
        )
        // Workspace tools (also exposed via MCP)
        .route("/api/sessions/{id}/ws/list", post(ws_list))
        .route("/api/sessions/{id}/ws/read", post(ws_read))
        .route("/api/sessions/{id}/ws/write", post(ws_write))
        .route("/api/sessions/{id}/ws/str_replace", post(ws_str_replace))
        .route("/api/sessions/{id}/ws/grep", post(ws_grep))
        .route("/api/sessions/{id}/ws/rm", post(ws_rm))
        // Intent Present commit targets (Agent → UX → Server product path)
        .route("/api/ux/create_project", post(ux_create_project))
        .route("/api/ux/create_pr", post(ux_create_pr))
        .route("/api/ux/intent_log", post(ux_intent_log).get(ux_intent_log_list))
        .route("/api/ux/intent_ack", post(ux_intent_ack))
        .route("/api/ux/intent_ack/{id}", get(ux_intent_ack_get))
        .route("/api/ux/sign_off", post(ux_sign_off))
        .route("/api/review/outstanding", get(review_outstanding))
        .route("/api/review/edits", get(review_edits))
        .route("/api/review/summary", get(review_summary))
        .route("/api/review/sign_off", post(review_sign_off))
        .route("/api/review/changeset", get(review_changeset))
        .route("/api/review/reconcile", post(review_reconcile))
        .route("/api/review/export", get(review_export))
}

fn json_ok(v: serde_json::Value) -> axum::response::Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        v.to_string(),
    )
        .into_response()
}

fn err_resp(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        json!({ "error": msg.into() }).to_string(),
    )
        .into_response()
}

#[derive(Deserialize)]
struct CreateBody {
    slug: String,
    /// Product base branch to materialize from (default main).
    #[serde(default)]
    branch: Option<String>,
    /// Legacy: draft isolation. Prefer `branch_name` for git-shaped branches.
    #[serde(default)]
    draft: Option<bool>,
    /// Git-shaped feature branch name (e.g. `fix-relay-opts`). Implies isolation.
    #[serde(default)]
    branch_name: Option<String>,
}

async fn create_session(Json(body): Json<CreateBody>) -> axum::response::Response {
    if !sessions_enabled() {
        return err_resp(
            StatusCode::SERVICE_UNAVAILABLE,
            "VEIL_SESSIONS disabled (set VEIL_SESSIONS=1 or use VEIL_SOURCE_MODE=s3)",
        );
    }
    let mgr = SessionManager::global();
    let slug = body.slug.trim();
    let draft = body.draft.unwrap_or(false);
    let branch_name = body
        .branch_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    // Mainline sticky: no draft, no branch_name
    let result = if !draft && branch_name.is_none() && body.branch.is_none() {
        mgr.get_or_create_default(slug)
    } else {
        mgr.create_branch(slug, body.branch.as_deref(), draft, branch_name)
    };
    match result {
        Ok(h) => {
            let meta = h.snapshot_meta();
            let reused = !meta.draft_mode && branch_name.is_none();
            json_ok(json!({
                "ok": true,
                "session": session_json(&h),
                "work_dir": h.work_dir.to_string_lossy(),
                "reused": reused,
            }))
        }
        Err(e) => {
            let kind = crate::provider::hub::ProjectsHub::open_error_kind(&e);
            let status = match kind {
                crate::provider::hub::OpenErrorKind::NotFound => StatusCode::NOT_FOUND,
                crate::provider::hub::OpenErrorKind::BadRequest => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            err_resp(status, e)
        }
    }
}

fn session_json(h: &std::sync::Arc<crate::session::SessionHandle>) -> serde_json::Value {
    let meta = h.snapshot_meta();
    let uncommitted = h.has_uncommitted();
    let git_files = h.git_status_files();
    let dirty: Vec<String> = if !git_files.is_empty() {
        git_files.iter().map(|f| f.path.clone()).collect()
    } else {
        meta.dirty.clone()
    };
    json!({
        "session_id": meta.session_id,
        "user_id": meta.user_id,
        "slug": meta.slug,
        "repo_id": meta.repo_id,
        "branch": meta.branch,
        "branch_name": meta.branch_name.clone().unwrap_or_else(|| {
            if meta.draft_mode { "work".into() } else { meta.branch.clone() }
        }),
        "base_branch": meta.base_branch.clone().unwrap_or_else(|| meta.branch.clone()),
        "work_prefix": meta.work_prefix,
        "revision": meta.revision,
        "committed_revision": meta.committed_revision,
        "head_commit": meta.head_commit,
        "uncommitted": uncommitted,
        "draft_mode": meta.draft_mode,
        "active_file": meta.active_file,
        "open_files": meta.open_files,
        "dirty": dirty,
        "git_status": git_files,
        "git_origin": crate::git_origin::origin_enabled(),
        "git_workdir": h.work_dir.join(".git").is_dir(),
        "created_at": meta.created_at,
        "updated_at": meta.updated_at,
        "last_focus": meta.last_focus,
        "intent_log": meta.intent_log,
        "last_activity_at": meta.last_activity_at,
        "active_pr_id": meta.active_pr_id,
    })
}

#[derive(Deserialize)]
struct CommitBody {
    message: String,
}

async fn create_commit(
    Path(id): Path<String>,
    Json(body): Json<CommitBody>,
) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    if let Err(e) = crate::coding_gates::gate_session_commit(&h) {
        return err_resp(StatusCode::BAD_REQUEST, e);
    }
    match h.commit(&body.message) {
        Ok(c) => {
            let slug = h.snapshot_meta().slug;
            let outstanding = crate::session::CURRENT_SESSION.sync_scope(id.clone(), || {
                crate::review::record_commit(&slug, &c.commit_id, &body.message)
            });
            json_ok(json!({
                "ok": true,
                "commit": c,
                "session": session_json(&h),
                "outstanding": outstanding,
            }))
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

async fn session_git_diff(Path(id): Path<String>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    let patch = h.git_working_diff();
    let status = h.git_status_files();
    json_ok(json!({
        "session_id": id,
        "via": "git",
        "patch": patch,
        "status": status,
        "uncommitted": h.has_uncommitted(),
    }))
}

async fn list_commits(Path(id): Path<String>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    if crate::git_origin::origin_enabled() && h.work_dir.join(".git").is_dir() {
        match h.git_log(50) {
            Ok(log) => {
                let commits: Vec<serde_json::Value> = log
                    .into_iter()
                    .map(|e| {
                        json!({
                            "commit_id": e.sha,
                            "message": e.message,
                            "created_at": e.created_at,
                            "parent": e.parent,
                            "author": e.author,
                            "files": e.files,
                            "via": "git",
                        })
                    })
                    .collect();
                return json_ok(json!({
                    "session_id": id,
                    "commits": commits,
                    "via": "git",
                }));
            }
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }
    match list_session_commits(&id) {
        Ok(commits) => json_ok(json!({ "session_id": id, "commits": commits })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct MergeBody {
    /// Escape hatch only — prefer PR Wizard. Requires VEIL_ALLOW_SESSION_MERGE=1 or true.
    #[serde(default)]
    force: bool,
}

async fn merge_session(
    Path(id): Path<String>,
    body: Option<Json<MergeBody>>,
) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    let force = body.map(|b| b.force).unwrap_or(false);
    match h.merge_to_base_gated(force) {
        Ok(v) => json_ok(v),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct PublishBranchBody {
    branch_name: String,
    /// Optional pull request id to bind for agent history writeback.
    #[serde(default)]
    pr_id: Option<String>,
}

async fn publish_branch(
    Path(id): Path<String>,
    Json(body): Json<PublishBranchBody>,
) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h.publish_to_branch(&body.branch_name) {
        Ok(mut v) => {
            if let Some(ref cid) = body.pr_id {
                if let Err(e) = h.set_active_pr_id(Some(cid)) {
                    return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e);
                }
                v["active_pr_id"] = json!(cid);
            }
            v["session"] = session_json(&h);
            json_ok(v)
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct ActivePrBody {
    #[serde(default)]
    pr_id: Option<String>,
}

async fn set_active_pr(
    Path(id): Path<String>,
    Json(body): Json<ActivePrBody>,
) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h.set_active_pr_id(body.pr_id.as_deref()) {
        Ok(()) => json_ok(json!({
            "ok": true,
            "active_pr_id": body.pr_id,
            "session": session_json(&h),
        })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn list_sessions() -> axum::response::Response {
    let user = current_user_id();
    match list_sessions_for_user(&user) {
        Ok(list) => json_ok(json!({ "user_id": user, "sessions": list })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn get_session(Path(id): Path<String>) -> axum::response::Response {
    match SessionManager::global().attach(&id) {
        Ok(h) => json_ok(json!({
            "session": session_json(&h),
            "work_dir": h.work_dir.to_string_lossy(),
        })),
        Err(e) => err_resp(StatusCode::NOT_FOUND, e),
    }
}

async fn attach_session(Path(id): Path<String>) -> axum::response::Response {
    get_session(Path(id)).await
}

async fn pull_session(Path(id): Path<String>) -> axum::response::Response {
    match SessionManager::global().attach(&id) {
        Ok(h) => match h.pull_remote() {
            Ok(()) => json_ok(json!({ "ok": true, "op": "pull_remote", "session_id": id })),
            Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
        },
        Err(e) => err_resp(StatusCode::NOT_FOUND, e),
    }
}

async fn reset_session(Path(id): Path<String>) -> axum::response::Response {
    match SessionManager::global().attach(&id) {
        Ok(h) => match h.reset_to_remote() {
            Ok(()) => json_ok(json!({ "ok": true, "op": "reset_to_remote", "session_id": id })),
            Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
        },
        Err(e) => err_resp(StatusCode::NOT_FOUND, e),
    }
}

async fn flush_session(Path(id): Path<String>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    let dirty = h.snapshot_meta().dirty;
    let mut results = Vec::new();
    for p in &dirty {
        match h.fs.flush_path(p) {
            Ok(r) => {
                h.bump_revision(p, r.etag.clone());
                results.push(json!({ "path": p, "ok": true, "etag": r.etag }));
            }
            Err(e) => results.push(json!({ "path": p, "ok": false, "error": e })),
        }
    }
    // Also flush all serve-set files if dirty empty (best-effort)
    if dirty.is_empty() {
        if let Ok(list) = h.fs.list("", 200) {
            for p in list.into_iter().filter(|p| !p.ends_with('/')).take(50) {
                if let Ok(r) = h.fs.flush_path(&p) {
                    h.bump_revision(&p, r.etag.clone());
                    results.push(json!({ "path": p, "ok": true, "etag": r.etag }));
                }
            }
        }
    }
    json_ok(json!({ "ok": true, "flushed": results, "revision": h.revision() }))
}

async fn close_session(Path(id): Path<String>) -> axum::response::Response {
    SessionManager::global().drop_handle(&id);
    let _ = delete_session_meta(&id);
    json_ok(json!({ "ok": true, "closed": id }))
}

async fn get_turns(Path(id): Path<String>) -> axum::response::Response {
    match list_turns(&id) {
        Ok(t) => json_ok(json!({ "session_id": id, "turns": t })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct TurnBody {
    role: String,
    content: String,
    #[serde(default)]
    tool_calls: Vec<serde_json::Value>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    active_file: Option<String>,
    #[serde(default)]
    backend: Option<String>,
}

async fn post_turn(Path(id): Path<String>, Json(body): Json<TurnBody>) -> axum::response::Response {
    let turn_id = uuid::Uuid::new_v4().to_string();
    let turn = SessionTurn {
        turn_id: turn_id.clone(),
        role: body.role,
        content: body.content,
        tool_calls: body.tool_calls,
        project: body.project,
        active_file: body.active_file,
        ts: crate::session::chrono_now(),
        backend: body.backend,
    };
    match append_turn(&id, &turn) {
        Ok(()) => json_ok(json!({ "ok": true, "turn": turn })),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

// ─── Workspace HTTP tools ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct ListBody {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max: Option<usize>,
}

async fn ws_list(Path(id): Path<String>, Json(body): Json<ListBody>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h.fs.list(body.path.as_deref().unwrap_or(""), body.max.unwrap_or(500)) {
        Ok(files) => json_ok(json!({ "files": files })),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct ReadBody {
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

async fn ws_read(Path(id): Path<String>, Json(body): Json<ReadBody>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h.fs.read(&body.path, body.max_bytes.unwrap_or(200_000)) {
        Ok(content) => json_ok(json!({ "path": body.path, "content": content })),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct WriteBody {
    path: String,
    content: String,
    #[serde(default)]
    if_match: Option<String>,
}

async fn ws_write(Path(id): Path<String>, Json(body): Json<WriteBody>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h.fs.write(&body.path, &body.content, body.if_match.as_deref()) {
        Ok(r) => {
            let rev = h.bump_revision(&body.path, r.etag.clone());
            crate::revision::bus().publish(r.bytes, &body.path, "ws_write");
            h.reload_provider_file(&body.path);
            let slug = h.snapshot_meta().slug;
            if !slug.is_empty() {
                crate::session::CURRENT_SESSION.sync_scope(id.clone(), || {
                    let _ = crate::review::record_file_edit(&slug, &body.path, None);
                });
            }
            json_ok(json!({
                "ok": true,
                "path": r.path,
                "bytes": r.bytes,
                "etag": r.etag,
                "revision": rev,
                "session_id": id,
            }))
        }
        Err(e) if e.contains("etag conflict") => err_resp(StatusCode::PRECONDITION_FAILED, e),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct StrReplaceBody {
    path: String,
    old: String,
    new: String,
    #[serde(default)]
    if_match: Option<String>,
}

async fn ws_str_replace(
    Path(id): Path<String>,
    Json(body): Json<StrReplaceBody>,
) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h
        .fs
        .str_replace(&body.path, &body.old, &body.new, body.if_match.as_deref())
    {
        Ok(r) => {
            let rev = h.bump_revision(&body.path, r.etag.clone());
            crate::revision::bus().publish(r.bytes, &body.path, "ws_str_replace");
            json_ok(json!({
                "ok": true,
                "path": r.path,
                "bytes": r.bytes,
                "etag": r.etag,
                "revision": rev,
                "session_id": id,
            }))
        }
        Err(e) if e.contains("etag conflict") => err_resp(StatusCode::PRECONDITION_FAILED, e),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct GrepBody {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_matches: Option<usize>,
}

async fn ws_grep(Path(id): Path<String>, Json(body): Json<GrepBody>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h
        .fs
        .grep(&body.pattern, body.path.as_deref(), body.max_matches.unwrap_or(50))
    {
        Ok(hits) => json_ok(json!({ "hits": hits })),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct RmBody {
    path: String,
}

async fn ws_rm(Path(id): Path<String>, Json(body): Json<RmBody>) -> axum::response::Response {
    let h = match SessionManager::global().attach(&id) {
        Ok(h) => h,
        Err(e) => return err_resp(StatusCode::NOT_FOUND, e),
    };
    match h.fs.rm(&body.path) {
        Ok(()) => {
            h.bump_revision(&body.path, None);
            json_ok(json!({ "ok": true, "removed": body.path }))
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

/// Project-scoped autosave (durable, light validation).
#[derive(Deserialize)]
pub struct AutosaveBody {
    pub file: String,
    pub content: String,
    #[serde(default)]
    pub if_match: Option<String>,
}

pub async fn post_autosave(
    State(multi): State<Arc<MultiProjectProvider>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AutosaveBody>,
) -> axum::response::Response {
    // Prefer session workspace write when session header present
    if let Some(sid) = headers
        .get("x-veil-session-id")
        .and_then(|v| v.to_str().ok())
    {
        if let Ok(h) = SessionManager::global().attach(sid) {
            return match h.fs.write(&body.file, &body.content, body.if_match.as_deref()) {
                Ok(r) => {
                    let rev = h.bump_revision(&body.file, r.etag.clone());
                    crate::revision::bus().publish(r.bytes, &body.file, "autosave");
                    json_ok(json!({
                        "ok": true,
                        "saved": true,
                        "path": body.file,
                        "etag": r.etag,
                        "revision": rev,
                        "session_id": sid,
                    }))
                }
                Err(e) if e.contains("etag conflict") => {
                    err_resp(StatusCode::PRECONDITION_FAILED, e)
                }
                Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
            };
        }
    }
    // Fallback: write via SourceProvider (active file if name matches)
    let _ = multi;
    err_resp(
        StatusCode::BAD_REQUEST,
        "autosave requires X-Veil-Session-Id and an attached session",
    )
}

// ─── UX Present commit targets (Agent → Present → UX → Server) ───────────────

#[derive(Deserialize)]
struct UxCreateProjectBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    open: Option<bool>,
    #[serde(default)]
    open_ide: Option<bool>,
}

/// Domain create after Present choreography — same as agent create_project domain.
async fn ux_create_project(Json(body): Json<UxCreateProjectBody>) -> axum::response::Response {
    match crate::platform_tools::create_project_domain(&body.name, body.description.as_deref())
        .await
    {
        Ok(mut v) => {
            let ok = v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);
            let slug = v
                .get("slug")
                .and_then(|s| s.as_str())
                .unwrap_or(&body.name)
                .to_string();
            let open_ide = body.open_ide.unwrap_or(true);
            let open = body.open.unwrap_or(true);
            let path = if open_ide {
                format!("/projects/{slug}/ide")
            } else if open {
                format!("/projects/{slug}")
            } else {
                "/projects".into()
            };
            v["path"] = json!(path);
            v["id"] = json!(slug);
            crate::focus::push_intent_log(json!({
                "type": "CreateProject",
                "actor": "ux",
                "summary": slug,
                "domain": "server",
                "ts": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
            }));
            if ok {
                let _ = crate::review::record_project_created(&slug, Some(&body.name), None);
            }
            if ok {
                json_ok(v)
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    [(header::CONTENT_TYPE, "application/json")],
                    v.to_string(),
                )
                    .into_response()
            }
        }
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct UxCreateChangeBody {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    repo_id: Option<String>,
}

async fn ux_create_pr(Json(body): Json<UxCreateChangeBody>) -> axum::response::Response {
    let args = json!({
        "title": body.title,
        "description": body.description,
        "slug": body.slug.or(body.project.clone()),
        "project": body.project,
        "repo_id": body.repo_id,
        "via": "server",
    });
    match crate::platform_tools::dispatch("create_pr", &args).await {
        Ok(s) => {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap_or(json!({ "raw": s }));
            let ok = v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);
            // Normalize id for resultPathTemplate
            let mut out = v.clone();
            if let Some(id) = v
                .pointer("/pull_request/id")
                .or_else(|| v.pointer("/pull_request/pull_request/id"))
                .or_else(|| v.get("id"))
                .cloned()
            {
                out["id"] = id;
            }
            if let Some(path) = v.pointer("/navigation/path").cloned() {
                out["path"] = path;
            }
            if ok {
                json_ok(out)
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    [(header::CONTENT_TYPE, "application/json")],
                    out.to_string(),
                )
                    .into_response()
            }
        }
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct IntentLogBody {
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

async fn ux_intent_log(Json(body): Json<IntentLogBody>) -> axum::response::Response {
    crate::focus::push_intent_log(json!({
        "type": body.r#type.unwrap_or_else(|| "Unknown".into()),
        "actor": body.actor.unwrap_or_else(|| "human".into()),
        "summary": body.summary.unwrap_or_default(),
        "payload": body.payload,
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    }));
    json_ok(json!({ "ok": true }))
}

async fn ux_intent_log_list() -> axum::response::Response {
    json_ok(json!({
        "ok": true,
        "intents": crate::focus::recent_intents(20),
        "acks": crate::focus::recent_acks(12),
    }))
}

#[derive(Deserialize)]
struct IntentAckBody {
    intent_id: String,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Browser finished Present (and optional UX domain commit).
async fn ux_intent_ack(Json(body): Json<IntentAckBody>) -> axum::response::Response {
    let ok = body.ok.unwrap_or(true);
    let result = json!({
        "ok": ok,
        "intent_id": body.intent_id,
        "result": body.result,
        "error": body.error,
    });
    crate::focus::ack_intent(&body.intent_id, result.clone());
    json_ok(json!({ "ok": true, "acked": body.intent_id, "detail": result }))
}

async fn ux_intent_ack_get(Path(id): Path<String>) -> axum::response::Response {
    match crate::focus::get_intent_ack(&id) {
        Some(v) => json_ok(json!({ "ok": true, "ack": v })),
        None => json_ok(json!({ "ok": false, "pending": true, "intent_id": id })),
    }
}

#[derive(Deserialize)]
struct ReviewQuery {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

fn parse_status(raw: Option<&str>) -> Option<crate::review::ItemStatus> {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("outstanding") | Some("open") => Some(crate::review::ItemStatus::Outstanding),
        Some("approved") | Some("approve") => Some(crate::review::ItemStatus::Approved),
        Some("rejected") | Some("reject") => Some(crate::review::ItemStatus::Rejected),
        _ => None,
    }
}

async fn review_outstanding(Query(q): Query<ReviewQuery>) -> axum::response::Response {
    let filter = crate::review::ListFilter {
        slug: q.slug.filter(|s| !s.is_empty()),
        session_id: q.session.filter(|s| !s.is_empty()),
        status: parse_status(q.status.as_deref()).or(Some(crate::review::ItemStatus::Outstanding)),
    };
    json_ok(crate::review::snapshot_json(filter))
}

#[derive(Deserialize)]
struct EditsQuery {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    turn: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// GET /api/review/edits?slug=&turn=&limit= — durable EditRecords (Spec A).
/// Additive endpoint powering the delta-on-map node cards + filmstrip.
async fn review_edits(Query(q): Query<EditsQuery>) -> axum::response::Response {
    let slug = q.slug.filter(|s| !s.is_empty());
    let turn = q.turn.filter(|s| !s.is_empty());
    let edits = crate::review::list_edits(
        slug.as_deref(),
        turn.as_deref(),
        q.limit.unwrap_or(200),
    );
    json_ok(json!({
        "ok": true,
        "count": edits.len(),
        "edits": edits,
    }))
}

async fn review_summary() -> axum::response::Response {    let mut by: Vec<_> = crate::review::summary_by_slug().into_values().collect();
    by.sort_by(|a, b| b.outstanding.cmp(&a.outstanding));
    json_ok(json!({
        "ok": true,
        "outstanding": crate::review::outstanding().len(),
        "projects": by,
    }))
}

#[derive(Deserialize)]
struct SignOffBody {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    git_sha: Option<String>,
    #[serde(default)]
    structural_diff_hash: Option<String>,
    #[serde(default)]
    host_check: Option<serde_json::Value>,
    #[serde(default)]
    pr_id: Option<String>,
    #[serde(default)]
    via: Option<String>,
}

fn apply_sign_off(body: SignOffBody) -> axum::response::Response {
    let via = body.via.clone().unwrap_or_else(|| "ui".into());
    if via.eq_ignore_ascii_case("server") && !crate::review::veil_dev_enabled() {
        return err_resp(
            StatusCode::FORBIDDEN,
            "sign_off via=server is forbidden; a human must use the Approve button on /review",
        );
    }
    match crate::review::sign_off(crate::review::SignOffRequest {
        ids: body.ids,
        slug: body.slug.filter(|s| !s.is_empty()),
        all: body.all,
        decision: body.decision.unwrap_or_else(|| "approve".into()),
        actor: body
            .actor
            .filter(|s| !s.trim().is_empty() && !s.eq_ignore_ascii_case("human"))
            .unwrap_or_else(current_user_id),
        note: body.note,
        git_sha: body.git_sha.filter(|s| !s.is_empty()),
        structural_diff_hash: body.structural_diff_hash.filter(|s| !s.is_empty()),
        host_check: body.host_check,
        pr_id: body.pr_id.filter(|s| !s.is_empty()),
        via: Some(via),
    }) {
        Ok((items, audit)) => json_ok(json!({
            "ok": true,
            "signed": items.len(),
            "items": items,
            "audit": audit,
            "outstanding": crate::review::outstanding().len(),
            "audit_env": crate::review::audit_env_json(),
            "approve_pr": audit.pr_id,
        })),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct ReconcileBody {
    #[serde(default)]
    live_slugs: Vec<String>,
}

async fn review_reconcile(Json(body): Json<ReconcileBody>) -> axum::response::Response {
    let closed = crate::review::close_unknown_projects(&body.live_slugs);
    json_ok(json!({
        "ok": true,
        "closed": closed,
        "outstanding": crate::review::outstanding().len(),
    }))
}

async fn review_changeset(Query(q): Query<ReviewQuery>) -> axum::response::Response {
    let slug = q.slug.filter(|s| !s.is_empty());
    json_ok(json!({
        "ok": true,
        "change_sets": crate::review::change_sets(slug.as_deref()),
        "audit_env": crate::review::audit_env_json(),
    }))
}

async fn review_export() -> axum::response::Response {
    json_ok(crate::review::export_json())
}

async fn review_sign_off(Json(body): Json<SignOffBody>) -> axum::response::Response {
    apply_sign_off(body)
}

async fn ux_sign_off(Json(body): Json<SignOffBody>) -> axum::response::Response {
    apply_sign_off(body)
}
