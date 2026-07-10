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
    pub files: Vec<FileEntry>,
    pub registry: LayerRegistry,
    pub active_file: Mutex<usize>,  // index into files
}

/// A loaded .veil file with its source and path.
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub source: Mutex<String>,
    pub editable: bool,
}

impl ServeState {
    pub fn new(file: PathBuf, source: String, registry: LayerRegistry, editable: bool) -> Self {
        let name = file.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        ServeState {
            files: vec![FileEntry {
                path: file,
                name,
                source: Mutex::new(source),
                editable,
            }],
            registry,
            active_file: Mutex::new(0),
        }
    }

    pub fn with_files(files: Vec<(PathBuf, String, bool)>, registry: LayerRegistry) -> Self {
        let entries = files.into_iter().map(|(path, source, editable)| {
            let name = path.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            FileEntry { path, name, source: Mutex::new(source), editable }
        }).collect();
        ServeState {
            files: entries,
            registry,
            active_file: Mutex::new(0),
        }
    }

    fn active_entry(&self) -> &FileEntry {
        let idx = *self.active_file.lock().unwrap();
        &self.files[idx]
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

    /// Run the unified check pipeline (validation + graph diagnostics).
    fn diagnostics_json(&self, source: &str) -> Result<String, String> {
        let sol = self.parse(source)?;
        let result = veil_ir::check_solution(&sol, &self.registry);
        serde_json::to_string(&result.diagnostics).map_err(|e| e.to_string())
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
    let entry = state.active_entry();
    let source = entry.source.lock().unwrap().clone();
    match state.ir_json(&source) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e).into_response(),
    }
}

pub async fn get_diagnostics(State(state): State<SharedState>) -> axum::response::Response {
    let entry = state.active_entry();
    let source = entry.source.lock().unwrap().clone();
    match state.diagnostics_json(&source) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e).into_response(),
    }
}

pub async fn get_source(State(state): State<SharedState>) -> impl IntoResponse {
    let entry = state.active_entry();
    let source = entry.source.lock().unwrap().clone();
    ([(header::CONTENT_TYPE, "text/plain")], source)
}

pub async fn get_generated(State(state): State<SharedState>) -> axum::response::Response {
    let entry = state.active_entry();
    let source = entry.source.lock().unwrap().clone();
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

/// GET /api/files — list all loaded files with their names and active status.
pub async fn get_files(State(state): State<SharedState>) -> axum::response::Response {
    let active_idx = *state.active_file.lock().unwrap();
    let files: Vec<serde_json::Value> = state.files.iter().enumerate().map(|(i, entry)| {
        serde_json::json!({
            "index": i,
            "name": entry.name,
            "path": entry.path.to_string_lossy(),
            "editable": entry.editable,
            "active": i == active_idx,
        })
    }).collect();
    match serde_json::to_string(&files) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e.to_string()).into_response(),
    }
}

/// POST /api/files/select — switch the active file by index.
#[derive(Debug, Deserialize)]
pub struct SelectFileRequest {
    pub index: usize,
}

pub async fn post_select_file(
    State(state): State<SharedState>,
    Json(req): Json<SelectFileRequest>,
) -> axum::response::Response {
    if req.index >= state.files.len() {
        return (StatusCode::BAD_REQUEST, "invalid file index".to_string()).into_response();
    }
    *state.active_file.lock().unwrap() = req.index;
    // Return the IR for the newly selected file
    let entry = &state.files[req.index];
    let source = entry.source.lock().unwrap().clone();
    match state.ir_json(&source) {
        Ok(json) => json_ok(json).into_response(),
        Err(e) => server_error(e).into_response(),
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
    let entry = state.active_entry();
    if !entry.editable {
        return (StatusCode::BAD_REQUEST, "this file is served read-only".to_string())
            .into_response();
    }

    // Lock for the whole edit so the write-back is atomic.
    let mut guard = entry.source.lock().unwrap();

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
    let check = veil_ir::check_solution(&reparsed, &state.registry);
    if check.has_errors() {
        let msg = check
            .errors()
            .map(veil_ir::format_diagnostic_line)
            .collect::<Vec<_>>()
            .join("; ");
        return (StatusCode::BAD_REQUEST, format!("validation failed: {}", msg)).into_response();
    }

    // 5. Commit: write the file and update in-memory state.
    if let Err(e) = std::fs::write(&entry.path, &new_source) {
        return server_error(format!("failed to write file: {}", e)).into_response();
    }
    *guard = new_source.clone();

    // 6. Return fresh IR + generated so the viewer refreshes in one round-trip.
    let graph = check.graph;
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
