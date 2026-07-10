//! HTTP API route handlers for the VEIL dev server.
//!
//! All handlers are parameterized by the [`SourceProvider`] trait — they don't
//! know whether source lives on disk or in a remote VCS.

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json,
};
use tower_http::cors::CorsLayer;

use crate::protocol::{EditRequest, EditResponse};
use crate::provider::SourceProvider;

/// Build the complete axum Router for the VEIL dev server API.
///
/// The returned router handles:
/// - `GET /api/ir` — current IR graph as JSON
/// - `GET /api/source` — raw .veil source text
/// - `GET /api/generated` — generated code map
/// - `GET /api/palette` — construct palette from loaded layers
/// - `GET /api/stubs` — loaded external crate APIs
/// - `GET /api/diagnostics` — layer-driven diagnostics
/// - `GET /api/files` — list loaded files
/// - `POST /api/files/select` — switch active file
/// - `POST /api/edit` — apply structured edits
pub fn build_router<P: SourceProvider>(provider: P) -> Router {
    let state = Arc::new(provider);

    Router::new()
        .route("/api/ir", get(get_ir::<P>))
        .route("/api/source", get(get_source::<P>))
        .route("/api/generated", get(get_generated::<P>))
        .route("/api/palette", get(get_palette::<P>))
        .route("/api/stubs", get(get_stubs::<P>))
        .route("/api/diagnostics", get(get_diagnostics::<P>))
        .route("/api/files", get(get_files::<P>))
        .route("/api/files/select", post(post_select_file::<P>))
        .route("/api/edit", post(post_edit::<P>))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

type SharedProvider<P> = Arc<P>;

// ─── Helpers ───────────────────────────────────────────────────────────────

fn parse_source(source: &str, registry: &veil_ir::LayerRegistry) -> Result<veil_ir::Solution, String> {
    let tokens = veil_parser::lex(source);
    veil_parser::parse_with_registry(&tokens, registry.clone())
        .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
}

fn json_response(body: String) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], body)
}

// ─── GET Handlers ──────────────────────────────────────────────────────────

async fn get_ir<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let sol = match parse_source(&source, state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let graph = veil_ir::build_ir(&sol);
    match serde_json::to_string(&graph) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_source<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    match state.read_source("").await {
        Ok(source) => ([(header::CONTENT_TYPE, "text/plain")], source).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn get_generated<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let sol = match parse_source(&source, state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let project = veil_codegen::generate(&sol, state.registry());
    let files_map: std::collections::HashMap<String, String> = project.files.iter()
        .map(|f| (f.path.clone(), f.content.clone()))
        .collect();
    match serde_json::to_string(&files_map) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_palette<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let palette = veil_ir::palette_from_registry(state.registry());
    match serde_json::to_string(&palette) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_stubs<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    match serde_json::to_string(&state.registry().stubs) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_diagnostics<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let sol = match parse_source(&source, state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let graph = veil_ir::build_ir(&sol);
    let diagnostics = veil_ir::diagnostics::analyze(&graph, state.registry());
    match serde_json::to_string(&diagnostics) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_files<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let files = state.list_files().await;
    match serde_json::to_string(&files) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── POST Handlers ─────────────────────────────────────────────────────────

async fn post_select_file<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<crate::protocol::SelectFileRequest>,
) -> axum::response::Response {
    // For filesystem provider, we downcast. For others, this is a no-op or custom logic.
    // The generic approach: just read the file at the given index.
    let files = state.list_files().await;
    if req.index >= files.len() {
        return (StatusCode::BAD_REQUEST, "invalid file index").into_response();
    }
    let file = &files[req.index];
    let source = match state.read_source(&file.name).await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let sol = match parse_source(&source, state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let graph = veil_ir::build_ir(&sol);
    match serde_json::to_string(&graph) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_edit<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<EditRequest>,
) -> axum::response::Response {
    // 1. Read current source
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    if !state.is_editable("") {
        return (StatusCode::BAD_REQUEST, "file is read-only").into_response();
    }

    // 2. Parse into AST
    let mut sol = match parse_source(&source, state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("parse failed: {}", e)).into_response(),
    };

    // 3. Apply edits
    if let Err(e) = veil_ir::apply_edits(&mut sol, &req.edits) {
        return (StatusCode::BAD_REQUEST, format!("edit failed: {}", e)).into_response();
    }

    // 4. Re-serialize
    let new_source = veil_ir::serialize_solution(&sol);

    // 5. Validate
    let reparsed = match parse_source(&new_source, state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("edit produced invalid source: {}", e)).into_response(),
    };
    let errors = veil_ir::validate::validate_solution(&reparsed, state.registry());
    if !errors.is_empty() {
        let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        return (StatusCode::BAD_REQUEST, format!("validation failed: {}", msg)).into_response();
    }

    // 6. Write back via provider
    if let Err(e) = state.write_source("", &new_source).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }

    // 7. Return fresh state
    let graph = veil_ir::build_ir(&reparsed);
    let project = veil_codegen::generate(&reparsed, state.registry());
    let generated: std::collections::HashMap<String, String> = project.files.iter()
        .map(|f| (f.path.clone(), f.content.clone()))
        .collect();

    let response = EditResponse {
        source: new_source,
        ir: serde_json::to_value(&graph).unwrap_or(serde_json::Value::Null),
        generated: serde_json::to_value(&generated).unwrap_or(serde_json::Value::Null),
    };
    Json(response).into_response()
}
