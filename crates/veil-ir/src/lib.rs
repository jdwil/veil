//! VEIL IR — AST definitions, graph model, and validation.

pub mod ast;
pub mod builder;
pub mod check;
pub mod diagnostics;
pub mod edit;
pub mod escape;
pub mod ir;
pub mod layer;
pub mod names;
pub mod presentation;
pub mod resolve;
pub mod typecheck;
pub mod serialize;
pub mod span;
pub mod validate;

pub use ast::*;
pub use builder::build_ir;
pub use check::{check_solution, format_diagnostic_line, sort_diagnostics, CheckResult};
pub use diagnostics::{Diagnostic, Severity};
pub use escape::{
    check_escape_hatches, is_escape_hatch_code, promote_escape_hatches, EscapeHatchSummary,
};
pub use edit::{apply_edit, apply_edit_with, apply_edits, apply_edits_with, EditError, EditOp};
pub use ir::*;
pub use layer::{CodegenRule, CodegenTemplate, ConstructSpec, LayerRegistry, Shape, StatementSpec, StmtShape, palette_from_registry};
pub use presentation::{
    presentation_from_registry, ConstructPresentation, HostPresentation, NestRule, NestableHint,
    PresentationModel, ViewSpec,
};
pub use resolve::{ResolvedPackage, build_composition_ir, find_package_files, resolve_package};
pub use serialize::{serialize_solution, serialize_package, serialize_composition};
pub use span::Span;
