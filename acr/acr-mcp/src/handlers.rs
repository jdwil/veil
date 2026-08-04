use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;

use acr_core::ir::{Algorithm, Param, Provenance, TypeHint};
use acr_core::trace::ExecutionTrace;
use acr_eval::harness::EvalHarness;
use acr_eval::task::{Difficulty, Task};
use acr_library::metadata::{LibraryQuery, PromotionStatus, ScoreRecord};
use acr_library::store::AlgorithmStore;

use crate::types::*;

pub struct AppState {
    pub store: Arc<dyn AlgorithmStore>,
    pub tasks: Vec<Task>,
    pub harness: EvalHarness,
}

type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

fn err_response(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: msg.into(),
        }),
    )
}

fn parse_type_hint(s: &str) -> Result<TypeHint, String> {
    match s.to_lowercase().as_str() {
        "any" => Ok(TypeHint::Any),
        "bool" => Ok(TypeHint::Bool),
        "int" => Ok(TypeHint::Int),
        "float" => Ok(TypeHint::Float),
        "str" => Ok(TypeHint::Str),
        "list" => Ok(TypeHint::List(Box::new(TypeHint::Any))),
        "map" => Ok(TypeHint::Map),
        other => Err(format!("Unknown type hint: {other}")),
    }
}

fn parse_promotion_status(s: &str) -> Result<PromotionStatus, String> {
    match s.to_lowercase().as_str() {
        "candidate" => Ok(PromotionStatus::Candidate),
        "promoted" => Ok(PromotionStatus::Promoted),
        "retired" => Ok(PromotionStatus::Retired),
        other => Err(format!("Unknown status: {other}")),
    }
}

fn parse_provenance(def: Option<ProvenanceDef>) -> Provenance {
    match def {
        None => Provenance::Manual,
        Some(p) => match p.kind.to_lowercase().as_str() {
            "generated" => Provenance::Generated {
                by: p.by.unwrap_or_else(|| "unknown".to_string()),
                prompt: p.prompt,
            },
            "mutated" => Provenance::Mutated {
                from_id: p.from_id.unwrap_or_default(),
                from_version: p.from_version.unwrap_or(1),
            },
            "composed" => Provenance::Composed {
                sources: p.sources.unwrap_or_default(),
            },
            _ => Provenance::Manual,
        },
    }
}

fn difficulty_to_string(d: &Difficulty) -> String {
    match d {
        Difficulty::Easy => "easy".to_string(),
        Difficulty::Medium => "medium".to_string(),
        Difficulty::Hard => "hard".to_string(),
    }
}

fn convert_params(defs: &[ParamDef]) -> Result<Vec<Param>, String> {
    defs.iter()
        .map(|p| {
            let type_hint = parse_type_hint(&p.type_hint)?;
            Ok(Param {
                name: p.name.clone(),
                type_hint,
            })
        })
        .collect()
}

// POST /tools/list_algorithms
pub async fn list_algorithms(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ListAlgorithmsRequest>,
) -> HandlerResult<ListAlgorithmsResponse> {
    let status = match req.status {
        Some(ref s) => Some(
            parse_promotion_status(s)
                .map_err(|e| err_response(StatusCode::BAD_REQUEST, e))?,
        ),
        None => None,
    };

    let query = LibraryQuery {
        domain: req.domain,
        tags: req.tags.unwrap_or_default(),
        status,
        name_contains: None,
    };

    let algorithms = state
        .store
        .list(&query)
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ListAlgorithmsResponse { algorithms }))
}

// POST /tools/create_candidate
pub async fn create_candidate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCandidateRequest>,
) -> HandlerResult<CreateCandidateResponse> {
    let params = convert_params(&req.params)
        .map_err(|e| err_response(StatusCode::BAD_REQUEST, e))?;

    let provenance = parse_provenance(req.provenance);

    let mut algorithm = Algorithm::new(
        req.name,
        req.description,
        req.domain.clone(),
        params,
        req.body,
    );

    // Set tags and provenance on metadata
    algorithm.metadata.tags = req.tags.unwrap_or_default();
    algorithm.metadata.provenance = provenance;

    let entry = state
        .store
        .create(&algorithm)
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(CreateCandidateResponse {
        id: algorithm.id,
        version: algorithm.version,
        entry,
    }))
}

// POST /tools/update_candidate
pub async fn update_candidate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateCandidateRequest>,
) -> HandlerResult<UpdateCandidateResponse> {
    // Load current version
    let mut algorithm = state
        .store
        .get(req.id)
        .await
        .map_err(|e| err_response(StatusCode::NOT_FOUND, e.to_string()))?;

    // Bump version
    algorithm.version += 1;
    algorithm.body = req.body;
    algorithm.created_at = Utc::now();

    if let Some(desc) = req.description {
        algorithm.description = desc;
    }

    if let Some(param_defs) = req.params {
        let params = convert_params(&param_defs)
            .map_err(|e| err_response(StatusCode::BAD_REQUEST, e))?;
        algorithm.params = params;
    }

    let entry = state
        .store
        .update(&algorithm)
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(UpdateCandidateResponse {
        id: algorithm.id,
        version: algorithm.version,
        entry,
    }))
}

// POST /tools/run_evaluation
pub async fn run_evaluation(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RunEvaluationRequest>,
) -> HandlerResult<RunEvaluationResponse> {
    // Load algorithm (specific version or latest)
    let algorithm = match req.version {
        Some(v) => state
            .store
            .get_version(req.algorithm_id, v)
            .await
            .map_err(|e| err_response(StatusCode::NOT_FOUND, e.to_string()))?,
        None => state
            .store
            .get(req.algorithm_id)
            .await
            .map_err(|e| err_response(StatusCode::NOT_FOUND, e.to_string()))?,
    };

    // Find the task
    let task = state
        .tasks
        .iter()
        .find(|t| t.id == req.task_id)
        .ok_or_else(|| err_response(StatusCode::NOT_FOUND, format!("Task not found: {}", req.task_id)))?;

    // Run evaluation
    let result = state
        .harness
        .evaluate(&algorithm, task)
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Record score
    let score_record = ScoreRecord {
        version: algorithm.version,
        score: result.score,
        task_id: task.id.clone(),
        evaluated_at: Utc::now(),
    };
    state
        .store
        .record_score(req.algorithm_id, score_record)
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store trace
    let trace = ExecutionTrace {
        algorithm_id: algorithm.id,
        algorithm_version: algorithm.version,
        started_at: Utc::now(),
        completed_at: Utc::now(),
        input: vec![],
        output: acr_core::trace::ExecutionResult::Success(acr_core::value::Value::Null),
        steps_executed: result.total_steps,
        max_stack_depth: 0,
        events: vec![],
    };

    let trace_id = state
        .store
        .store_trace(&trace)
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(RunEvaluationResponse {
        result,
        trace_id: Some(trace_id),
    }))
}

// POST /tools/get_trace
pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GetTraceRequest>,
) -> HandlerResult<serde_json::Value> {
    let trace = state
        .store
        .get_trace(&req.trace_id)
        .await
        .map_err(|e| err_response(StatusCode::NOT_FOUND, e.to_string()))?;

    let value = serde_json::to_value(&trace)
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(value))
}

// POST /tools/promote
pub async fn promote(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PromoteRequest>,
) -> HandlerResult<PromoteResponse> {
    let entry = state
        .store
        .promote(req.algorithm_id)
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(PromoteResponse { entry }))
}

// POST /tools/list_tasks
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
) -> HandlerResult<ListTasksResponse> {
    let tasks = state
        .tasks
        .iter()
        .map(|t| TaskSummary {
            id: t.id.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            domain: t.domain.clone(),
            difficulty: difficulty_to_string(&t.difficulty),
            num_test_cases: t.test_cases.len(),
        })
        .collect();

    Ok(Json(ListTasksResponse { tasks }))
}

// POST /tools/get_library_status
pub async fn get_library_status(
    State(state): State<Arc<AppState>>,
) -> HandlerResult<GetLibraryStatusResponse> {
    let status = state
        .store
        .status()
        .await
        .map_err(|e| err_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(GetLibraryStatusResponse { status }))
}
