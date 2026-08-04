use acr_core::executor::{Executor, ExecutorConfig};
use acr_core::ir::Algorithm;
use acr_core::trace::ExecutionResult;

use crate::error::EvalError;
use crate::scoring::{calculate_score, values_match};
use crate::task::{EvaluationResult, Task, TestResult};

pub struct EvalHarness {
    config: ExecutorConfig,
}

impl EvalHarness {
    pub fn new() -> Self {
        Self {
            config: ExecutorConfig::default(),
        }
    }

    pub fn with_config(config: ExecutorConfig) -> Self {
        Self { config }
    }

    /// Run an algorithm against all test cases in a task
    pub fn evaluate(
        &self,
        algorithm: &Algorithm,
        task: &Task,
    ) -> Result<EvaluationResult, EvalError> {
        let mut test_results = Vec::new();
        let mut total_steps = 0;

        for case in &task.test_cases {
            let trace_result =
                Executor::execute_with_config(algorithm, case.input.clone(), self.config.clone());

            let result = match trace_result {
                Ok(trace) => {
                    total_steps += trace.steps_executed;
                    let passed = match &trace.output {
                        ExecutionResult::Success(val) => values_match(val, &case.expected_output),
                        _ => false,
                    };
                    let actual = match &trace.output {
                        ExecutionResult::Success(val) => Some(val.clone()),
                        _ => None,
                    };
                    let error = match &trace.output {
                        ExecutionResult::Error(e) => Some(e.clone()),
                        ExecutionResult::Timeout => Some("Timeout".to_string()),
                        ExecutionResult::StepLimitExceeded => {
                            Some("Step limit exceeded".to_string())
                        }
                        _ => None,
                    };
                    TestResult {
                        case_name: case.name.clone(),
                        passed,
                        actual_output: actual,
                        expected_output: case.expected_output.clone(),
                        error,
                        steps_used: trace.steps_executed,
                    }
                }
                Err(e) => TestResult {
                    case_name: case.name.clone(),
                    passed: false,
                    actual_output: None,
                    expected_output: case.expected_output.clone(),
                    error: Some(e.to_string()),
                    steps_used: 0,
                },
            };
            test_results.push(result);
        }

        let score = calculate_score(&test_results);
        let passed = test_results.iter().filter(|r| r.passed).count();
        let failed = test_results.len() - passed;

        Ok(EvaluationResult {
            task_id: task.id.clone(),
            algorithm_id: algorithm.id,
            algorithm_version: algorithm.version,
            total_cases: test_results.len(),
            passed,
            failed,
            score,
            test_results,
            total_steps,
        })
    }
}

impl Default for EvalHarness {
    fn default() -> Self {
        Self::new()
    }
}
