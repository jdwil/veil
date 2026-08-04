use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("No algorithm found for goal: {0}")]
    NoAlgorithmFound(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(#[from] acr_core::error::AcrError),
    #[error("Library error: {0}")]
    LibraryError(#[from] acr_library::error::LibraryError),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}
