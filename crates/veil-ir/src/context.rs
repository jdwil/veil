//! Agent / IDE context pack (LAY-010).
//!
//! Compact topology + presentation summary so agents describe structure the
//! way humans see it (views, not only flat IR).

use serde::{Deserialize, Serialize};

use crate::ir::{IrGraph, NodeKind};
use crate::layer::LayerRegistry;
use crate::presentation::{presentation_from_registry, PresentationModel};
use crate::project::{
    project_view_with_edges, ProjectInputNode, ProjectOutput,
};
use crate::presentation::ViewSpec;

/// Query params for context assembly.
#[derive(Debug, Clone, Default)]
pub struct ContextQuery {
    /// Optional host IR node id to expand with active-view projection.
    pub host_id: Option<u32>,
    /// Optional view id (default: host default view).
    pub view_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub version: u32,
    /// Loaded layer names (order).
    pub layers: Vec<String>,
    /// Layer prompt texts for RAG (name, text).
    pub layer_prompts: Vec<(String, String)>,
    /// Full presentation model (views / roles / lenses).
    pub presentation: PresentationModel,
    /// Compact topology outline (name, kind, subkind, id, parent).
    pub outline: Vec<OutlineNode>,
    /// Guidance for agents.
    pub agent_hints: Vec<String>,
    /// Optional projection of one host under a presentation view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected: Option<ProjectedContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineNode {
    pub id: u64,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subkind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lenses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectedContext {
    pub host_id: u64,
    pub host_name: String,
    pub host_construct: String,
    pub view_id: String,
    pub view_label: String,
    pub layout: String,
    /// Top-level names in this projection (human-readable).
    pub top_level: Vec<String>,
    /// Nest edges as "Child under Parent".
    pub nest: Vec<String>,
    pub tabs: Vec<String>,
}

/// Build context pack from IR graph + registry.
pub fn build_context_pack(
    graph: &IrGraph,
    registry: &LayerRegistry,
    query: &ContextQuery,
) -> ContextPack {
    let presentation = presentation_from_registry(registry);
    let outline: Vec<OutlineNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Solution)
        .filter(|n| {
            !n.metadata
                .annotations
                .iter()
                .any(|a| a == "layer-provided")
        })
        .map(|n| {
            let sub = n.metadata.subkind.clone();
            let lenses = sub
                .as_ref()
                .and_then(|s| presentation.constructs.get(s))
                .map(|c| c.lenses.clone())
                .unwrap_or_default();
            OutlineNode {
                id: n.id,
                name: n.name.clone(),
                kind: format!("{:?}", n.kind),
                subkind: sub,
                parent: n.metadata.parent,
                lenses,
            }
        })
        .collect();

    let mut agent_hints = vec![
        "Prefer structured EditOp over raw text rewrites when possible.".into(),
        "Describe topology using presentation views when available (e.g. 'Domain model' vs 'Layers').".into(),
        "Construct identity in views uses layer construct names (Aggregate), not keywords (agg).".into(),
    ];

    // Summarize hosts with multiple views
    for (host, h) in &presentation.hosts {
        if h.views.len() > 1 {
            let ids: Vec<_> = h.views.iter().map(|v| v.id.as_str()).collect();
            agent_hints.push(format!(
                "Host '{host}' has views: {} — speak in those terms when reviewing.",
                ids.join(", ")
            ));
        }
    }

    let projected = query.host_id.and_then(|hid| {
        project_host(graph, &presentation, hid, query.view_id.as_deref())
    });

    ContextPack {
        version: 1,
        layers: registry.layers.clone(),
        layer_prompts: registry.prompts.clone(),
        presentation,
        outline,
        agent_hints,
        projected,
    }
}

fn project_host(
    graph: &IrGraph,
    presentation: &PresentationModel,
    host_id: u32,
    view_id: Option<&str>,
) -> Option<ProjectedContext> {
    let host = graph.nodes.iter().find(|n| n.id == host_id as u64)?;
    let construct = host.metadata.subkind.clone().unwrap_or_default();
    let host_views = presentation.hosts.get(&construct)?;
    let view: &ViewSpec = if let Some(vid) = view_id {
        host_views.views.iter().find(|v| v.id == vid)?
    } else if let Some(ref d) = host_views.default_view {
        host_views
            .views
            .iter()
            .find(|v| &v.id == d)
            .or_else(|| host_views.views.first())?
    } else {
        host_views.views.first()?
    };

    let inputs: Vec<ProjectInputNode> = graph
        .nodes
        .iter()
        .map(|n| {
            let fields = n
                .metadata
                .properties
                .iter()
                .find(|(k, _)| k == "fields")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            ProjectInputNode {
                id: n.id as u32,
                parent: n.metadata.parent.map(|p| p as u32),
                name: n.name.clone(),
                construct: n.metadata.subkind.clone().unwrap_or_else(|| format!("{:?}", n.kind)),
                is_group: n.kind == NodeKind::Group,
                layer_provided: n.metadata.annotations.iter().any(|a| a == "layer-provided"),
                fields,
            }
        })
        .collect();

    let edges: Vec<crate::project::ProjectEdge> = graph
        .edges
        .iter()
        .map(|e| crate::project::ProjectEdge {
            from: e.from as u32,
            to: e.to as u32,
            kind: format!("{:?}", e.kind),
        })
        .collect();

    let out: ProjectOutput =
        project_view_with_edges(&inputs, &edges, host_id as u32, view, true);
    let by_id: std::collections::HashMap<u32, &str> = inputs
        .iter()
        .map(|n| (n.id, n.name.as_str()))
        .collect();

    let top_level: Vec<String> = out
        .node_ids
        .iter()
        .filter_map(|id| by_id.get(id).map(|s| s.to_string()))
        .collect();
    let nest: Vec<String> = out
        .nest_edges
        .iter()
        .filter_map(|(c, p)| {
            let cn = by_id.get(c)?;
            let pn = by_id.get(p)?;
            Some(format!("{cn} under {pn}"))
        })
        .collect();

    Some(ProjectedContext {
        host_id: host_id as u64,
        host_name: host.name.clone(),
        host_construct: construct,
        view_id: view.id.clone(),
        view_label: view.label.clone(),
        layout: out.layout,
        top_level,
        nest,
        tabs: out.tabs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::LayerRegistry;

    #[test]
    fn context_pack_includes_presentation_and_hints() {
        let mut reg = LayerRegistry::builtin();
        reg.load_content("ddd", include_str!("../../../layers/ddd.layer"))
            .expect("ddd");
        // Minimal empty graph
        let graph = IrGraph {
            nodes: vec![],
            edges: vec![],
            next_id: 1,
        };
        let pack = build_context_pack(&graph, &reg, &ContextQuery::default());
        assert_eq!(pack.version, 1);
        assert!(pack.presentation.hosts.contains_key("Context"));
        assert!(
            pack.agent_hints.iter().any(|h| h.contains("Domain model") || h.contains("views")),
            "expected view hint: {:?}",
            pack.agent_hints
        );
    }
}
