//! Rust code generation from VEIL AST.
//!
//! Fully shape-driven: constructs are generated according to their core
//! shape (`mod` → crate, `struct`/`enum` → types, `trait` → async traits,
//! `impl` → adapter structs, `fn` → application functions). The construct's
//! layer subkind appears only in doc comments — never in generation logic.
//!
//! ## Module structure
//!
//! - `generate` — top-level `generate` entry point, collect_by_shape, flatten_module
//! - `harness` — axum harness generation (HTTP server, routing, auth, CORS)
//! - `workspace` — Cargo workspace, bin crate, module crates, manifest
//! - `types` — struct/enum/type generation, field defaults, constructors
//! - `shared` — shared crate (bus impl, auth impl, handler registration)
//! - `traits_impls` — trait + impl generation, adapters, generic monomorphization
//! - `application` — application functions, runtime delegation, flow return types
//! - `type_convert` — type name conversion (to_snake, type_to_rust, multi-package harness)

mod generate;
mod harness;
mod workspace;
mod types;
mod shared;
mod traits_impls;
mod application;
mod type_convert;

// Re-export everything so `crate::rust::X` paths continue to work unchanged.
pub use generate::*;
pub use harness::*;
pub use workspace::*;
pub use types::*;
pub use shared::*;
pub use traits_impls::*;
pub use application::*;
pub use type_convert::*;
