use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use acr_core::value::Value;

/// Working state maintained across algorithm executions in a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkingState {
    /// Key-value store for intermediate results.
    pub memory: HashMap<String, Value>,
    /// History of algorithm executions in this session.
    pub history: Vec<ExecutionRecord>,
}

/// A record of a single algorithm execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub algorithm_name: String,
    pub input_summary: String,
    pub output: Value,
    pub success: bool,
}

impl WorkingState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: String, value: Value) {
        self.memory.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.memory.get(key)
    }

    pub fn record_execution(
        &mut self,
        name: String,
        input_summary: String,
        output: Value,
        success: bool,
    ) {
        self.history.push(ExecutionRecord {
            algorithm_name: name,
            input_summary,
            output,
            success,
        });
    }
}
