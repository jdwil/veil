//! IR graph for `.layer` files — language-designer topology (DSL-005).
//!
//! Layers are not package ASTs. We project constructs / statements / sections
//! into the same `IrGraph` shape the viewer already understands.

use crate::diagnostics::{Diagnostic, Severity};
use crate::ir::{EdgeKind, IrGraph, NodeKind};
use crate::layer::parse_layer_file;
use crate::span::Span;

/// Build a navigable IR graph from layer source text.
pub fn build_layer_ir(content: &str, layer_name: &str) -> Result<IrGraph, String> {
    let raw = parse_layer_file(content, layer_name)?;
    let mut g = IrGraph::new();
    let root = g.add_node(
        NodeKind::Solution,
        raw.name.clone(),
        Span::new(0, content.len()),
    );
    g.nodes.last_mut().unwrap().metadata.subkind = Some("Layer".into());
    g.nodes.last_mut().unwrap().metadata.doc = Some(format!("layer {}", layer_name));

    // Group nodes by construct.group
    let mut group_ids: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for c in &raw.constructs {
        let gname = if c.group.is_empty() {
            "general".to_string()
        } else {
            c.group.clone()
        };
        if !group_ids.contains_key(&gname) {
            let gid = g.add_node(NodeKind::Group, gname.clone(), Span::new(0, 0));
            g.nodes.last_mut().unwrap().metadata.parent = Some(root);
            g.nodes.last_mut().unwrap().metadata.subkind = Some("Group".into());
            g.add_edge(root, gid, EdgeKind::Contains);
            group_ids.insert(gname.clone(), gid);
        }
        let parent = group_ids[&gname];
        let nid = g.add_node(NodeKind::TypeDef, c.name.clone(), Span::new(0, 0));
        if let Some(n) = g.nodes.last_mut() {
            n.metadata.parent = Some(parent);
            n.metadata.subkind = Some(c.name.clone());
            n.metadata.properties = vec![
                ("keyword".into(), c.keyword.clone()),
                ("maps_to".into(), c.maps_to.clone()),
                ("shape".into(), c.shape.name().into()),
                ("allowed_in".into(), c.allowed_in.clone()),
            ];
            if !c.desc.is_empty() {
                n.metadata.doc = Some(c.desc.clone());
            }
            if !c.visual.icon.is_empty() {
                n.metadata.annotations.push(format!("icon:{}", c.visual.icon));
            }
        }
        g.add_edge(parent, nid, EdgeKind::Contains);
    }

    if !raw.statements.is_empty() {
        let stmts = g.add_node(NodeKind::Group, "statements".into(), Span::new(0, 0));
        g.nodes.last_mut().unwrap().metadata.parent = Some(root);
        g.add_edge(root, stmts, EdgeKind::Contains);
        for s in &raw.statements {
            let nid = g.add_node(NodeKind::Action, s.keyword.clone(), Span::new(0, 0));
            if let Some(n) = g.nodes.last_mut() {
                n.metadata.parent = Some(stmts);
                n.metadata.subkind = Some("Statement".into());
                n.metadata.properties = vec![
                    ("keyword".into(), s.keyword.clone()),
                    ("maps_to".into(), s.maps_to.clone()),
                ];
            }
            g.add_edge(stmts, nid, EdgeKind::Contains);
        }
    }

    if raw.prompt.is_some() {
        let pid = g.add_node(NodeKind::Group, "prompt".into(), Span::new(0, 0));
        g.nodes.last_mut().unwrap().metadata.parent = Some(root);
        g.nodes.last_mut().unwrap().metadata.subkind = Some("Prompt".into());
        g.add_edge(root, pid, EdgeKind::Contains);
    }

    Ok(g)
}

/// Validate layer source; returns diagnostics (empty if OK).
pub fn check_layer(content: &str, layer_name: &str) -> Vec<Diagnostic> {
    match parse_layer_file(content, layer_name) {
        Ok(raw) => {
            let mut diags = Vec::new();
            // Light advisory checks
            for c in &raw.constructs {
                if c.keyword.is_empty() {
                    diags.push(Diagnostic {
                        severity: Severity::Warning,
                        message: format!("construct '{}' has empty keyword", c.name),
                        node_id: None,
                        node_name: Some(c.name.clone()),
                        code: "layer_empty_kw".into(),
                        constraint: "layer_empty_kw".into(),
                        parent: None,
                        hint: Some("set `kw` on the construct".into()),
                        span_start: None,
                        span_end: None,
                    });
                }
            }
            diags
        }
        Err(e) => vec![Diagnostic {
            severity: Severity::Error,
            message: e,
            node_id: None,
            node_name: Some(layer_name.into()),
            code: "layer_parse".into(),
            constraint: "layer_parse".into(),
            parent: None,
            hint: Some("fix layer syntax (construct / statement / present / prompt)".into()),
            span_start: None,
            span_end: None,
        }],
    }
}

/// Extract prompt section text if present.
pub fn layer_prompt(content: &str, layer_name: &str) -> Option<String> {
    parse_layer_file(content, layer_name)
        .ok()
        .and_then(|r| r.prompt)
}
