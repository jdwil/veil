//! HTTP API route handlers for the VEIL dev server.
//!
//! All handlers are parameterized by the [`SourceProvider`] trait — they don't
//! know whether source lives on disk or in a remote VCS.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json,
};
use tower_http::cors::CorsLayer;

use crate::protocol::{CheckRequest, CheckResponse, EditRequest, EditResponse};
use crate::provider::SourceProvider;

/// Build the complete axum Router for the VEIL dev server API.
///
/// The returned router handles:
/// - `GET /api/ir` — current IR graph as JSON
/// - `GET /api/source` — raw .veil source text
/// - `GET /api/generated` — generated code map
/// - `GET /api/palette` — construct palette from loaded layers
/// - `GET /api/stubs` — loaded external crate APIs
/// - `GET /api/diagnostics` — diagnostics array (compat; same pipeline as check)
/// - `GET|POST /api/check` — full check pipeline (CHK-007)
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
        .route("/api/check", get(get_check::<P>).post(post_check::<P>))
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

/// Query params for `GET /api/check` and `GET /api/diagnostics`.
#[derive(Debug, Default, serde::Deserialize)]
struct CheckQuery {
    /// Codegen target: `rust` (default) or `typescript` / `ts`.
    #[serde(default)]
    target: Option<String>,
    /// Promote escape-hatch debt to errors.
    #[serde(default)]
    deny_escape_hatches: Option<bool>,
    /// Include multi-target debt warnings (default true when target is rust).
    #[serde(default)]
    target_debt: Option<bool>,
}

/// Run the full check pipeline (same as CLI `veil check`), including target
/// capability matrix and escape-hatch debt.
fn run_check(
    sol: &veil_ir::Solution,
    registry: &veil_ir::LayerRegistry,
    target_str: &str,
    deny_escape_hatches: bool,
    target_debt: bool,
) -> Result<CheckResponse, String> {
    let codegen_target = veil_codegen::CodegenTarget::from_str(target_str).ok_or_else(|| {
        format!("unknown target '{}'; use rust or typescript", target_str)
    })?;

    let mut result = veil_ir::check_solution(sol, registry);
    result.diagnostics.extend(veil_codegen::check_target_capabilities(
        sol,
        registry,
        codegen_target,
    ));
    if target_debt && codegen_target == veil_codegen::CodegenTarget::Rust {
        result
            .diagnostics
            .extend(veil_codegen::check_multi_target_debt(sol, registry));
    }
    if deny_escape_hatches {
        veil_ir::promote_escape_hatches(&mut result.diagnostics);
    }
    veil_ir::sort_diagnostics(&mut result.diagnostics);

    let escape_hatch = veil_ir::EscapeHatchSummary::from_diagnostics(&result.diagnostics);
    let error_count = result.error_count();
    let warning_count = result.warning_count();
    let ok = !result.has_errors();

    Ok(CheckResponse {
        diagnostics: result.diagnostics,
        error_count,
        warning_count,
        target: target_str.to_string(),
        escape_hatch,
        ok,
    })
}

async fn get_diagnostics<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Query(q): Query<CheckQuery>,
) -> axum::response::Response {
    // Compat: return diagnostics array only (same pipeline as /api/check).
    match run_check_for_provider(&*state, &q).await {
        Ok(resp) => match serde_json::to_string(&resp.diagnostics) {
            Ok(json) => json_response(json).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err((status, msg)) => (status, msg).into_response(),
    }
}

async fn get_check<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Query(q): Query<CheckQuery>,
) -> axum::response::Response {
    match run_check_for_provider(&*state, &q).await {
        Ok(resp) => match serde_json::to_string(&resp) {
            Ok(json) => {
                let status = if resp.ok {
                    StatusCode::OK
                } else {
                    // 200 still OK for tooling that only reads body; clients use `ok` field.
                    // Use 422 when there are errors so agents can branch on HTTP status.
                    StatusCode::UNPROCESSABLE_ENTITY
                };
                // Prefer always 200 with ok:false so IDE fetch is simpler; story doesn't require 422.
                let _ = status;
                json_response(json).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err((status, msg)) => (status, msg).into_response(),
    }
}

async fn post_check<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<CheckRequest>,
) -> axum::response::Response {
    let q = CheckQuery {
        target: req.target,
        deny_escape_hatches: Some(req.deny_escape_hatches),
        target_debt: Some(req.target_debt),
    };
    match run_check_for_provider(&*state, &q).await {
        Ok(resp) => match serde_json::to_string(&resp) {
            Ok(json) => json_response(json).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err((status, msg)) => (status, msg).into_response(),
    }
}

async fn run_check_for_provider<P: SourceProvider>(
    state: &P,
    q: &CheckQuery,
) -> Result<CheckResponse, (StatusCode, String)> {
    let source = state
        .read_source("")
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let sol = parse_source(&source, state.registry())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("parse failed: {}", e)))?;
    let target = q.target.as_deref().unwrap_or("rust");
    let deny = q.deny_escape_hatches.unwrap_or(false);
    let debt = q.target_debt.unwrap_or(true);
    run_check(&sol, state.registry(), target, deny, debt)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
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
    // 5. Full check (same pipeline as /api/check) — reject on errors
    let check_resp = match run_check(&reparsed, state.registry(), "rust", false, false) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    if !check_resp.ok {
        let msg = check_resp
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
            .map(veil_ir::format_diagnostic_line)
            .collect::<Vec<_>>()
            .join("; ");
        return (StatusCode::BAD_REQUEST, format!("validation failed: {}", msg)).into_response();
    }

    // 6. Write back via provider
    if let Err(e) = state.write_source("", &new_source).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }

    // 7. Return fresh state + diagnostics
    let graph = veil_ir::build_ir(&reparsed);
    let project = veil_codegen::generate(&reparsed, state.registry());
    let generated: std::collections::HashMap<String, String> = project.files.iter()
        .map(|f| (f.path.clone(), f.content.clone()))
        .collect();

    let response = EditResponse {
        source: new_source,
        ir: serde_json::to_value(&graph).unwrap_or(serde_json::Value::Null),
        generated: serde_json::to_value(&generated).unwrap_or(serde_json::Value::Null),
        diagnostics: Some(check_resp.diagnostics),
    };
    Json(response).into_response()
}
