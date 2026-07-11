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
