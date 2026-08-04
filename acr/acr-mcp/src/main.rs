use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let library_path = std::env::var("ACR_LIBRARY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./acr-data"));

    let port: u16 = std::env::var("ACR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3100);

    tracing::info!("ACR MCP server starting on port {port}");
    tracing::info!("Library path: {}", library_path.display());

    let app = acr_mcp::build_router(library_path).await;
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}
