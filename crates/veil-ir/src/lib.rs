//! VEIL IR — AST definitions, graph model, and validation.

pub mod ast;
pub mod builder;
pub mod check;
pub mod context;
pub mod diagnostics;
pub mod edit;
pub mod escape;
pub mod ir;
pub mod layer;
pub mod names;
pub mod presentation;
pub mod project;
pub mod resolve;
pub mod typecheck;
pub mod serialize;
pub mod span;
pub mod struct_diff;
pub mod validate;

pub use ast::*;
pub use builder::{build_ir, build_ir_with_registry};
pub use check::{check_solution, format_diagnostic_line, sort_diagnostics, CheckResult};
pub use context::{build_context_pack, ContextPack, ContextQuery};
pub use diagnostics::{Diagnostic, Severity};
pub use escape::{
    check_escape_hatches, is_escape_hatch_code, promote_escape_hatches, EscapeHatchSummary,
};
pub use edit::{apply_edit, apply_edit_with, apply_edits, apply_edits_with, EditError, EditOp};
pub use ir::*;
pub use layer::{
    palette_from_registry, CodegenRule, CodegenTemplate, ConstructSpec, ConstructorPolicy,
    LayerRegistry, Shape, StatementSpec, StmtShape,
};
pub use presentation::{
    presentation_from_registry, ConstructPresentation, HostPresentation, NestRule, NestableHint,
    PresentationModel, ViewSpec,
};
pub use project::{
    orphan_policy_valid, parse_orphan_policy, project_view, project_view_with_edges, resolve_layout,
    ProjectEdge, ProjectInputNode, ProjectOutput, MVP_LAYOUTS, NEST_WHENS,
};
pub use resolve::{ResolvedPackage, build_composition_ir, find_package_files, resolve_package};
pub use serialize::{serialize_solution, serialize_package, serialize_composition};
pub use span::Span;
pub use struct_diff::{structural_diff, DiffItem, StructDiff};
