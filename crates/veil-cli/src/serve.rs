//! Serve-command state and HTTP handlers, including the write-back pipeline.
//!
//! The server treats the on-disk `.veil` source as the single source of truth.
//! GET endpoints re-derive the IR graph and generated code live from it, so a
//! successful edit is immediately reflected everywhere. `POST /api/edit`
//! applies a structured edit to the parsed AST, re-serializes (the serializer
//! is idempotent), validates, and writes the file back — the "viewer IS the
//! editor" loop.

use std::path::PathBuf;
use std::sync::Mutex;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use veil_ir::{EditOp, LayerRegistry};

/// Shared, mutable server state.
pub struct ServeState {
    pub file: PathBuf,
    pub registry: LayerRegistry,
    pub editable: bool,
    /// The current .veil source — the source of truth. Guarded so an edit is
    /// atomic against concurrent reads.
    pub source: Mutex<String>,
}

impl ServeState {
    pub fn new(file: PathBuf, source: String, registry: LayerRegistry, editable: bool) -> Self {
        ServeState {
            file,
            registry,
            editable,
            source: Mutex::new(source),
        }
    }

    /// Parse the current source into a Solution using the server's registry.
    fn parse(&self, source: &str) -> Result<veil_ir::Solution, String> {
        let tokens = veil_parser::lex(source);
        veil_parser::parse_with_registry(&tokens, self.registry.clone())
            .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "))
    }

    /// Build the IR graph JSON for the current source.
    fn ir_json(&self, source: &str) -> Result<String, String> {
        let sol = self.parse(source)?;
        let graph = veil_ir::build_ir(&sol);
        serde_json::to_string(&graph).map_err(|e| e.to_string())
    }

    /// Build the generated-code map JSON for the current source.
    fn generated_json(&self, source: &str) -> Result<String, String> {
        let sol = self.parse(source)?;
        let project = veil_codegen::generate(&sol, &self.registry);
        let files_map: std::collections::HashMap<String, String> = project
            .files
            .iter()
            .map(|f| (f.path.clone(), f.content.clone()))
            .collect();
        serde_json::to_string(&files_map).map_err(|e| e.to_string())
    }
}

type SharedState = std::sync::Arc<ServeState>;

fn json_ok(body: String) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], body)
}

fn server_error(msg: String) -> impl IntoResponse {
    (StatusCode::INTERNAL_SERVER_ERROR, msg)
}

pub async fn get_ir(State(state): State<SharedState>) -> axum::response::Response {
    let source = state.source.lock().unwrap().clone();
    match state.ir_json(&source) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e).into_response(),
    }
}

pub async fn get_source(State(state): State<SharedState>) -> impl IntoResponse {
    let source = state.source.lock().unwrap().clone();
    ([(header::CONTENT_TYPE, "text/plain")], source)
}

pub async fn get_generated(State(state): State<SharedState>) -> axum::response::Response {
    let source = state.source.lock().unwrap().clone();
    match state.generated_json(&source) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e).into_response(),
    }
}

/// Serve the loaded `.stub` crates (external crate APIs) so the viewer can
/// show them in a dedicated "External" palette section (UX-006).
pub async fn get_stubs(State(state): State<SharedState>) -> axum::response::Response {
    match serde_json::to_string(&state.registry.stubs) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/edit`: an ordered batch of structured edits.
#[derive(Debug, Deserialize)]
pub struct EditRequest {
    pub edits: Vec<EditOp>,
}

/// Response for a successful edit: fresh source, IR, and generated code, so the
/// viewer can update every panel from one round-trip.
#[derive(Debug, Serialize)]
pub struct EditResponse {
    pub source: String,
    pub ir: serde_json::Value,
    pub generated: serde_json::Value,
}

pub async fn post_edit(
    State(state): State<SharedState>,
    Json(req): Json<EditRequest>,
) -> axum::response::Response {
    if !state.editable {
        return (StatusCode::BAD_REQUEST, "this file is served read-only".to_string())
            .into_response();
    }

    // Lock for the whole edit so the write-back is atomic.
    let mut guard = state.source.lock().unwrap();

    // 1. Parse current source into the AST.
    let mut sol = match state.parse(&guard) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("parse failed: {}", e)).into_response(),
    };

    // 2. Apply the structured edits to the AST.
    if let Err(e) = veil_ir::apply_edits(&mut sol, &req.edits) {
        return (StatusCode::BAD_REQUEST, format!("edit failed: {}", e)).into_response();
    }

    // 3. Re-serialize to VEIL source (idempotent).
    let new_source = veil_ir::serialize_solution(&sol);

    // 4. Re-parse + validate the edited source before committing it, so a bad
    //    edit never corrupts the file on disk.
    let reparsed = match state.parse(&new_source) {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("edit produced invalid source: {}", e)).into_response(),
    };
    let errors = veil_ir::validate::validate_solution(&reparsed, &state.registry);
    if !errors.is_empty() {
        let msg = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        return (StatusCode::BAD_REQUEST, format!("validation failed: {}", msg)).into_response();
    }

    // 5. Commit: write the file and update in-memory state.
    if let Err(e) = std::fs::write(&state.file, &new_source) {
        return server_error(format!("failed to write file: {}", e)).into_response();
    }
    *guard = new_source.clone();

    // 6. Return fresh IR + generated so the viewer refreshes in one round-trip.
    let graph = veil_ir::build_ir(&reparsed);
    let project = veil_codegen::generate(&reparsed, &state.registry);
    let generated: std::collections::HashMap<String, String> = project
        .files
        .iter()
        .map(|f| (f.path.clone(), f.content.clone()))
        .collect();

    let response = EditResponse {
        source: new_source,
        ir: serde_json::to_value(&graph).unwrap_or(serde_json::Value::Null),
        generated: serde_json::to_value(&generated).unwrap_or(serde_json::Value::Null),
    };
    Json(response).into_response()
}
