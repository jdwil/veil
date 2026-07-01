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
}

/// The kind/type of an IR node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    Solution,
    Context,
    Aggregate,
    Entity,
    ValueObject,
    Event,
    Command,
    Query,
    Port,
    PortMethod,
    Adapter,
    Flow,
    Saga,
    Step,
    ParallelGateway,
    ErrorBoundary,
    Service,
    // Step body elements (drillable)
    CallAction,
    EmitAction,
    AssignAction,
    MatchDecision,
    MatchArm,
    DispatchAction,
    InvokeAction,
    RequestAction,
    GuardAction,
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
