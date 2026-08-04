use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Execution error: {0}")]
    Execution(#[from] acr_core::error::AcrError),
    #[error("Library error: {0}")]
    Library(#[from] acr_library::error::LibraryError),
    #[error("Invalid task definition: {0}")]
    InvalidTask(String),
}
