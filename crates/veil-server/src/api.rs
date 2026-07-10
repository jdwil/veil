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
use crate::provider::{FileKind, SourceProvider};

/// Build the complete axum Router for the VEIL dev server API.
///
/// The returned router handles:
/// - `GET /api/ir` — current IR graph as JSON
/// - `GET /api/source` — raw .veil source text
/// - `GET /api/generated` — generated code map
/// - `GET /api/palette` — construct palette from loaded layers
/// - `GET /api/presentation` — layer-driven views / nest rules (LAY-002)
/// - `GET /api/context` — agent context pack: outline + presentation (LAY-010)
/// - `GET /api/stubs` — loaded external crate APIs
/// - `GET /api/diagnostics` — diagnostics array (compat; same pipeline as check)
/// - `GET|POST /api/check` — full check pipeline (CHK-007)
/// - `GET /api/files` — list loaded files
/// - `POST /api/files/select` — switch active file
/// - `POST /api/edit` — apply structured edits
/// - `GET /api/diff` — structural IR diff vs git HEAD (UX-021)
/// - `POST /api/agent/turn` — built-in agent turn (AGT-001)
/// - `GET /api/events` — SSE source-change stream (AGT-002 MVP)
pub fn build_router<P: SourceProvider>(provider: P) -> Router {
    let state = Arc::new(provider);

    let mut router = Router::new()
        .route("/api/ir", get(get_ir::<P>))
        .route("/api/source", get(get_source::<P>).post(post_source::<P>))
        .route("/api/generated", get(get_generated::<P>))
        .route("/api/palette", get(get_palette::<P>))
        .route("/api/presentation", get(get_presentation::<P>))
        .route("/api/context", get(get_context::<P>))
        .route("/api/stubs", get(get_stubs::<P>))
        .route("/api/diagnostics", get(get_diagnostics::<P>))
        .route("/api/check", get(get_check::<P>).post(post_check::<P>))
        .route("/api/files", get(get_files::<P>))
        .route("/api/files/select", post(post_select_file::<P>))
        .route("/api/edit", post(post_edit::<P>))
        .route("/api/diff", get(get_diff::<P>))
        .route("/api/agent/turn", post(post_agent_turn::<P>))
        .route("/api/agent/turn/stream", post(post_agent_turn_stream::<P>))
        .route("/api/agent/tools", get(get_agent_tools))
        .route("/api/events", get(get_events::<P>))
        .route("/api/models", get(get_models))
        .route("/api/layer/dependents", get(get_layer_dependents::<P>))
        .route("/api/layer/scaffold", post(post_layer_scaffold::<P>))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // AGT-016: optional bearer auth when VEIL_AUTH_TOKEN is set.
    if let Ok(token) = std::env::var("VEIL_AUTH_TOKEN") {
        if !token.is_empty() {
            let expected = token;
            router = router.layer(axum::middleware::from_fn(
                move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let expected = expected.clone();
                    async move {
                        let ok = req
                            .headers()
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .map(|h| {
                                h == expected
                                    || h.strip_prefix("Bearer ")
                                        .map(|t| t == expected)
                                        .unwrap_or(false)
                            })
                            .unwrap_or(false);
                        if ok {
                            Ok(next.run(req).await)
                        } else {
                            Err(StatusCode::UNAUTHORIZED)
                        }
                    }
                },
            ));
        }
    }

    router
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
    let kind = state.file_kind("");
    let graph = match kind {
        FileKind::Layer => {
            let name = layer_name_from_files(state.as_ref()).await;
            match veil_ir::build_layer_ir(&source, &name) {
                Ok(g) => g,
                Err(e) => {
                    // Empty graph with error still allows IDE shell
                    let mut g = veil_ir::IrGraph::new();
                    let id = g.add_node(
                        veil_ir::NodeKind::Solution,
                        name.clone(),
                        veil_ir::Span::new(0, source.len()),
                    );
                    let _ = id;
                    g.nodes.last_mut().map(|n| {
                        n.metadata.doc = Some(format!("layer parse error: {e}"));
                        n.metadata.subkind = Some("Layer".into());
                    });
                    g
                }
            }
        }
        FileKind::Stub => {
            let mut g = veil_ir::IrGraph::new();
            g.add_node(
                veil_ir::NodeKind::Solution,
                "stub".into(),
                veil_ir::Span::new(0, source.len()),
            );
            g
        }
        FileKind::Package => {
            let sol = match parse_source(&source, &state.registry()) {
                Ok(s) => s,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
            };
            veil_ir::build_ir_with_registry(&sol, Some(&state.registry()))
        }
    };
    match serde_json::to_string(&graph) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn layer_name_from_files<P: SourceProvider>(state: &P) -> String {
    let files = state.list_files().await;
    files
        .into_iter()
        .find(|f| f.active)
        .map(|f| {
            std::path::Path::new(&f.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| f.name.clone())
        })
        .unwrap_or_else(|| "layer".into())
}

async fn get_source<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    match state.read_source("").await {
        Ok(source) => ([(header::CONTENT_TYPE, "text/plain")], source).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// AGT-010: full-file write for remote SourceStore clients.
async fn post_source<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    body: String,
) -> axum::response::Response {
    if !state.is_editable("") {
        return (StatusCode::FORBIDDEN, "file is read-only").into_response();
    }
    match state.file_kind("") {
        FileKind::Layer => {
            let name = layer_name_from_files(state.as_ref()).await;
            let diags = veil_ir::check_layer(&body, &name);
            if diags.iter().any(|d| matches!(d.severity, veil_ir::Severity::Error)) {
                let msg = diags
                    .iter()
                    .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
                    .map(veil_ir::format_diagnostic_line)
                    .collect::<Vec<_>>()
                    .join("; ");
                return (StatusCode::BAD_REQUEST, format!("layer validation failed: {msg}"))
                    .into_response();
            }
        }
        FileKind::Stub => {}
        FileKind::Package => {
            match parse_source(&body, &state.registry()) {
                Ok(sol) => {
                    let check = veil_ir::check_solution(&sol, &state.registry());
                    if check.has_errors() {
                        let msg = check
                            .diagnostics
                            .iter()
                            .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
                            .map(veil_ir::format_diagnostic_line)
                            .collect::<Vec<_>>()
                            .join("; ");
                        return (
                            StatusCode::BAD_REQUEST,
                            format!("validation failed: {msg}"),
                        )
                            .into_response();
                    }
                }
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, format!("parse error: {e}")).into_response();
                }
            }
        }
    }
    match state.write_source("", &body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// UX-021: structural IR diff of active file vs git HEAD (when available).
async fn get_diff<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let head_src = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let kind = state.file_kind("");
    let head_ir = match kind {
        FileKind::Layer => {
            let name = layer_name_from_files(state.as_ref()).await;
            veil_ir::build_layer_ir(&head_src, &name).unwrap_or_else(|_| veil_ir::IrGraph::new())
        }
        FileKind::Stub => veil_ir::IrGraph::new(),
        FileKind::Package => {
            let head_sol = match parse_source(&head_src, &state.registry()) {
                Ok(s) => s,
                Err(e) => {
                    return (StatusCode::BAD_REQUEST, format!("parse head: {}", e)).into_response();
                }
            };
            veil_ir::build_ir_with_registry(&head_sol, Some(&state.registry()))
        }
    };

    let baseline = match state.baseline_source("").await {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let (base_label, base_ir) = match baseline {
        Some((label, src)) => {
            let ir = match kind {
                FileKind::Layer => {
                    let name = layer_name_from_files(state.as_ref()).await;
                    veil_ir::build_layer_ir(&src, &name).unwrap_or_else(|_| veil_ir::IrGraph::new())
                }
                FileKind::Stub => veil_ir::IrGraph::new(),
                FileKind::Package => match parse_source(&src, &state.registry()) {
                    Ok(sol) => veil_ir::build_ir_with_registry(&sol, Some(&state.registry())),
                    Err(e) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            format!("parse baseline ({label}): {e}"),
                        )
                            .into_response();
                    }
                },
            };
            (label, ir)
        }
        None => {
            // No git baseline — empty base (everything appears as added).
            ("(no baseline)".into(), veil_ir::IrGraph::new())
        }
    };

    let diff = veil_ir::structural_diff(&base_ir, &head_ir, &base_label, "working tree");
    match serde_json::to_string(&diff) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_generated<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    if !matches!(state.file_kind(""), FileKind::Package) {
        // Layers/stubs do not codegen
        return json_response("{}".into()).into_response();
    }
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let sol = match parse_source(&source, &state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let project = veil_codegen::generate(&sol, &state.registry());
    let files_map: std::collections::HashMap<String, String> = project.files.iter()
        .map(|f| (f.path.clone(), f.content.clone()))
        .collect();
    match serde_json::to_string(&files_map) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_palette<P: SourceProvider>(State(state): State<SharedProvider<P>>) -> axum::response::Response {
    let palette = veil_ir::palette_from_registry(&state.registry());
    match serde_json::to_string(&palette) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Layer-driven presentation model (views, nest rules, roles, lenses).
/// See `docs/PRESENTATION.md` / LAY-002. Empty hosts when no `present` blocks.
async fn get_presentation<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
) -> axum::response::Response {
    let model = veil_ir::presentation_from_registry(&state.registry());
    match serde_json::to_string(&model) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Agent context pack: topology outline + presentation + optional host projection.
/// Query: `?host_id=N&view_id=model&max_tokens=N` (LAY-010 / AGT-015).
#[derive(Debug, Default, serde::Deserialize)]
struct ContextApiQuery {
    host_id: Option<u32>,
    view_id: Option<String>,
    /// Approximate token budget (chars/4). 0 or omit = unlimited.
    #[serde(default)]
    max_tokens: Option<usize>,
}

/// AGT-015: truncate a context pack JSON string to ~max_tokens (chars/4).
fn apply_token_budget(mut json: String, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return json;
    }
    let max_chars = max_tokens.saturating_mul(4);
    if json.len() <= max_chars {
        return json;
    }
    json.truncate(max_chars);
    // Keep valid-ish JSON by closing a truncated string blob
    json.push_str("…[truncated for token budget]…\"}");
    json
}

async fn get_context<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Query(q): Query<ContextApiQuery>,
) -> axum::response::Response {
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let sol = match parse_source(&source, &state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let graph = veil_ir::build_ir_with_registry(&sol, Some(&state.registry()));
    let pack = veil_ir::build_context_pack(
        &graph,
        &state.registry(),
        &veil_ir::ContextQuery {
            host_id: q.host_id,
            view_id: q.view_id,
        },
    );
    match serde_json::to_string(&pack) {
        Ok(json) => {
            let max = q
                .max_tokens
                .or_else(|| {
                    std::env::var("VEIL_CONTEXT_MAX_TOKENS")
                        .ok()
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(0);
            let json = apply_token_budget(json, max);
            json_response(json).into_response()
        }
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
    /// Include multi-target debt warnings (default **false** — primary target only).
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
    let target = q.target.as_deref().unwrap_or("rust");
    let deny = q.deny_escape_hatches.unwrap_or(false);
    let debt = q.target_debt.unwrap_or(false);

    if matches!(state.file_kind(""), FileKind::Layer) {
        let name = layer_name_from_files(state).await;
        let mut diagnostics = veil_ir::check_layer(&source, &name);
        veil_ir::sort_diagnostics(&mut diagnostics);
        let error_count = diagnostics
            .iter()
            .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
            .count();
        let warning_count = diagnostics
            .iter()
            .filter(|d| matches!(d.severity, veil_ir::Severity::Warning))
            .count();
        return Ok(CheckResponse {
            diagnostics,
            error_count,
            warning_count,
            target: target.to_string(),
            escape_hatch: veil_ir::EscapeHatchSummary::default(),
            ok: error_count == 0,
        });
    }

    let sol = parse_source(&source, &state.registry())
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("parse failed: {}", e)))?;
    run_check(&sol, &state.registry(), target, deny, debt)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

async fn get_layer_dependents<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let layer = match q.get("layer").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => layer_name_from_files(state.as_ref()).await,
    };
    let deps = state.layer_dependents(&layer).await;
    match serde_json::to_string(&serde_json::json!({ "layer": layer, "dependents": deps })) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct ScaffoldRequest {
    /// Layer package name (file stem), e.g. `loyalty`
    name: String,
    /// Optional description
    #[serde(default)]
    desc: Option<String>,
    /// Directory relative to CWD (default `layers`)
    #[serde(default)]
    dir: Option<String>,
}

async fn post_layer_edit<P: SourceProvider>(
    state: SharedProvider<P>,
    req: EditRequest,
) -> axum::response::Response {
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let name = layer_name_from_files(state.as_ref()).await;
    let new_src = match crate::layer_edit::apply_layer_edits(&source, &req.edits) {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let diags = veil_ir::check_layer(&new_src, &name);
    if diags
        .iter()
        .any(|d| matches!(d.severity, veil_ir::Severity::Error))
    {
        let msg = diags
            .iter()
            .filter(|d| matches!(d.severity, veil_ir::Severity::Error))
            .map(veil_ir::format_diagnostic_line)
            .collect::<Vec<_>>()
            .join("; ");
        return (StatusCode::BAD_REQUEST, format!("validation failed: {msg}")).into_response();
    }
    if let Err(e) = state.write_source("", &new_src).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    let graph = veil_ir::build_layer_ir(&new_src, &name).unwrap_or_else(|_| veil_ir::IrGraph::new());
    let response = EditResponse {
        source: new_src,
        ir: serde_json::to_value(&graph).unwrap_or(serde_json::Value::Null),
        generated: serde_json::json!({}),
        diagnostics: Some(diags),
    };
    Json(response).into_response()
}

async fn post_layer_scaffold<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<ScaffoldRequest>,
) -> axum::response::Response {
    let name = req.name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return (
            StatusCode::BAD_REQUEST,
            "invalid layer name (use alphanumerics, _ , -)",
        )
            .into_response();
    }
    let dir = req.dir.as_deref().unwrap_or("layers");
    let path = std::path::Path::new(dir).join(format!("{name}.layer"));
    if path.exists() {
        return (StatusCode::CONFLICT, format!("{} already exists", path.display()))
            .into_response();
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let desc = req
        .desc
        .unwrap_or_else(|| format!("{name} team DSL"));
    let content = format!(
        "pkg {name} v1\n  desc \"{desc}\"\n  author \"VEIL\"\n\n  construct Example\n    kw example\n    mt struct\n    desc \"Starter construct — rename me\"\n    visual\n      icon \"📦\"\n      color \"#6366f1\"\n      label \"Example\"\n    group domain\n\n  prompt\n    You are authoring packages that use the `{name}` layer.\n    Prefer layer keywords; keep platform packages as dependencies.\n"
    );
    if let Err(e) = std::fs::write(&path, &content) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let idx = match state.register_file(path.clone(), content.clone(), true) {
        Ok(i) => i,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
    };
    crate::revision::bus().publish(content.len(), &path.to_string_lossy(), "scaffold_layer");
    match serde_json::to_string(&serde_json::json!({
        "index": idx,
        "path": path.to_string_lossy(),
        "name": format!("{name}.layer"),
        "kind": "layer",
    })) {
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
    // UX-011 / DSL-001: switch active file, then return IR for package or layer.
    if let Err(e) = state.set_active(req.index) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    // Reuse get_ir logic
    get_ir(State(state)).await
}

async fn post_edit<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<EditRequest>,
) -> axum::response::Response {
    if !state.is_editable("") {
        return (StatusCode::BAD_REQUEST, "file is read-only").into_response();
    }

    if matches!(state.file_kind(""), FileKind::Layer) {
        // Layer structured ops: apply textual patch helpers (DSL-006/008)
        return post_layer_edit(state, req).await;
    }

    // AGT-017: remote SourceStore — forward structured edit to host serve
    let edit_json = match serde_json::to_string(&req) {
        Ok(j) => j,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if let Some(remote) = state.forward_edit(&edit_json).await {
        return match remote {
            Ok(body) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
            Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
        };
    }

    // Local path: read → apply → check → write
    let source = match state.read_source("").await {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let mut sol = match parse_source(&source, &state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("parse failed: {}", e)).into_response(),
    };

    if let Err(e) = veil_parser::apply_edits(&mut sol, &req.edits, &state.registry()) {
        return (StatusCode::BAD_REQUEST, format!("edit failed: {}", e)).into_response();
    }

    let new_source = veil_ir::serialize_solution(&sol);

    let reparsed = match parse_source(&new_source, &state.registry()) {
        Ok(s) => s,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("edit produced invalid source: {}", e)).into_response(),
    };
    let check_resp = match run_check(&reparsed, &state.registry(), "rust", false, false) {
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

    if let Err(e) = state.write_source("", &new_source).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }

    let graph = veil_ir::build_ir_with_registry(&reparsed, Some(&state.registry()));
    let project = veil_codegen::generate(&reparsed, &state.registry());
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

// ─── Agent (AGT-001) ───────────────────────────────────────────────────────

async fn post_agent_turn<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<crate::agent::AgentTurnRequest>,
) -> axum::response::Response {
    let resp = crate::agent::run_turn(state.clone(), req).await;
    match serde_json::to_string(&resp) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Streaming agent turn (SSE) — chunks typewriter-style for the IDE.
///
/// Events: `status` | `chunk` | `tool` | `done` | `error`
async fn post_agent_turn_stream<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
    Json(req): Json<crate::agent::AgentTurnRequest>,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::stream::unfold;
    use std::convert::Infallible;
    use std::time::Duration;

    let (tx, rx) = tokio::sync::mpsc::channel::<(String, String)>(128);
    tokio::spawn(async move {
        crate::agent_stream::run_turn_stream(state, req, tx).await;
    });

    let stream = unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some((event, data)) => {
                let ev = Event::default().event(event).data(data);
                Some((Ok::<_, Infallible>(ev), rx))
            }
            None => None,
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
        .into_response()
}

/// AGT-003: list models for the configured Rig-backed provider.
async fn get_models() -> axum::response::Response {
    let body = crate::model::list_provider_info();
    match serde_json::to_string(&body) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// AGT-008 MVP: expose Rig tool schemas for MCP / external agents.
/// Full MCP stdio server can wrap this registry (rig-mcp / rmcp follow-up).
async fn get_agent_tools() -> axum::response::Response {
    let tools = serde_json::json!([
        {
            "name": "veil_check",
            "description": "Run check on the active file (package dual-loop or layer validate).",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "veil_outline",
            "description": "Compact IR construct outline for the active package or layer.",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "read_source",
            "description": "Read active .veil or .layer source (truncated).",
            "parameters": {
                "type": "object",
                "properties": {
                    "max_chars": { "type": "integer" }
                }
            }
        },
        {
            "name": "rename_construct",
            "description": "Rename a construct via structured EditOp (packages) or layer text patch.",
            "parameters": {
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "confirmed": { "type": "boolean" }
                },
                "required": ["from", "to"]
            }
        },
        {
            "name": "list_layers",
            "description": "List .layer files in the serve set (DSL-011).",
            "parameters": { "type": "object", "properties": {} }
        },
        {
            "name": "layer_outline",
            "description": "Outline constructs/keywords for the active layer.",
            "parameters": { "type": "object", "properties": {} }
        }
    ]);
    let body = serde_json::json!({
        "protocol": "veil-tools-v1",
        "mcp_note": "HTTP tool discovery for MCP bridges; host tools via Rig. Full MCP server = AGT-008 follow-up with rig-mcp/rmcp.",
        "tools": tools
    });
    match serde_json::to_string(&body) {
        Ok(json) => json_response(json).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// AGT-002 / AGT-018: live SSE revision stream.
///
/// Sends an immediate snapshot, then streams every `revision::bus` publish
/// (agent mid-turn writes, POST /api/source, structured edits).
async fn get_events<P: SourceProvider>(
    State(state): State<SharedProvider<P>>,
) -> axum::response::Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::stream::{self, StreamExt};
    use std::convert::Infallible;
    use std::time::Duration;

    let src = state.read_source("").await.unwrap_or_default();
    let remote = state.remote_events_url();
    let bus = crate::revision::bus();
    let rx = bus.subscribe();

    let initial = crate::revision::RevisionEvent {
        revision: bus.current(),
        bytes: src.len(),
        path: String::new(),
        reason: "subscribe".into(),
    };
    let mut init_json = serde_json::to_string(&initial).unwrap_or_else(|_| "{}".into());
    if let Some(ref u) = remote {
        // Attach remote hint for proxies without breaking schema consumers.
        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&init_json) {
            v.as_object_mut()
                .map(|m| m.insert("remote_events".into(), serde_json::json!(u)));
            init_json = v.to_string();
        }
    }

    let stream = stream::once(async move {
        Ok::<_, Infallible>(Event::default().event("revision").data(init_json))
    })
    .chain(stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                    return Some((
                        Ok::<_, Infallible>(Event::default().event("revision").data(data)),
                        rx,
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    }));

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
        .into_response()
}
