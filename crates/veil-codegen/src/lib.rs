//! VEIL Codegen — generates code from VEIL AST.
//!
//! Supports multiple target languages via `CodegenTarget`.

pub mod expr;
pub mod rust;
pub mod typescript;

pub use rust::generate;
pub use typescript::generate_ts;

use veil_ir::ast::Solution;
use veil_ir::layer::LayerRegistry;

/// Target language for code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenTarget {
    Rust,
    TypeScript,
}

impl CodegenTarget {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "typescript" | "ts" => Some(Self::TypeScript),
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
    }
}
