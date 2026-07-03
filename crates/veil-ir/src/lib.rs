//! VEIL IR — AST definitions, graph model, and validation.

pub mod ast;
pub mod builder;
pub mod ir;
pub mod resolve;
pub mod serialize;
pub mod span;
pub mod validate;

pub use ast::*;
pub use builder::build_ir;
pub use ir::*;
pub use resolve::{ResolvedPackage, build_composition_ir, find_package_files, resolve_package};
pub use serialize::{serialize_solution, serialize_package, serialize_composition};
pub use span::Span;
