//! TypeScript IR and emission — structured codegen for the TS target.
//!
//! ## Module structure
//!
//! - `expr` — `TsExpr` enum, `TsType`, operators, patterns
//! - `emit` — `emit_ts()` renders TsExpr trees to TypeScript source text
//! - `lower` — `lower_to_ts()` converts VEIL Expr → TsExpr IR, plus type/name mapping
//! - `api_client` — typed API client generation from expose blocks
//! - `transforms` — import tracking, async detection
//! - `generate` — top-level project generation pipeline
//!
//! ## Pipeline
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

pub mod api_client;
pub mod emit;
pub mod expr;
pub mod generate;
pub mod lower;
pub mod transforms;
#[cfg(test)]
mod tests;

// Public API
pub use api_client::{TsFile, TsProject};
pub use emit::emit_ts;
pub use expr::{TsBinOp, TsExpr, TsPattern, TsTemplatePart, TsType, TsUnaryOp};
pub use generate::generate_ts_ir;
pub use lower::{lower_to_ts, lower_block, to_camel, to_camel_case, type_to_ts, infer_field_type_ts, field_type_ts, veil_type_to_ts};
pub use transforms::{track_imports, detect_async, import_statement};
