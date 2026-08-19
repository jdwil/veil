//! Expression translator — converts VEIL AST Expr to Rust source code.
//!
//! Fully shape-driven: the translator uses `GenCtx.name_to_shape` to decide
//! how to emit a Call (port call → deps.x.method().await?, struct call →
//! Type::new(args), local → target.method(args)).
//!
//! ## Module structure
//!
//! - `context` — GenCtx struct and solution-level context building
//! - `types` — type helpers, copy/clone analysis, string/numeric predicates
//! - `translate` — `expr_to_rust` entry point (thin IR wrapper) + fstring helpers
//! - `rust_ir` — typed IR (`RustExpr`), `lower_to_rust`, `emit`, `apply_ownership`
//! - `calls` — method/function call translation (receiver dispatch, args, bus)
//! - `actions` — layer statement/action translation, guard emission
//! - `patterns` — pattern translation, block emission, indentation
//! - `analysis` — mutability analysis, ident usage counting
//! - `inference` — type inference, bus return resolution, deps collection

mod context;
mod types;
mod translate;
mod calls;
mod actions;
mod patterns;
mod analysis;
mod inference;
pub mod rust_ir;

// Re-export everything so `crate::expr::X` paths continue to work unchanged.
pub use context::*;
pub use types::*;
pub use translate::*;
pub use calls::*;
pub use actions::*;
pub use patterns::*;
pub use analysis::*;
pub use inference::*;
pub use rust_ir::{RustExpr, RustType, emit, lower_to_rust, apply_ownership, suppress_try_in_closure};
