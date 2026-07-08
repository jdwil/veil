//! IR diagnostics — detects structural issues in the graph.
//!
//! All analysis is layer-driven. The engine reads constraint declarations
//! from the LayerRegistry and checks the IR graph against them. The engine
//! has NO domain knowledge — it only knows about core shapes and constraint
//! keywords defined in `.layer` files.

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
    /// The constraint that was violated.
    pub constraint: String,
}

#[derive(Debug, Clone, Serialize)]
pub enum Severity {
    Warning,
    Error,
}

/// Analyze the IR graph against constraints declared in the layer registry.
pub fn analyze(graph: &IrGraph, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Find which construct specs declare `requires_implementation`.
    // These are trait-shaped constructs that require an impl-shaped construct
    // targeting them via an Implements edge.
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
    // Collect all node IDs that are targets of Implements edges.
    let implemented_targets: std::collections::HashSet<u64> = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .map(|e| e.to)
        .collect();

    for node in &graph.nodes {
        // Only check Interface (trait-shaped) nodes.
        if node.kind != NodeKind::Interface {
            continue;
        }

        // Look up the construct spec for this node's subkind.
        let subkind = match &node.metadata.subkind {
            Some(sk) => sk,
            None => continue,
        };

        let spec = match registry.construct_by_name(subkind) {
            Some(s) => s,
            None => continue,
        };

        // Check if this spec declares `requires_implementation`.
        let has_constraint = spec.constraints.iter().any(|c| c == "requires_implementation");
        if !has_constraint {
            continue;
        }

        // Now check if this node has an Implements edge targeting it.
        if !implemented_targets.contains(&node.id) {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!(
                    "{} '{}' has no implementation",
                    subkind, node.name
                ),
                node_id: Some(node.id),
                node_name: Some(node.name.clone()),
                constraint: "requires_implementation".to_string(),
            });
        }
    }
}
