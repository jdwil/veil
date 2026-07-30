//! veil-runtime — thin trampoline (CAP-002 / PVR-010).
//!
//! Product HTTP surface lives in `veil_server::ProductHost`.
//! Bus dispatch lives in `platform` until CAP-003/004 wire generated handlers fully.
//! Target: keep this file ≤ ~80 lines of process glue.

mod local_ports;
mod platform;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use futures::FutureExt;
use veil_server::{resolve_static_dir, ProductHost};

#[derive(Debug)]
enum BusError {
    NotFound,
}

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusError::NotFound => write!(f, "handler not found"),
        }
    }
}

type Handler =
    Box<dyn Fn(String) -> futures::future::BoxFuture<'static, Result<String, BusError>> + Send + Sync>;

struct InProcessBus {
    handlers: HashMap<String, Arc<Handler>>,
}

impl InProcessBus {
    fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    fn register(&mut self, name: &str, handler: Handler) {
        self.handlers.insert(name.to_string(), Arc::new(handler));
    }

    async fn dispatch(&self, evt: serde_json::Value) -> Result<(), BusError> {
        let type_name = evt.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if let Some(handler) = self.handlers.get(type_name) {
            let payload = serde_json::to_string(&evt).unwrap_or_default();
            let h = handler.clone();
            tokio::spawn(async move {
                let _ = h(payload).await;
            });
        }
        Ok(())
    }

    async fn invoke(&self, cmd: serde_json::Value) -> Result<serde_json::Value, BusError> {
        let type_name = cmd.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let handler = self.handlers.get(type_name).ok_or(BusError::NotFound)?;
        let payload = serde_json::to_string(&cmd).unwrap_or_default();
        let result = handler(payload).await?;
        let value: serde_json::Value =
            serde_json::from_str(&result).unwrap_or(serde_json::Value::String(result));
        Ok(value)
    }

    async fn request(&self, qry: serde_json::Value) -> Result<serde_json::Value, BusError> {
        self.invoke(qry).await
    }
}

#[derive(Clone)]
struct BusState {
    bus: Arc<InProcessBus>,
}

#[derive(serde::Deserialize)]
struct BusRequest {
    message: serde_json::Value,
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "veil-runtime",
        "ide": "multi",
        "host": "ProductHost",
        "docs": "docs/IDE_RUNTIME.md",
    }))
}

async fn bus_invoke(
    State(state): State<BusState>,
    Json(req): Json<BusRequest>,
) -> Json<serde_json::Value> {
    match state.bus.invoke(req.message).await {
        Ok(result) => Json(serde_json::json!({ "result": result })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn bus_request(
    State(state): State<BusState>,
    Json(req): Json<BusRequest>,
) -> Json<serde_json::Value> {
    match state.bus.request(req.message).await {
        Ok(result) => Json(serde_json::json!({ "result": result })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn bus_dispatch(
    State(state): State<BusState>,
    Json(req): Json<BusRequest>,
) -> Json<serde_json::Value> {
    match state.bus.dispatch(req.message).await {
        Ok(()) => Json(serde_json::json!({ "status": "accepted" })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn api_artifacts() -> Json<serde_json::Value> {
    Json(platform::list_artifacts(None))
}

async fn api_layers() -> Json<serde_json::Value> {
    Json(platform::list_layers())
}

async fn api_compile(
    axum::extract::Path(repo): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    Json(platform::compile_project(&repo))
}

// ─── Change Management SDLC Endpoints ───────────────────────────────────────

/// POST /api/p/{project}/changes → CreateChangeRequest
async fn cm_create_change_request(
    axum::extract::Path(project): axum::extract::Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let repo_id = body
        .get("repo_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4);
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or(&project)
        .to_string();
    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let jira_ticket = body
        .get("jira_ticket")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match change_management::application::create_change_request(
        &deps,
        repo_id,
        slug,
        title,
        description,
        jira_ticket,
        author,
    )
    .await
    {
        Ok(cr) => Json(serde_json::json!({ "change_request": cr })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// GET /api/p/{project}/changes → ListChangeRequests
async fn cm_list_change_requests(
    axum::extract::Path(_project): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let repo_id = params
        .get("repo_id")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4);

    match change_management::application::list_change_requests(&deps, repo_id, None).await {
        Ok(items) => Json(serde_json::json!({ "change_requests": items })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// GET /api/p/{project}/changes/{id} → GetChangeRequest
async fn cm_get_change_request(
    axum::extract::Path((_project, id)): axum::extract::Path<(String, String)>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    match change_management::application::get_change_request(&deps, pr_id).await {
        Ok(data) => Json(data),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/p/{project}/changes/{id}/commit → CommitToChange
async fn cm_commit_to_change(
    axum::extract::Path((project, id)): axum::extract::Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or(&project)
        .to_string();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let content = body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("update")
        .to_string();
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match change_management::application::commit_to_change(
        &deps, pr_id, slug, path, content, message, author,
    )
    .await
    {
        Ok(data) => Json(data),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/p/{project}/changes/{id}/submit → SubmitForReview
async fn cm_submit_for_review(
    axum::extract::Path((_project, id)): axum::extract::Path<(String, String)>,
    Json(_body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    match change_management::application::submit_for_review(&deps, pr_id).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/p/{project}/changes/{id}/approve → ApproveChange
async fn cm_approve_change(
    axum::extract::Path((_project, id)): axum::extract::Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    let reviewer = body
        .get("reviewer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let comment = body.get("comment").and_then(|v| v.as_str()).map(String::from);

    match change_management::application::approve_change(&deps, pr_id, reviewer, comment).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/p/{project}/changes/{id}/reject → RequestChanges
async fn cm_request_changes(
    axum::extract::Path((_project, id)): axum::extract::Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    let reviewer = body
        .get("reviewer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let comment = body
        .get("comment")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match change_management::application::request_changes(&deps, pr_id, reviewer, comment).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/p/{project}/changes/{id}/merge → MergeChange
async fn cm_merge_change(
    axum::extract::Path((project, id)): axum::extract::Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    let merger = body
        .get("merger")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let slug = body
        .get("slug")
        .and_then(|v| v.as_str())
        .unwrap_or(&project)
        .to_string();

    match change_management::application::merge_change(&deps, pr_id, merger, slug).await {
        Ok(data) => Json(data),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// GET /api/p/{project}/changes/{id}/diff → GetStructuralDiff
async fn cm_get_structural_diff(
    axum::extract::Path((project, id)): axum::extract::Path<(String, String)>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    match change_management::application::get_structural_diff(&deps, pr_id, project).await {
        Ok(data) => Json(data),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// POST /api/p/{project}/changes/{id}/comments → AddComment
async fn cm_add_comment(
    axum::extract::Path((_project, id)): axum::extract::Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let deps = crate::local_ports::change_management_deps().await;
    let pr_id = match uuid::Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({ "error": format!("invalid id: {e}") })),
    };
    let author = body
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let construct_path = body
        .get("construct_path")
        .and_then(|v| v.as_str())
        .map(String::from);
    let comment_body = body
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match change_management::application::add_comment(
        &deps,
        pr_id,
        author,
        construct_path,
        comment_body,
    )
    .await
    {
        Ok(comment) => Json(serde_json::json!({ "comment": comment })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// CAP-003: register via `platform::register_all` (single name registry).
fn register_bus_handlers(bus: &mut InProcessBus, stub: bool) {
    platform::register_all(|name| {
        if stub {
            let handler_name = name.to_string();
            bus.register(
                name,
                Box::new(move |payload: String| {
                    let name = handler_name.clone();
                    async move {
                        Ok(serde_json::json!({
                            "handler": name,
                            "status": "ok",
                            "mode": "stub",
                            "received": payload.len()
                        })
                        .to_string())
                    }
                    .boxed()
                }),
            );
        } else {
            let ty = name.to_string();
            bus.register(
                name,
                Box::new(move |payload: String| {
                    let ty = ty.clone();
                    async move {
                        let mut m: serde_json::Value =
                            serde_json::from_str(&payload).unwrap_or(serde_json::json!({}));
                        if let Some(obj) = m.as_object_mut() {
                            obj.entry("type".to_string())
                                .or_insert(serde_json::json!(ty));
                        } else {
                            m = serde_json::json!({ "type": ty, "raw": payload });
                        }
                        Ok(serde_json::to_string(&platform::handle_bus(&m).await)
                            .unwrap_or_else(|_| "{}".into()))
                    }
                    .boxed()
                }),
            );
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let port: u16 = std::env::var("VEIL_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let non_interactive = std::env::var_os("CI").is_some()
        || std::env::var_os("VEIL_NONINTERACTIVE").is_some();

    let stub = std::env::var("VEIL_RUNTIME_STUB")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let mut bus = InProcessBus::new();
    register_bus_handlers(&mut bus, stub);
    let bus_state = BusState {
        bus: Arc::new(bus),
    };

    let bus_routes = Router::new()
        .route("/health", get(health))
        .route("/bus/invoke", post(bus_invoke))
        .route("/bus/request", post(bus_request))
        .route("/bus/dispatch", post(bus_dispatch))
        .route("/api/artifacts", get(api_artifacts))
        .route("/api/layers", get(api_layers))
        .route("/api/platform/compile/{repo}", post(api_compile))
        // SDLC Change Management endpoints
        .route(
            "/api/p/{project}/changes",
            post(cm_create_change_request).get(cm_list_change_requests),
        )
        .route("/api/p/{project}/changes/{id}", get(cm_get_change_request))
        .route(
            "/api/p/{project}/changes/{id}/commit",
            post(cm_commit_to_change),
        )
        .route(
            "/api/p/{project}/changes/{id}/submit",
            post(cm_submit_for_review),
        )
        .route(
            "/api/p/{project}/changes/{id}/approve",
            post(cm_approve_change),
        )
        .route(
            "/api/p/{project}/changes/{id}/reject",
            post(cm_request_changes),
        )
        .route(
            "/api/p/{project}/changes/{id}/merge",
            post(cm_merge_change),
        )
        .route(
            "/api/p/{project}/changes/{id}/diff",
            get(cm_get_structural_diff),
        )
        .route(
            "/api/p/{project}/changes/{id}/comments",
            post(cm_add_comment),
        )
        .with_state(bus_state);

    let static_dir = resolve_static_dir(Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))));

    // CAP-002: product host owns IDE + SPA + config; trampoline only mounts bus.
    ProductHost::new()
        .port(port)
        .static_dir(static_dir)
        .mount_bus_router(bus_routes)
        .ensure_config(non_interactive)?
        .listen()
        .await?;

    Ok(())
}
