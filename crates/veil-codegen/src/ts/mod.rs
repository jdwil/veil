//! TypeScript IR and emission — structured codegen for the TS target.
//!
//! ## Module structure
//!
//! - `expr` — `TsExpr` enum, `TsType`, operators, patterns
//! - `emit` — `emit_ts()` renders TsExpr trees to TypeScript source text
//!
//! ## Pipeline (when complete)
//!
//! ```text
//! VEIL Expr → lower_to_ts(expr, &GenCtx) → TsExpr
//!                                            ↓
//!                              apply_ts_transforms(expr)
//!                              (import tracking, async detection,
//!                               null-safety insertion)
//!                                            ↓
//!                                       emit_ts(expr) → String
//! ```

pub mod emit;
pub mod expr;
pub mod lower;
#[cfg(test)]
mod tests;

// Public API
pub use emit::emit_ts;
pub use expr::{TsBinOp, TsExpr, TsPattern, TsTemplatePart, TsType, TsUnaryOp};
pub use lower::{lower_to_ts, to_camel_case, veil_type_to_ts};
