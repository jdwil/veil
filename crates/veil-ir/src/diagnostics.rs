//! IR diagnostics — detects structural issues in the graph.
//!
//! Prefer [`crate::check::check_solution`] as the public entry point; this
//! module holds graph-level rules that run after AST validation.

use crate::ir::{EdgeKind, IrGraph, NodeKind};
use crate::layer::LayerRegistry;
use serde::Serialize;

/// A single diagnostic warning/error.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    /// The node ID this diagnostic applies to (if any).
    pub node_id: Option<u64>,
    /// The name of the affected construct.
    pub node_name: Option<String>,
    /// Machine-stable rule id (e.g. `must_have`, `requires_implementation`).
    pub code: String,
    /// Same as `code` for backward compatibility with the viewer/API clients.
    pub constraint: String,
    /// Parent construct name when the issue is nested (AST validation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Optional remediation hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Source span start (byte offset), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_start: Option<usize>,
    /// Source span end (byte offset), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_end: Option<usize>,
}

impl Diagnostic {
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        node_name: Option<String>,
    ) -> Self {
        let code = code.into();
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            node_id: None,
            node_name,
            constraint: code.clone(),
            code,
            parent: None,
            hint: None,
            span_start: None,
            span_end: None,
        }
    }

    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        node_name: Option<String>,
    ) -> Self {
        let code = code.into();
        Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            node_id: None,
            node_name,
            constraint: code.clone(),
            code,
            parent: None,
            hint: None,
            span_start: None,
            span_end: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

/// Analyze the IR graph against constraints declared in the layer registry.
///
/// Prefer [`crate::check::check_solution`] which also runs AST validation.
pub fn analyze(graph: &IrGraph, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    check_requires_implementation(graph, registry, &mut diagnostics);
    diagnostics
}

/// Check: for every node whose layer spec has `requires_implementation`,
/// verify there's at least one Implements edge targeting it.
fn check_requires_implementation(
    graph: &IrGraph,
    registry: &LayerRegistry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let implemented_targets: std::collections::HashSet<u64> = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .map(|e| e.to)
        .collect();

    for node in &graph.nodes {
        if node.kind != NodeKind::Interface {
            continue;
        }

        let subkind = match &node.metadata.subkind {
            Some(sk) => sk,
            None => continue,
        };

        let spec = match registry.construct_by_name(subkind) {
            Some(s) => s,
            None => continue,
        };

        let has_constraint = spec
            .constraints
            .iter()
            .any(|c| c == "requires_implementation");
        if !has_constraint {
            continue;
        }

        if !implemented_targets.contains(&node.id) {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!("{} '{}' has no implementation", subkind, node.name),
                node_id: Some(node.id),
                node_name: Some(node.name.clone()),
                code: "requires_implementation".to_string(),
                constraint: "requires_implementation".to_string(),
                parent: None,
                hint: Some(format!(
                    "Add an impl-shaped construct that implements '{}'",
                    node.name
                )),
                span_start: Some(node.span.start),
                span_end: Some(node.span.end),
            });
        }
    }
}
