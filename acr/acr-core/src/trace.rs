//! Structured execution traces for algorithm runs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::value::Value;

/// A complete trace of an algorithm execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub algorithm_id: Uuid,
    pub algorithm_version: u32,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub input: Vec<(String, Value)>,
    pub output: ExecutionResult,
    pub steps_executed: usize,
    pub max_stack_depth: usize,
    pub events: Vec<TraceEvent>,
}

/// The result of executing an algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionResult {
    Success(Value),
    Error(String),
    Timeout,
    StepLimitExceeded,
}

/// A single event recorded during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub step: usize,
    pub kind: TraceEventKind,
}

/// The kind of trace event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraceEventKind {
    VarAssigned { name: String, value: Value },
    FunctionCalled { name: String, args: Vec<Value> },
    BranchTaken { condition_value: bool },
    LoopIteration { iteration: usize },
    AssertPassed { message: String },
    AssertFailed { message: String },
    Returned(Value),
}
