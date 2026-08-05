//! HTTP API for durable coding sessions and workspace tools.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::session::{
    append_turn, current_user_id, delete_session_meta, list_sessions_for_user, list_turns,
    sessions_enabled, SessionManager, SessionTurn, WorkspaceFs,
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
        // Workspace tools (also exposed via MCP)
        .route("/api/sessions/{id}/ws/list", post(ws_list))
        .route("/api/sessions/{id}/ws/read", post(ws_read))
        .route("/api/sessions/{id}/ws/write", post(ws_write))
        .route("/api/sessions/{id}/ws/str_replace", post(ws_str_replace))
        .route("/api/sessions/{id}/ws/grep", post(ws_grep))
        .route("/api/sessions/{id}/ws/rm", post(ws_rm))
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
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    draft: Option<bool>,
}

async fn create_session(Json(body): Json<CreateBody>) -> axum::response::Response {
    if !sessions_enabled() {
        return err_resp(
            StatusCode::SERVICE_UNAVAILABLE,
            "VEIL_SESSIONS disabled (set VEIL_SESSIONS=1 or use VEIL_SOURCE_MODE=s3)",
        );
    }
    let mgr = SessionManager::global();
    match mgr.create_with_opts(
        body.slug.trim(),
        body.branch.as_deref(),
        body.draft.unwrap_or(false),
    ) {
        Ok(h) => {
            let meta = h.snapshot_meta();
            json_ok(json!({
                "ok": true,
                "session": meta,
                "work_dir": h.work_dir.to_string_lossy(),
            }))
        }
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
        Ok(h) => {
            let meta = h.snapshot_meta();
            json_ok(json!({
                "session": meta,
                "work_dir": h.work_dir.to_string_lossy(),
            }))
        }
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
