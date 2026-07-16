//! VEIL IR — AST definitions, graph model, and validation.

pub mod adapt;
pub mod ast;
pub mod builder;
pub mod check;
pub mod context;
pub mod deps;
pub mod diagnostics;
pub mod edit;
pub mod escape;
pub mod ir;
pub mod layer;
pub mod layer_graph;
pub mod names;
pub mod presentation;
pub mod project;
pub mod resolve;
pub mod typecheck;
pub mod serialize;
pub mod span;
pub mod struct_diff;
pub mod validate;

pub use adapt::{
    build_adapt_chain, default_adapt_search_paths, find_package_source, inject_implicit_adapts,
    is_adapt_denied, merge_adapt_chain, merge_adapted_package, package_as_solution, path_exists,
    AdaptError, MergeResult, ADAPT_DENYLIST,
};
pub use deps::{
    adapt_search_paths_for_file, deps_cache_dir, find_project_root, layer_source_in_root,
    load_package_entry, load_product_deps, missing_package_hint, package_source_in_root,
    product_provides_use, projects_hub, resolve_dep_root, resolve_dependency_roots,
    resolve_dependency_roots_for, PackageEntry, ProductDep,
};
pub use ast::*;
pub use builder::{build_ir, build_ir_with_registry};
pub use check::{
    check_solution, format_diagnostic_line, parse_error_diagnostic, sort_diagnostics,
    CheckResult, StructuredCheckReport, StructuredDiagnostic, StructuredSpan,
};
pub use context::{build_context_pack, ContextPack, ContextQuery};
pub use diagnostics::{Diagnostic, Severity};
pub use escape::{
    check_escape_hatches, is_escape_hatch_code, promote_escape_hatches, EscapeHatchSummary,
};
pub use edit::{apply_edit, apply_edit_with, apply_edits, apply_edits_with, EditError, EditOp};
pub use ir::*;
pub use layer::{
    palette_from_registry, parse_layer_file, parse_stub_file, CodegenRule, CodegenTemplate,
    ConstructSpec, ConstructorPolicy, LayerRegistry, RawLayer, ReactivityPolicy, Shape,
    StatementSpec, StmtShape, StubCrate, StubImpl, StubMethod, StubStruct,
};
pub use layer_graph::{build_layer_ir, check_layer, layer_prompt};
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
