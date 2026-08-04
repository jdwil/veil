use std::sync::Arc;

use acr_core::executor::ExecutorConfig;
use acr_core::trace::ExecutionResult;
use acr_core::value::Value;
use acr_library::store::AlgorithmStore;

use crate::error::RuntimeError;
use crate::selector::{AlgorithmSelector, Goal};
use crate::state::WorkingState;

/// The ACR runtime: selects and executes promoted algorithms without teacher LLM involvement.
pub struct Runtime {
    store: Arc<dyn AlgorithmStore>,
    executor_config: ExecutorConfig,
    state: WorkingState,
}

impl Runtime {
    /// Create a new runtime with default executor configuration.
    pub fn new(store: Arc<dyn AlgorithmStore>) -> Self {
        Self {
            store,
            executor_config: ExecutorConfig::default(),
            state: WorkingState::new(),
        }
    }

    /// Create a new runtime with a custom executor configuration.
    pub fn with_config(store: Arc<dyn AlgorithmStore>, config: ExecutorConfig) -> Self {
        Self {
            store,
            executor_config: config,
            state: WorkingState::new(),
        }
    }

    /// Execute a goal using promoted algorithms from the library.
    ///
    /// Strategy (v0): select matching algorithms, try each in ranked order,
    /// return the first successful result.
    pub async fn execute_goal(&mut self, goal: Goal) -> Result<Value, RuntimeError> {
        // 1. Select relevant algorithms
        let algorithms = AlgorithmSelector::select(&self.store, &goal).await?;

        // 2. Try algorithms in order (first success wins for v0)
        let mut last_error = None;

        for algorithm in &algorithms {
            let result = acr_core::executor::Executor::execute_with_config(
                algorithm,
                goal.input.clone(),
                self.executor_config.clone(),
            );

            match result {
                Ok(trace) => match &trace.output {
                    ExecutionResult::Success(value) => {
                        self.state.record_execution(
                            algorithm.name.clone(),
                            format!("{:?}", goal.input),
                            value.clone(),
                            true,
                        );
                        self.state.set("last_result".to_string(), value.clone());
                        return Ok(value.clone());
                    }
                    ExecutionResult::Error(e) => {
                        self.state.record_execution(
                            algorithm.name.clone(),
                            format!("{:?}", goal.input),
                            Value::Null,
                            false,
                        );
                        last_error = Some(RuntimeError::ExecutionFailed(
                            acr_core::error::AcrError::Custom {
                                message: e.clone(),
                            },
                        ));
                    }
                    ExecutionResult::Timeout => {
                        self.state.record_execution(
                            algorithm.name.clone(),
                            format!("{:?}", goal.input),
                            Value::Null,
                            false,
                        );
                        last_error = Some(RuntimeError::ExecutionFailed(
                            acr_core::error::AcrError::ExecutionTimeout,
                        ));
                    }
                    ExecutionResult::StepLimitExceeded => {
                        self.state.record_execution(
                            algorithm.name.clone(),
                            format!("{:?}", goal.input),
                            Value::Null,
                            false,
                        );
                        last_error = Some(RuntimeError::ExecutionFailed(
                            acr_core::error::AcrError::StepLimitExceeded {
                                limit: self.executor_config.max_steps,
                            },
                        ));
                    }
                },
                Err(e) => {
                    self.state.record_execution(
                        algorithm.name.clone(),
                        format!("{:?}", goal.input),
                        Value::Null,
                        false,
                    );
                    last_error = Some(RuntimeError::ExecutionFailed(e));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| RuntimeError::NoAlgorithmFound(goal.description)))
    }

    /// Get the current working state.
    pub fn state(&self) -> &WorkingState {
        &self.state
    }

    /// Get a mutable reference to the working state.
    pub fn state_mut(&mut self) -> &mut WorkingState {
        &mut self.state
    }

    /// Reset the working state for a new session.
    pub fn reset_state(&mut self) {
        self.state = WorkingState::new();
    }
}
