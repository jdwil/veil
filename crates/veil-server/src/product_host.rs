//! CAP-002: Product HTTP host surface — mount IDE multi-router, static SPA,
//! bus routes, and listen. Used by `veil-runtime` trampoline and generated
//! `@main` hosts (`link veil_server`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::get;
use axum::{extract::Path as AxumPath, extract::State, Router};
use tower_http::services::ServeDir;

use crate::api::build_multi_router;
use crate::config::ensure_config;
use crate::provider::hub::ProjectsHub;

/// CAP-002: builder for the ProductHost HTTP surface.
#[derive(Clone)]
pub struct ProductHost {
    projects_dir: PathBuf,
    /// Directory containing the built SPA (index.html + assets/).
    /// Resolved from VEIL_UI_DIR env var, or falls back to `static/dist` for local dev.
    ui_dir: PathBuf,
    show_core_layers: bool,
    port: u16,
    viewer_url: String,
    /// Optional pre-built bus / platform router (merged as-is).
    bus_router: Option<Router>,
}

impl Default for ProductHost {
    fn default() -> Self {
        let ui_dir = std::env::var("VEIL_UI_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("static/dist"));
        Self {
            projects_dir: crate::default_projects_dir(),
            ui_dir,
            show_core_layers: false,
            port: 8080,
            // Same-origin embedded viewer (ProductHost). Override with full URL for Vite dev.
            viewer_url: std::env::var("VEIL_VIEWER_URL").unwrap_or_else(|_| "/viewer".into()),
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
        self.ui_dir = dir.into();
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

    /// Access the projects directory.
    pub fn projects_dir_path(&self) -> &std::path::Path {
        &self.projects_dir
    }

    /// Access the viewer URL.
    pub fn viewer_url_ref(&self) -> &str {
        &self.viewer_url
    }

    /// Access the port.
    pub fn port_val(&self) -> u16 {
        self.port
    }

    /// Ensure config exists (first-run) and resolve projects dir.
    pub fn ensure_config(self, non_interactive: bool) -> Result<Self, String> {
        let cfg = ensure_config(non_interactive)?;
        let dir = crate::ensure_projects_dir_exists().unwrap_or_else(|_| cfg.projects_dir_path());
        Ok(self.projects_dir(dir).show_core_layers(cfg.show_core_layers))
    }

    /// Build the full product router: shell + multi IDE + config + optional bus.
    ///
    /// Note: CORS and auth layers are NOT applied here — the caller (main.rs)
    /// is responsible for layering those on top before serving.
    pub fn build_router(self) -> Router {
        // Warm platform language packs + stubs (monorepo or S3+DDB META → $TMP).
        crate::layer_ops::ensure_platform_layer_cache();
        crate::stub_ops::ensure_platform_stub_cache();

        let hub = ProjectsHub::new(self.projects_dir.clone(), self.show_core_layers);
        let ide = build_multi_router(hub);

        let shell_state = ShellState {
            ui_dir: self.ui_dir.clone(),
            viewer_url: self.viewer_url.clone(),
        };

        // SPA shell routes (generated nav). Full page loads must return index.html.
        // IDE UI (`/viewer`) is nested once by `build_multi_router` — do not double-nest.
        // `/projects/{name}/ide` is the **in-shell IDE embed** (SPA + iframe to /viewer);
        // do not redirect away from the shell (keeps AgentDock + chat history).
        let shell = Router::new()
            .route("/", get(shell_index))
            .route("/projects", get(shell_index))
            .route("/projects/new", get(shell_index))
            .route("/projects/{name}/ide", get(shell_index))
            .route("/projects/{name}", get(shell_index))
            .route("/pulls", get(shell_index))
            .route("/pulls/{*rest}", get(shell_index))
            .route("/pullcreate", get(shell_index))
            .route("/pulldetail", get(shell_index))
            .route("/deploy", get(shell_index))
            .route("/registry", get(shell_index))
            .route("/bus", get(shell_index))
            .route("/agents", get(shell_index))
            .route("/config", get(shell_index))
            .route("/dashboard", get(shell_index))
            .route("/review", get(shell_index))
            .route("/review/{*rest}", get(shell_index))
            // Optional deep link: bare dual-loop viewer (standalone agent chrome).
            .route("/ide/{name}", get(ide_embed))
            // SPA assets served from the UI dir
            .nest_service("/static", ServeDir::new(&self.ui_dir))
            .nest_service("/assets", ServeDir::new(self.ui_dir.join("assets")))
            .with_state(shell_state.clone());

        // IDE multi-router already includes GET+PATCH /api/config (CAP-007).
        let mut app = shell.merge(ide);
        if let Some(bus) = self.bus_router {
            app = app.merge(bus);
        }
        // Unmatched GET paths that look like shell pages → SPA (not /api|/bus|/health).
        let spa_fb = Router::new()
            .fallback(get(spa_fallback))
            .with_state(shell_state);
        app.merge(spa_fb)
    }

    /// CAP-002: listen and serve until shutdown signal (simple path — no custom layers).
    pub async fn listen(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let port = self.port;
        let projects_dir = self.projects_dir.clone();
        let viewer = self.viewer_url.clone();
        let app = self.build_router();
        Self::serve(app, port, &projects_dir, &viewer).await
    }

    /// Serve a pre-built (and optionally layered) router. Use this when you need
    /// to apply auth/CORS/etc. on the router before listen.
    pub async fn serve_app(
        app: Router,
        port: u16,
        projects_dir: &std::path::Path,
        viewer: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Self::serve(app, port, projects_dir, viewer).await
    }

    async fn serve(
        app: Router,
        port: u16,
        projects_dir: &std::path::Path,
        viewer: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!("veil product host listening on {addr}");
        tracing::info!("  shell:        http://127.0.0.1:{port}/");
        tracing::info!("  projects_dir: {}", projects_dir.display());
        tracing::info!("  hub:          http://127.0.0.1:{port}/api/projects");
        tracing::info!("  ide API:      http://127.0.0.1:{port}/api/p/{{name}}/ir");
        tracing::info!("  viewer:       {viewer}/?project=<name>&api=http://127.0.0.1:{port}");

        if crate::git_origin::origin_enabled() {
            tokio::spawn(async {
                let n = tokio::task::spawn_blocking(|| {
                    crate::provider::s3_workspace::backfill_git_origins()
                })
                .await;
                match n {
                    Ok(rows) => {
                        let ok = rows.iter().filter(|r| r.2.is_ok()).count();
                        tracing::info!(
                            catalog = rows.len(),
                            with_origin = ok,
                            "git origin catalog backfill finished"
                        );
                    }
                    Err(e) => tracing::warn!(error = %e, "git origin backfill task failed"),
                }
            });
        }

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
struct ShellState {
    ui_dir: PathBuf,
    viewer_url: String,
}

async fn shell_index(State(st): State<ShellState>) -> impl IntoResponse {
    serve_spa_html(&st)
}

/// Fallback: serve SPA for unknown non-API paths so deep links work.
async fn spa_fallback(
    State(st): State<ShellState>,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    let path = uri.path();
    // Leave SPA paths (including GET /bus page) free; only block API-like prefixes.
    if path.starts_with("/api")
        || path.starts_with("/bus/")
        || path.starts_with("/health")
        || path.starts_with("/static")
        || path.starts_with("/assets")
        || path.starts_with("/viewer")
    {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    // Avoid stealing method-not-allowed noise for POST etc. — only GET reaches fallback typically.
    serve_spa_html(&st)
}

fn serve_spa_html(st: &ShellState) -> axum::response::Response {
    // PVR-032 / CAP-005: generated SPA only — never handwritten static root shells.
    let path = st.ui_dir.join("index.html");
    if path.is_file() {
        if let Ok(html) = std::fs::read_to_string(&path) {
            return Html(inject_viewer_url(html, &st.viewer_url)).into_response();
        }
    }
    let dir_display = st.ui_dir.display();
    Html(format!(
        "<h1>veil-runtime</h1><p>UI not found at <code>{dir_display}/index.html</code>. \
         Set <code>VEIL_UI_DIR</code> or build the UI (<code>cd ui &amp;&amp; npm run build</code>). \
         API: <a href=\"/api/projects\">/api/projects</a></p>"
    ))
    .into_response()
}

/// Standalone dual-loop IDE (`/ide/{name}` → `/viewer/?project=…`).
///
/// The **product path** is shell embed: SPA route `/projects/{name}/ide` (iframe +
/// AgentDock). Use this only for standalone / external deep links with IDE agent chrome.
///
/// Same-origin ProductHost: `/viewer/?project=…` (viewer uses `location.origin` for API).
/// Absolute `VEIL_VIEWER_URL` (e.g. Vite :5173): redirect there with `?api=` public URL.
async fn ide_embed(
    State(st): State<ShellState>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    let viewer = st.viewer_url.trim().trim_end_matches('/');
    let name = urlencoding_path(&name);
    let target = if viewer.starts_with("http://") || viewer.starts_with("https://") {
        let api = std::env::var("VEIL_PUBLIC_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".into());
        format!("{viewer}/?project={name}&api={api}")
    } else {
        let base = if viewer.is_empty() { "/viewer" } else { viewer };
        // Same origin: no api= needed if viewer treats empty api as location.origin.
        format!("{base}/?project={name}")
    };
    Redirect::temporary(&target).into_response()
}

fn urlencoding_path(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

fn inject_viewer_url(html: String, viewer: &str) -> String {
    let v = if viewer.is_empty() { "/viewer" } else { viewer };
    html.replacen(
        "<head>",
        &format!(
            "<head>\n  <script>window.VEIL_VIEWER_URL = {};</script>",
            serde_json::to_string(v).unwrap_or_else(|_| "\"/viewer\"".into())
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

/// Resolve UI directory for the product host.
///
/// Priority: VEIL_UI_DIR env var → exe-relative static/dist → CARGO_MANIFEST relative → cwd.
pub fn resolve_ui_dir(manifest_dir: Option<&Path>) -> PathBuf {
    // Explicit env override takes priority (Docker / deployed).
    if let Ok(dir) = std::env::var("VEIL_UI_DIR") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            return p;
        }
        tracing::warn!("VEIL_UI_DIR={dir} is not a directory; falling back to probing");
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("static/dist"));
        }
    }
    if let Some(m) = manifest_dir {
        candidates.push(m.join("static/dist"));
    }
    candidates.push(PathBuf::from("static/dist"));
    candidates.push(PathBuf::from("crates/veil-runtime/static/dist"));
    // Also probe the old `static` dir (backwards compat).
    candidates.push(PathBuf::from("static"));
    for c in candidates {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from("static/dist")
}
