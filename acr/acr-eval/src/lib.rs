//! ACR - Evaluation engine for component selection and ranking.

pub mod builtin_tasks;
pub mod error;
pub mod harness;
pub mod scoring;
pub mod task;

pub use error::EvalError;
pub use harness::EvalHarness;
pub use scoring::{calculate_score, values_match};
pub use task::{Difficulty, EvaluationResult, ParamHint, Task, TestCase, TestResult};
