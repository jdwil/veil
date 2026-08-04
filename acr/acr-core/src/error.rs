//! Error types for the ACR execution engine.

use thiserror::Error;

/// Errors that can occur during algorithm execution.
#[derive(Debug, Error, Clone)]
pub enum AcrError {
    #[error("execution timed out")]
    ExecutionTimeout,

    #[error("step limit exceeded: {limit} steps")]
    StepLimitExceeded { limit: usize },

    #[error("stack overflow at depth {depth}")]
    StackOverflow { depth: usize },

    #[error("type error: expected {expected}, got {got}")]
    TypeError { expected: String, got: String },

    #[error("undefined variable: {name}")]
    UndefinedVariable { name: String },

    #[error("undefined function: {name}")]
    UndefinedFunction { name: String },

    #[error("division by zero")]
    DivisionByZero,

    #[error("index out of bounds: index {index}, length {len}")]
    IndexOutOfBounds { index: i64, len: usize },

    #[error("invalid argument count: expected {expected}, got {got}")]
    InvalidArgCount { expected: usize, got: usize },

    #[error("assertion failed: {message}")]
    AssertionFailed { message: String },

    #[error("{message}")]
    Custom { message: String },
}
