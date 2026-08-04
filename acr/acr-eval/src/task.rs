use serde::{Deserialize, Serialize};
use acr_core::value::Value;

/// A single test case for an algorithm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub name: String,
    pub input: Vec<Value>,
    pub expected_output: Value,
}

/// A task = a collection of test cases that an algorithm should solve
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub difficulty: Difficulty,
    pub param_hints: Vec<ParamHint>,
    pub test_cases: Vec<TestCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamHint {
    pub name: String,
    pub description: String,
    pub type_hint: String,
}

/// Result of running one test case
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub case_name: String,
    pub passed: bool,
    pub actual_output: Option<Value>,
    pub expected_output: Value,
    pub error: Option<String>,
    pub steps_used: usize,
}

/// Result of running all test cases for a task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub task_id: String,
    pub algorithm_id: uuid::Uuid,
    pub algorithm_version: u32,
    pub total_cases: usize,
    pub passed: usize,
    pub failed: usize,
    pub score: f64,
    pub test_results: Vec<TestResult>,
    pub total_steps: usize,
}
