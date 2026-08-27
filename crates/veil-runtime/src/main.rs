//! veil-runtime — process glue for ProductHost.
//!
//! Product HTTP surface lives in `veil_server::ProductHost`.
//! Platform REST (repos, PRs, deploy) lives in `platform_http`.

mod access;
mod artifact_registry;
mod auth;
mod deploy;
mod function_invoke;
mod local_ports;
mod platform;
mod platform_http;
pub mod tenancy;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Json, Router,
};
use veil_server::{resolve_ui_dir, ProductHost};

/// Build CORS layer from VEIL_CORS_ORIGINS env var.
///
/// If `VEIL_CORS_ORIGINS` is not set or empty, defaults to permissive CORS (same behavior
/// as before for local dev). If set to a comma-separated list of origins, only those
/// origins are allowed.
fn build_cors_layer() -> tower_http::cors::CorsLayer {
    use axum::http::{header, Method};
    use tower_http::cors::CorsLayer;

    let origins_env = std::env::var("VEIL_CORS_ORIGINS").unwrap_or_default();

    if origins_env.is_empty() {
        return CorsLayer::permissive();
    }

    let origins: Vec<axum::http::HeaderValue> = origins_env
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse().ok()
        })
        .collect();

    if origins.is_empty() {
        return CorsLayer::permissive();
    }

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
        ])
        .max_age(std::time::Duration::from_secs(86400))
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

    let extra = Router::new()
        .route("/health", get(health))
        .route("/api/artifacts", get(api_artifacts))
        .route("/api/layers", get(api_layers))
        .route("/api/platform/compile/{repo}", post(api_compile));

    // Empty in-process bus: generated `deploy::application::Deps` still requires
    // a `Bus`. Live deploy HTTP uses `exec`/`store`, not bus invoke.
    let platform_bus: Arc<dyn veil_shared::Bus + Send + Sync> =
        Arc::new(veil_shared::InProcessBus::new());
    let extra = extra.merge(platform_http::build_platform_router(platform_bus).await);

    let ui_dir = resolve_ui_dir(Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))));

    let host = ProductHost::new()
        .port(port)
        .static_dir(ui_dir)
        .mount_bus_router(extra)
        .ensure_config(non_interactive)?;

    let projects_dir = host.projects_dir_path().to_path_buf();
    let viewer = host.viewer_url_ref().to_string();

    let app = host.build_router();

    let auth_config = auth::AuthConfig::from_env();
    let auth_state = auth::AuthState::new(auth_config.clone()).await;
    let app = app.layer(auth::AuthLayer::new(auth_state));

    if auth_config.is_active() {
        tracing::info!("auth enabled (provider: {:?})", auth_config.provider_name());
    } else {
        tracing::info!("auth disabled (local dev mode)");
    }

    let cors = build_cors_layer();
    let app = app.layer(cors);

    ProductHost::serve_app(app, port, &projects_dir, &viewer).await?;

    Ok(())
}
