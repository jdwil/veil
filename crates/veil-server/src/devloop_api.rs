//! Dev-loop API routes — IDE controls for starting/stopping dev targets.
//!
//! Routes (nested under `/api/p/{project}/dev/`):
//! - GET  /targets  — list configured targets + status
//! - POST /start    — start a target (or all)
//! - POST /stop     — stop a target (or all)
//! - GET  /logs     — recent log lines for a target

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::devloop::{self, SharedDevLoops};
use crate::provider::hub::CURRENT_PROJECT;
use crate::provider::SourceProvider;

/// Shared dev-loop state injected as axum extension.
pub type DevLoopState = SharedDevLoops;

/// Multi-project: task-local name. Single-project: display name of serve root
/// (`CURRENT_PROJECT` is only set under `/api/p/{project}/…`).
fn resolve_project_key<P: SourceProvider>(state: &P) -> Result<String, &'static str> {
    if let Ok(n) = CURRENT_PROJECT.try_with(|n| n.clone()) {
        if !n.is_empty() {
            return Ok(n);
        }
    }
    state
        .project_root()
        .map(|p| crate::project_layout::project_display_name(&p))
        .ok_or("no project root")
}

/// GET /dev/targets — list targets + status
pub async fn get_dev_targets<P: SourceProvider>(
    State(state): State<Arc<P>>,
    axum::Extension(loops): axum::Extension<DevLoopState>,
) -> impl IntoResponse {
    let project = match resolve_project_key(state.as_ref()) {
        Ok(n) => n,
        Err(e) => return Json(json!({"error": e})),
    };

    let project_root = match state.project_root() {
        Some(r) => r,
        None => return Json(json!({"error": "no project root"})),
    };

    // Ensure dev loop exists (parse config on first access)
    if let Err(e) = devloop::get_or_create_dev_loop(&loops, &project, &project_root) {
        return Json(json!({"error": e, "hint": "Add [[targets]] to veil.toml"}));
    }

    // Poll process health + re-probe attached targets before reporting status.
    let needs_watcher = {
        let mut map = loops.lock().unwrap();
        if let Some(dev) = map.get_mut(&project) {
            dev.poll_health();
            // If we just discovered running targets, ensure the file watcher is live.
            dev.any_running() && dev.stop_tx_active() == false
        } else {
            false
        }
    };
    if needs_watcher {
        let _ = devloop::start_file_watcher(
            loops.clone(),
            project.clone(),
            project_root.clone(),
        );
    }

    let map = loops.lock().unwrap();
    if let Some(dev) = map.get(&project) {
        let targets: Vec<_> = dev.status().iter().map(|s| {
            json!({
                "name": s.name,
                "status": s.status,
                "package": s.config.package,
                "target": s.config.target,
                "output": s.config.output,
                "dev_command": s.config.dev_command,
                "dev_port": s.config.dev_port,
                "last_gen": s.last_gen,
                "last_error": s.last_error,
                "attached": s.attached,
            })
        }).collect();
        Json(json!({"targets": targets}))
    } else {
        Json(json!({"targets": [], "error": "not initialized"}))
    }
}

#[derive(Deserialize, Default)]
pub struct TargetParam {
    pub name: Option<String>,
}

/// POST /dev/start — start a target (or all if no name given)
pub async fn post_dev_start<P: SourceProvider>(
    State(state): State<Arc<P>>,
    axum::Extension(loops): axum::Extension<DevLoopState>,
    Json(params): Json<TargetParam>,
) -> impl IntoResponse {
    let project = match resolve_project_key(state.as_ref()) {
        Ok(n) => n,
        Err(e) => return Json(json!({"error": e})),
    };

    let project_root = match state.project_root() {
        Some(r) => r,
        None => return Json(json!({"error": "no project root"})),
    };

    if let Err(e) = devloop::get_or_create_dev_loop(&loops, &project, &project_root) {
        return Json(json!({"error": e}));
    }

    // Start file watcher if not already running
    {
        let has_watcher = loops.lock().unwrap()
            .get(&project)
            .map(|d| d.status().iter().any(|s| s.status != devloop::TargetStatus::Stopped))
            .unwrap_or(false);
        if !has_watcher {
            let _ = devloop::start_file_watcher(
                loops.clone(),
                project.clone(),
                project_root.clone(),
            );
        }
    }

    let mut map = loops.lock().unwrap();
    let dev = match map.get_mut(&project) {
        Some(d) => d,
        None => return Json(json!({"error": "dev loop not found"})),
    };

    if let Some(name) = &params.name {
        match dev.start(name) {
            Ok(()) => Json(json!({"ok": true, "started": name})),
            Err(e) => Json(json!({"ok": false, "error": e})),
        }
    } else {
        let results = dev.start_all();
        let summary: Vec<_> = results
            .iter()
            .map(|(n, r)| json!({"name": n, "ok": r.is_ok(), "error": r.as_ref().err()}))
            .collect();
        Json(json!({"ok": true, "results": summary}))
    }
}

/// POST /dev/stop — stop a target (or all if no name given)
pub async fn post_dev_stop<P: SourceProvider>(
    State(state): State<Arc<P>>,
    axum::Extension(loops): axum::Extension<DevLoopState>,
    Json(params): Json<TargetParam>,
) -> impl IntoResponse {
    let project = match resolve_project_key(state.as_ref()) {
        Ok(n) => n,
        Err(e) => return Json(json!({"error": e})),
    };

    let _project_root = match state.project_root() {
        Some(r) => r,
        None => return Json(json!({"error": "no project root"})),
    };

    let mut map = loops.lock().unwrap();
    let dev = match map.get_mut(&project) {
        Some(d) => d,
        None => return Json(json!({"ok": true, "note": "nothing running"})),
    };

    if let Some(name) = &params.name {
        dev.stop_dev_server(name);
        Json(json!({"ok": true, "stopped": name}))
    } else {
        dev.stop_all();
        Json(json!({"ok": true, "stopped": "all"}))
    }
}

/// GET /dev/logs?name=frontend — recent log lines
pub async fn get_dev_logs<P: SourceProvider>(
    State(state): State<Arc<P>>,
    axum::Extension(loops): axum::Extension<DevLoopState>,
    Query(params): Query<TargetParam>,
) -> impl IntoResponse {
    let project = match resolve_project_key(state.as_ref()) {
        Ok(n) => n,
        Err(e) => return Json(json!({"error": e})),
    };

    let map = loops.lock().unwrap();
    let dev = match map.get(&project) {
        Some(d) => d,
        None => return Json(json!({"logs": []})),
    };

    if let Some(name) = &params.name {
        if let Some(state) = dev.target_status(name) {
            Json(json!({"name": name, "logs": state.logs}))
        } else {
            Json(json!({"error": format!("unknown target: {name}")}))
        }
    } else {
        // All targets' logs
        let all: Vec<_> = dev.status().iter().map(|s| {
            json!({"name": s.name, "logs": s.logs})
        }).collect();
        Json(json!({"targets": all}))
    }
}
