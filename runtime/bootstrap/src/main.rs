//! veil-runtime bootstrap — Stage 0 composition root.
//!
//! This is the ONLY handwritten Rust in the entire veil-runtime. It solves
//! the bootstrapping problem: veil-runtime provides the harness for all VEIL
//! apps, but since it IS written in VEIL, it needs a minimal seed binary.
//!
//! Responsibilities:
//! 1. Construct the InProcessBus
//! 2. Register all context handlers (from generated manifest.json files)
//! 3. Start the HTTP/WS server
//! 4. Graceful shutdown
//!
//! Everything else — domain logic, tools, storage, agents — is in the
//! generated VEIL crates. This file just wires them together.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
    extract::State,
    response::IntoResponse,
    Json,
};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use veil_shared::{Bus, DomainError};

// ─── Bus Implementation ────────────────────────────────────────────────────

type Handler = Box<dyn Fn(String) -> futures::future::BoxFuture<'static, Result<String, DomainError>> + Send + Sync>;

struct InProcessBus {
    handlers: HashMap<String, Arc<Handler>>,
}

impl InProcessBus {
    fn new() -> Self {
        Self { handlers: HashMap::new() }
    }

    fn register(&mut self, name: &str, handler: Handler) {
        self.handlers.insert(name.to_string(), Arc::new(handler));
    }
}

#[async_trait::async_trait]
impl Bus for InProcessBus {
    async fn dispatch(&self, evt: serde_json::Value) -> Result<(), DomainError> {
        let type_name = evt.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if let Some(handler) = self.handlers.get(type_name) {
            let payload = serde_json::to_string(&evt).unwrap_or_default();
            let h = handler.clone();
            tokio::spawn(async move { let _ = h(payload).await; });
        }
        Ok(())
    }

    async fn invoke(&self, cmd: serde_json::Value) -> Result<serde_json::Value, DomainError> {
        let type_name = cmd.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let handler = self.handlers.get(type_name).ok_or_else(|| {
            DomainError::NotFound
        })?;
        let payload = serde_json::to_string(&cmd).unwrap_or_default();
        let result = handler(payload).await?;
        let value: serde_json::Value = serde_json::from_str(&result)
            .unwrap_or(serde_json::Value::String(result));
        Ok(value)
    }

    async fn request(&self, qry: serde_json::Value) -> Result<serde_json::Value, DomainError> {
        self.invoke(qry).await
    }
}

use futures::FutureExt;

// ─── HTTP Routes ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    bus: Arc<dyn Bus>,
}

#[derive(serde::Deserialize)]
struct BusRequest {
    message: serde_json::Value,
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "healthy", "service": "veil-runtime" }))
}

async fn bus_invoke(State(state): State<AppState>, Json(req): Json<BusRequest>) -> impl IntoResponse {
    match state.bus.invoke(req.message).await {
        Ok(result) => Json(serde_json::json!({ "result": result })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn bus_request(State(state): State<AppState>, Json(req): Json<BusRequest>) -> impl IntoResponse {
    match state.bus.request(req.message).await {
        Ok(result) => Json(serde_json::json!({ "result": result })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

async fn bus_dispatch(State(state): State<AppState>, Json(req): Json<BusRequest>) -> impl IntoResponse {
    match state.bus.dispatch(req.message).await {
        Ok(()) => Json(serde_json::json!({ "status": "accepted" })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

// ─── Entry Point ───────────────────────────────────────────────────────────

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

    // Construct the Bus with all handlers registered.
    // For now, handlers are placeholders — the generated crate functions
    // will be wired here once the adapter implementations are complete.
    let mut bus = InProcessBus::new();

    // Register handlers from each context's manifest.
    // In production, these would call the actual generated functions.
    // For now, echo handlers demonstrate the routing works.
    let handler_names = [
        // Storage context
        "CreateRepo", "ListRepos", "WriteFile", "ReadFile", "ListFiles",
        "CreateBranch", "ListBranches", "GetDiff", "Compile", "Deploy", "GetCommitLog",
        // Tools context
        "CreateRepoTool", "WriteFileTool", "ReadFileTool", "ListFilesTool",
        "CreateBranchTool", "ListBranchesTool", "DiffTool", "CompileTool",
        "DeployTool", "ListReposTool", "LogTool",
        // Daemon context
        "HealthCheck", "LoadConfig", "HandleConnection", "HandleAgentMessage", "HandleToolCall",
        // Exec context
        "ParseManifest", "ReadAllManifests", "LoadEnvConfig", "WireApplication",
        "RunSecurityScan", "StartHarness",
    ];

    for name in handler_names {
        let handler_name = name.to_string();
        bus.register(name, Box::new(move |payload: String| {
            let name = handler_name.clone();
            async move {
                Ok(serde_json::json!({
                    "handler": name,
                    "status": "ok",
                    "received": payload.len()
                }).to_string())
            }.boxed()
        }));
    }

    let state = AppState {
        bus: Arc::new(bus),
    };

    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/bus/invoke", post(bus_invoke))
        .route("/bus/request", post(bus_request))
        .route("/bus/dispatch", post(bus_dispatch))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("veil-runtime listening on {}", addr);

    let listener = TcpListener::bind(&addr).await.expect("failed to bind");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Ctrl+C handler failed");
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
