//! ACR Runtime - Executes promoted algorithms without teacher LLM involvement.
//!
//! Given a goal, the runtime:
//! 1. Selects relevant algorithms from the promoted library
//! 2. Executes them using the bounded ACR interpreter
//! 3. Maintains working state across executions
//! 4. Produces output

pub mod error;
pub mod runtime;
pub mod selector;
pub mod state;

pub use runtime::Runtime;
