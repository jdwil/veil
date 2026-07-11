//! VEIL Codegen — generates code from VEIL AST.
//!
//! Supports multiple target languages via `CodegenTarget`.

pub mod capabilities;
pub mod expr;
pub mod kotlin;
pub mod links;
pub mod rust;
pub mod swift;
pub mod template;
pub mod typescript;

pub use links::{cargo_dep_line, resolve_link, resolve_links, ResolvedLink};

pub use capabilities::{
    check_multi_target_debt, check_target_capabilities, target_capability_summary, Feature,
};
pub use rust::generate;
pub use template::execute_templates;
pub use typescript::generate_ts;

use veil_ir::ast::Solution;
use veil_ir::layer::LayerRegistry;

/// Target language for code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenTarget {
    Rust,
    TypeScript,
    /// PAR-005 spike — not production
    Swift,
    /// PAR-006 spike — not production
    Kotlin,
}

impl CodegenTarget {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "typescript" | "ts" => Some(Self::TypeScript),
            "swift" => Some(Self::Swift),
            "kotlin" | "kt" => Some(Self::Kotlin),
            _ => None,
        }
    }
}

/// Generated output file (target-agnostic).
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// Generate code for the specified target.
pub fn generate_for_target(
    solution: &Solution,
    registry: &LayerRegistry,
    target: CodegenTarget,
) -> Vec<GeneratedFile> {
    match target {
        CodegenTarget::Rust => {
            let project = rust::generate(solution, registry);
            project.files.into_iter()
                .map(|f| GeneratedFile { path: f.path, content: f.content })
                .collect()
        }
        CodegenTarget::TypeScript => {
            let project = typescript::generate_ts(solution, registry);
            project.files.into_iter()
                .map(|f| GeneratedFile { path: f.path, content: f.content })
                .collect()
        }
        CodegenTarget::Swift => {
            let project = swift::generate_swift(solution, registry);
            project.files.into_iter()
                .map(|f| GeneratedFile { path: f.path, content: f.content })
                .collect()
        }
        CodegenTarget::Kotlin => {
            let project = kotlin::generate_kotlin(solution, registry);
            project.files.into_iter()
                .map(|f| GeneratedFile { path: f.path, content: f.content })
                .collect()
        }
    }
}

/// Generate code for the specified target, with optional package expose blocks
/// for typed API client generation (TS target).
pub fn generate_for_target_with_packages(
    solution: &Solution,
    registry: &LayerRegistry,
    target: CodegenTarget,
    used_packages: &[(String, veil_ir::ast::ExposeBlock)],
) -> Vec<GeneratedFile> {
    match target {
        CodegenTarget::Rust => {
            let project = rust::generate(solution, registry);
            project.files.into_iter()
                .map(|f| GeneratedFile { path: f.path, content: f.content })
                .collect()
        }
        CodegenTarget::TypeScript => {
            let project = typescript::generate_ts_with_packages(solution, registry, used_packages);
            project.files.into_iter()
                .map(|f| GeneratedFile { path: f.path, content: f.content })
                .collect()
        }
        other => generate_for_target(solution, registry, other),
    }
}
