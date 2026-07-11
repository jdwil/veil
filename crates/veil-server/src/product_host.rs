//! CAP-002: Product HTTP host surface — mount IDE multi-router, static SPA,
//! bus routes, and listen. Used by `veil-runtime` trampoline and generated
//! `@main` hosts (`link veil_server`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::get;
use axum::{extract::Path as AxumPath, extract::State, Router};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::api::build_multi_router;
use crate::config::ensure_config;
use crate::provider::hub::ProjectsHub;

/// CAP-002: builder for the pure-runtime product HTTP surface.
#[derive(Clone)]
pub struct ProductHost {
    projects_dir: PathBuf,
    static_dir: PathBuf,
    show_core_layers: bool,
    port: u16,
    viewer_url: String,
    /// Optional pre-built bus / platform router (merged as-is).
    bus_router: Option<Router>,
}

impl Default for ProductHost {
    fn default() -> Self {
        Self {
            projects_dir: crate::default_projects_dir(),
            static_dir: PathBuf::from("static"),
            show_core_layers: false,
            port: 8080,
            viewer_url: std::env::var("VEIL_VIEWER_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:5173".into()),
            bus_router: None,
        }
    }
}

impl ProductHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn projects_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.projects_dir = dir.into();
        self
    }

    pub fn static_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.static_dir = dir.into();
        self
    }

    pub fn show_core_layers(mut self, show: bool) -> Self {
        self.show_core_layers = show;
        self
    }

    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn viewer_url(mut self, url: impl Into<String>) -> Self {
        self.viewer_url = url.into();
        self
    }

    /// Mount custom bus / platform JSON routes (merged at top level).
    pub fn mount_bus_router(mut self, router: Router) -> Self {
        self.bus_router = Some(router);
        self
    }

    /// Ensure config exists (first-run) and resolve projects dir.
    pub fn ensure_config(self, non_interactive: bool) -> Result<Self, String> {
        let cfg = ensure_config(non_interactive)?;
        let dir = crate::ensure_projects_dir_exists().unwrap_or_else(|_| cfg.projects_dir_path());
        Ok(self.projects_dir(dir).show_core_layers(cfg.show_core_layers))
    }

    /// Build the full product router: shell + multi IDE + config + optional bus.
    pub fn build_router(self) -> Router {
        let hub = ProjectsHub::new(self.projects_dir.clone(), self.show_core_layers);
        let ide = build_multi_router(hub);

        let shell_state = ShellState {
            static_dir: self.static_dir.clone(),
            viewer_url: self.viewer_url.clone(),
        };

        let shell = Router::new()
            .route("/", get(shell_index))
            .route("/projects/{name}/ide", get(ide_embed))
            .route("/config", get(shell_index))
            .route("/projects", get(shell_index))
            // SPA assets under /static/dist/ (index references absolute paths)
            .nest_service("/static", ServeDir::new(&self.static_dir))
            .nest_service("/assets", ServeDir::new(self.static_dir.join("assets")))
            .with_state(shell_state);

        // IDE multi-router already includes GET+PATCH /api/config (CAP-007).
        let mut app = shell.merge(ide);
        if let Some(bus) = self.bus_router {
            app = app.merge(bus);
        }
        app.layer(CorsLayer::permissive())
    }

    /// CAP-002: listen and serve until shutdown signal.
    pub async fn listen(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let port = self.port;
        let projects_dir = self.projects_dir.clone();
        let viewer = self.viewer_url.clone();
        let app = self.build_router();

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!("veil product host listening on {addr}");
        tracing::info!("  shell:        http://127.0.0.1:{port}/");
        tracing::info!("  projects_dir: {}", projects_dir.display());
        tracing::info!("  hub:          http://127.0.0.1:{port}/api/projects");
        tracing::info!("  ide API:      http://127.0.0.1:{port}/api/p/{{name}}/ir");
        tracing::info!("  viewer:       {viewer}/?project=<name>&api=http://127.0.0.1:{port}");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
struct ShellState {
    static_dir: PathBuf,
    viewer_url: String,
}

async fn shell_index(State(st): State<ShellState>) -> impl IntoResponse {
    // Prefer SPA dist (CAP-005), then static/app, then legacy index.html
    let candidates = [
        st.static_dir.join("dist/index.html"),
        st.static_dir.join("app/index.html"),
        st.static_dir.join("index.html"),
    ];
    for path in &candidates {
        if path.is_file() {
            if let Ok(html) = std::fs::read_to_string(path) {
                return Html(inject_viewer_url(html, &st.viewer_url)).into_response();
            }
        }
    }
    Html(
        "<h1>veil-runtime</h1><p>Missing shell — open <a href=\"/api/projects\">/api/projects</a></p>"
            .to_string(),
    )
    .into_response()
}

async fn ide_embed(
    State(st): State<ShellState>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    let path = st.static_dir.join("ide.html");
    match std::fs::read_to_string(&path) {
        Ok(html) => Html(inject_viewer_url(html, &st.viewer_url)).into_response(),
        Err(_) => Redirect::temporary(&format!(
            "{}/?project={}&api=http://127.0.0.1:8080",
            st.viewer_url, name
        ))
        .into_response(),
    }
}

fn inject_viewer_url(html: String, viewer: &str) -> String {
    html.replacen(
        "<head>",
        &format!(
            "<head>\n  <script>window.VEIL_VIEWER_URL = {};</script>",
            serde_json::to_string(viewer).unwrap_or_else(|_| "\"http://127.0.0.1:5173\"".into())
        ),
        1,
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Resolve static directory for the product host (exe-relative, CARGO_MANIFEST, cwd).
pub fn resolve_static_dir(manifest_dir: Option<&Path>) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("static"));
        }
    }
    if let Some(m) = manifest_dir {
        candidates.push(m.join("static"));
    }
    candidates.push(PathBuf::from("static"));
    candidates.push(PathBuf::from("runtime/bootstrap/static"));
    for c in candidates {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from("static")
}
