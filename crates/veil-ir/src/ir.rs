//! VEIL IR Graph model — nodes and edges for visualization and codegen.

use serde::{Deserialize, Serialize};

use crate::span::Span;

/// Unique identifier for an IR node.
pub type NodeId = u64;

/// The IR graph — a collection of nodes and edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrGraph {
    pub nodes: Vec<IrNode>,
    pub edges: Vec<IrEdge>,
    pub next_id: NodeId,
}

impl IrGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            next_id: 1,
        }
    }

    pub fn add_node(&mut self, kind: NodeKind, name: String, span: Span) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(IrNode {
            id,
            kind,
            name,
            span,
            metadata: Default::default(),
        });
        id
    }

    pub fn add_edge(&mut self, from: NodeId, to: NodeId, kind: EdgeKind) {
        self.edges.push(IrEdge { from, to, kind });
    }
}

impl Default for IrGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// A node in the IR graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub span: Span,
    pub metadata: NodeMetadata,
}

/// Visual and semantic metadata for a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeMetadata {
    pub parent: Option<NodeId>,
    pub annotations: Vec<String>,
    pub properties: Vec<(String, String)>,
    /// Package-defined subkind (e.g., "Aggregate", "ValueObject", "Context")
    pub subkind: Option<String>,
    /// Doc comments attached to this node (preserved from source).
    pub doc: Option<String>,
}

/// The kind/type of an IR node. These correspond to core language shapes
/// only — all domain flavor lives in `metadata.subkind`, which is stamped
/// from the layer registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Solution,
    Module,
    Group,
    Inputs,
    Field,
    Return,
    TypeDef,
    Interface,
    InterfaceMethod,
    Implementation,
    Flow,
    Step,
    ParallelGateway,
    ErrorBoundary,
    /// A statement inside a step body. `metadata.subkind` holds the
    /// statement keyword ("call", "assign", or a layer keyword like "dispatch").
    Action,
    MatchDecision,
    MatchArm,
}

/// An edge in the IR graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrEdge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}

/// The kind/type of an IR edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EdgeKind {
    Contains,
    SequenceFlow,
    Calls,
    Emits,
    Implements,
    References,
}
