//! ACR Core - Types, traits, and data structures for the Algorithm Crystallized Representation.
//!
//! This crate defines the Algorithm IR, a custom AST for a restricted expression language
//! that is safely executable, serializable, and diffable.

pub mod error;
pub mod ir;
pub mod trace;
pub mod value;

// Executor will be implemented separately
pub mod executor;

pub use error::AcrError;
pub use ir::{Algorithm, AlgorithmMetadata, BinOp, Expr, LiteralValue, Param, Provenance, Statement, TypeHint, UnaryOp};
pub use trace::{ExecutionResult, ExecutionTrace, TraceEvent, TraceEventKind};
pub use value::Value;
