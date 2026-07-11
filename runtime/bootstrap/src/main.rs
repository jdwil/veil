//! veil-runtime — local product host (PVR pure-runtime path).
//!
//! - Multi-project IDE kernel via `veil-server::build_multi_router`
//! - Live Bus under `/bus/*` via `platform` (projects FS + git + veil check)
//! - Shell: generated SPA preferred, static fallback
//! - Config: `~/.veil/config.json`

mod platform;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Json,
};
use futures::FutureExt;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, services::ServeDir};

#[derive(Debug)]
enum BusError {
    NotFound,
    Other(String),
}

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusError::NotFound => write!(f, "handler not found"),
            BusError::Other(s) => write!(f, "{s}"),
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

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "veil-runtime",
        "ide": "multi",
        "docs": "docs/IDE_RUNTIME.md",
    }))
}

/// RTU-003/004: shell home (static dashboard).
async fn shell_index() -> impl IntoResponse {
    match std::fs::read_to_string(static_path("index.html")) {
        Ok(html) => Html(inject_viewer_url(html)).into_response(),
        Err(_) => Html(
            "<h1>veil-runtime</h1><p>Missing static/index.html — open <a href=\"/api/projects\">/api/projects</a></p>"
                .to_string(),
        )
        .into_response(),
    }
}

/// RTU-004: embed IDE iframe for a project (`/projects/{name}/ide`).
async fn ide_embed(Path(name): Path<String>) -> impl IntoResponse {
    match std::fs::read_to_string(static_path("ide.html")) {
        Ok(html) => Html(inject_viewer_url(html)).into_response(),
        Err(_) => Redirect::temporary(&format!(
            "http://127.0.0.1:5173/?project={}&api={}",
            name,
            urlencoding_origin()
        ))
        .into_response(),
    }
}

fn static_dir() -> PathBuf {
    let candidates = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("static"))),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")),
        Some(PathBuf::from("static")),
        Some(PathBuf::from("runtime/bootstrap/static")),
    ];
    for c in candidates.into_iter().flatten() {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}

fn static_path(file: &str) -> PathBuf {
    static_dir().join(file)
}

fn inject_viewer_url(html: String) -> String {
    let viewer = std::env::var("VEIL_VIEWER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:5173".into());
    html.replacen(
        "<head>",
        &format!(
            "<head>\n  <script>window.VEIL_VIEWER_URL = {};</script>",
            serde_json::to_string(&viewer).unwrap_or_else(|_| "\"http://127.0.0.1:5173\"".into())
        ),
        1,
    )
}

fn urlencoding_origin() -> String {
    std::env::var("VEIL_PUBLIC_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

async fn bus_invoke(
    State(state): State<BusState>,
    Json(req): Json<BusRequest>,
) -> impl IntoResponse {
    match state.bus.invoke(req.message).await {
        Ok(result) => Json(serde_json::json!({ "result": result })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn bus_request(
    State(state): State<BusState>,
    Json(req): Json<BusRequest>,
) -> impl IntoResponse {
    match state.bus.request(req.message).await {
        Ok(result) => Json(serde_json::json!({ "result": result })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn bus_dispatch(
    State(state): State<BusState>,
    Json(req): Json<BusRequest>,
) -> impl IntoResponse {
    match state.bus.dispatch(req.message).await {
        Ok(()) => Json(serde_json::json!({ "status": "accepted" })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn api_artifacts() -> impl IntoResponse {
    Json(platform::list_artifacts(None))
}

async fn api_layers() -> impl IntoResponse {
    Json(platform::list_layers())
}

async fn api_compile(Path(repo): Path<String>) -> impl IntoResponse {
    Json(platform::compile_project(&repo))
}


/// Register Bus handlers: live platform by default; echo if VEIL_RUNTIME_STUB=1 (PVR-017).
fn register_bus_handlers(bus: &mut InProcessBus, stub: bool) {
    let names = [
        "CreateRepo", "ListRepos", "WriteFile", "ReadFile", "ListFiles",
        "CreateBranch", "ListBranches", "GetDiff", "Compile", "Deploy", "GetCommitLog",
        "CreateRepoTool", "WriteFileTool", "ReadFileTool", "ListFilesTool",
        "CreateBranchTool", "ListBranchesTool", "DiffTool", "CompileTool",
        "DeployTool", "ListReposTool", "LogTool",
        "HealthCheck", "LoadConfig", "HandleConnection", "HandleAgentMessage", "HandleToolCall",
        "ParseManifest", "ReadAllManifests", "LoadEnvConfig", "WireApplication",
        "RunSecurityScan", "StartHarness",
    ];
    for name in names {
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
                            obj.entry("type".to_string()).or_insert(serde_json::json!(ty));
                        } else {
                            m = serde_json::json!({ "type": ty, "raw": payload });
                        }
                        Ok(serde_json::to_string(&platform::handle_bus(&m)).unwrap_or_else(|_| "{}".into()))
                    }
                    .boxed()
                }),
            );
        }
    }
}

#[tokio::main]
async fn main() {
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

    // First-run config (non-interactive when CI / no TTY)
    let non_interactive = std::env::var_os("CI").is_some()
        || std::env::var_os("VEIL_NONINTERACTIVE").is_some();
    match veil_server::ensure_config(non_interactive) {
        Ok(cfg) => {
            tracing::info!(
                "config {} projects_dir={}",
                veil_server::config_path().display(),
                cfg.projects_dir_path().display()
            );
        }
        Err(e) => tracing::warn!("config: {e}"),
    }
    let projects_dir = veil_server::ensure_projects_dir_exists()
        .unwrap_or_else(|_| veil_server::default_projects_dir());

    let stub = std::env::var("VEIL_RUNTIME_STUB")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let mut bus = InProcessBus::new();
    register_bus_handlers(&mut bus, stub);
    let bus_state = BusState {
        bus: Arc::new(bus),
    }; // Arc<InProcessBus>

    let show_core = std::env::var("VEIL_SHOW_CORE_LAYERS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let hub = veil_server::ProjectsHub::new(projects_dir.clone(), show_core);
    let ide = veil_server::build_multi_router(hub);

    let bus_routes = Router::new()
        .route("/health", get(health))
        .route("/bus/invoke", post(bus_invoke))
        .route("/bus/request", post(bus_request))
        .route("/bus/dispatch", post(bus_dispatch))
        .route("/api/artifacts", get(api_artifacts))
        .route("/api/layers", get(api_layers))
        .route("/api/platform/compile/{repo}", post(api_compile))
        .with_state(bus_state);

    let shell = Router::new()
        .route("/", get(shell_index))
        .route("/projects/{name}/ide", get(ide_embed))
        .nest_service("/static", ServeDir::new(static_dir()));

    // Merge: shell + IDE multi routes + bus/health (same process, one port)
    let app = shell
        .merge(ide)
        .merge(bus_routes)
        .layer(CorsLayer::permissive());

    let viewer = std::env::var("VEIL_VIEWER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:5173".into());

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("veil-runtime listening on {addr}");
    tracing::info!("  shell:        http://127.0.0.1:{port}/");
    tracing::info!("  projects_dir: {}", projects_dir.display());
    tracing::info!("  hub:          http://127.0.0.1:{port}/api/projects");
    tracing::info!("  ide API:      http://127.0.0.1:{port}/api/p/{{name}}/ir");
    tracing::info!("  ide embed:    http://127.0.0.1:{port}/projects/{{name}}/ide");
    tracing::info!("  viewer:       {viewer}/?project=<name>&api=http://127.0.0.1:{port}");
    tracing::info!(
        "  bus mode:     {}",
        if stub {
            "stub echo"
        } else {
            "hub-backed ListRepos/CreateRepo"
        }
    );

    let listener = TcpListener::bind(&addr).await.expect("failed to bind");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Ctrl+C handler failed");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler failed")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Ctrl+C received"),
        _ = terminate => tracing::info!("SIGTERM received"),
    }
}
