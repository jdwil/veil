use serde::{Deserialize, Serialize};
use uuid::Uuid;

use acr_core::ir::Statement;
use acr_eval::task::EvaluationResult;
use acr_library::metadata::AlgorithmEntry;
use acr_library::store::LibraryStatus;

// list_algorithms
#[derive(Debug, Deserialize)]
pub struct ListAlgorithmsRequest {
    pub domain: Option<String>,
    pub status: Option<String>, // "candidate", "promoted", "retired"
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct ListAlgorithmsResponse {
    pub algorithms: Vec<AlgorithmEntry>,
}

// create_candidate
#[derive(Debug, Deserialize)]
pub struct CreateCandidateRequest {
    pub name: String,
    pub description: String,
    pub domain: String,
    pub tags: Option<Vec<String>>,
    pub params: Vec<ParamDef>,
    pub body: Vec<Statement>,
    pub provenance: Option<ProvenanceDef>,
}

#[derive(Debug, Deserialize)]
pub struct ParamDef {
    pub name: String,
    pub type_hint: String, // "any", "bool", "int", "float", "str", "list", "map"
}

#[derive(Debug, Deserialize)]
pub struct ProvenanceDef {
    pub kind: String, // "generated", "mutated", "composed", "manual"
    pub by: Option<String>,
    pub prompt: Option<String>,
    pub from_id: Option<Uuid>,
    pub from_version: Option<u32>,
    pub sources: Option<Vec<Uuid>>,
}

#[derive(Debug, Serialize)]
pub struct CreateCandidateResponse {
    pub id: Uuid,
    pub version: u32,
    pub entry: AlgorithmEntry,
}

// update_candidate
#[derive(Debug, Deserialize)]
pub struct UpdateCandidateRequest {
    pub id: Uuid,
    pub description: Option<String>,
    pub params: Option<Vec<ParamDef>>,
    pub body: Vec<Statement>,
    pub change_summary: String,
}

#[derive(Debug, Serialize)]
pub struct UpdateCandidateResponse {
    pub id: Uuid,
    pub version: u32,
    pub entry: AlgorithmEntry,
}

// run_evaluation
#[derive(Debug, Deserialize)]
pub struct RunEvaluationRequest {
    pub algorithm_id: Uuid,
    pub task_id: String,
    pub version: Option<u32>, // defaults to latest
}

#[derive(Debug, Serialize)]
pub struct RunEvaluationResponse {
    pub result: EvaluationResult,
    pub trace_id: Option<String>,
}

// get_trace
#[derive(Debug, Deserialize)]
pub struct GetTraceRequest {
    pub trace_id: String,
}

// promote
#[derive(Debug, Deserialize)]
pub struct PromoteRequest {
    pub algorithm_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct PromoteResponse {
    pub entry: AlgorithmEntry,
}

// list_tasks
#[derive(Debug, Serialize)]
pub struct ListTasksResponse {
    pub tasks: Vec<TaskSummary>,
}

#[derive(Debug, Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub difficulty: String,
    pub num_test_cases: usize,
}

// get_library_status
#[derive(Debug, Serialize)]
pub struct GetLibraryStatusResponse {
    pub status: LibraryStatus,
}

// Generic error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
