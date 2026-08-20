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

pub mod components;
pub mod emit;
pub mod expr;
pub mod generate;
pub mod lower;
pub mod transforms;
#[cfg(test)]
mod tests;

// Public API
pub use emit::emit_ts;
pub use expr::{TsBinOp, TsExpr, TsPattern, TsTemplatePart, TsType, TsUnaryOp};
pub use generate::generate_ts_ir;
pub use lower::{lower_to_ts, lower_block, to_camel_case, veil_type_to_ts};
pub use transforms::{track_imports, detect_async, import_statement};
pub use components::gen_svelte_component;
pub use components::{gen_svelte_component_at, gen_svelte_store, sveltekit_output_path, SvelteFile};
