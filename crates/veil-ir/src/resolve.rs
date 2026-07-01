//! Package resolution — finds and parses imported packages,
//! filters IR to only expose what packages declare.

use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::ir::*;
use crate::span::Span;

/// Resolved package info with expose-filtered nodes.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub name: String,
    pub alias: Option<String>,
    pub exposed_nodes: Vec<ExposedNode>,
    pub constraints: Vec<String>,
}

/// Resolve all `use` imports for a composition, looking in the given search paths.
/// Returns file paths to parse — actual parsing is done by the caller.
pub fn find_package_files(
    imports: &[UseImport],
    search_paths: &[PathBuf],
) -> Vec<Result<(UseImport, PathBuf), String>> {
    imports
        .iter()
        .map(|imp| {
            let filename = format!("{}.veil", imp.package_name);
            for dir in search_paths {
                let candidate = dir.join(&filename);
                if candidate.exists() {
                    return Ok((imp.clone(), candidate));
                }
            }
            Err(format!(
                "Package '{}' not found in search paths: {:?}",
                imp.package_name, search_paths
            ))
        })
        .collect()
}

/// Build a ResolvedPackage from a parsed Package AST.
pub fn resolve_package(pkg: &Package, alias: Option<String>) -> ResolvedPackage {
    let expose = pkg.expose.clone().unwrap_or(ExposeBlock {
        span: Span::new(0, 0),
        nodes: Vec::new(),
        constraints: Vec::new(),
    });
    ResolvedPackage {
        name: pkg.name.clone(),
        alias,
        exposed_nodes: expose.nodes,
        constraints: expose.constraints,
    }
}

/// Build an IR graph for a composition, showing only exposed nodes as the palette.
pub fn build_composition_ir(
    composition: &Composition,
    resolved_packages: &[ResolvedPackage],
) -> IrGraph {
    let mut graph = IrGraph::new();

    // Add a root "Composition" node
    let root_id = graph.add_node(
        NodeKind::Solution,
        "Composition".to_string(),
        composition.span,
    );

    // Add exposed nodes from each package as a palette group
    for pkg in resolved_packages {
        let display_name = pkg.alias.as_deref().unwrap_or(&pkg.name);
        let pkg_node_id = graph.add_node(
            NodeKind::Context, // Reuse Context as "package group"
            display_name.to_string(),
            Span::new(0, 0),
        );
        graph.nodes.last_mut().unwrap().metadata.parent = Some(root_id);
        graph.nodes.last_mut().unwrap().metadata.annotations.push("📦 package".to_string());
        graph.add_edge(root_id, pkg_node_id, EdgeKind::Contains);

        for exposed in &pkg.exposed_nodes {
            let node_id = graph.add_node(
                NodeKind::Step, // Exposed nodes appear as "action" steps
                exposed.name.clone(),
                exposed.span,
            );
            if let Some(node) = graph.nodes.last_mut() {
                node.metadata.parent = Some(pkg_node_id);
                if let Some(desc) = &exposed.description {
                    node.metadata.annotations.push(desc.clone());
                }
                // Add input/output info as properties
                for input in &exposed.inputs {
                    node.metadata.properties.push((
                        format!("input:{}", input.name),
                        type_to_display(&input.type_expr),
                    ));
                }
                for output in &exposed.outputs {
                    node.metadata.properties.push((
                        format!("output:{}", output.name),
                        type_to_display(&output.type_expr),
                    ));
                }
            }
            graph.add_edge(pkg_node_id, node_id, EdgeKind::Contains);
        }
    }

    // Add flows from the composition
    for flow in &composition.flows {
        let flow_id = graph.add_node(NodeKind::Flow, flow.name.clone(), flow.span);
        graph.nodes.last_mut().unwrap().metadata.parent = Some(root_id);
        graph.add_edge(root_id, flow_id, EdgeKind::Contains);

        // Add flow steps
        let mut prev_step_id: Option<NodeId> = None;
        for step in &flow.steps {
            match step {
                FlowStep::Step(s) => {
                    let step_id = graph.add_node(NodeKind::Step, s.name.clone(), s.span);
                    graph.nodes.last_mut().unwrap().metadata.parent = Some(flow_id);
                    graph.add_edge(flow_id, step_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        graph.add_edge(prev, step_id, EdgeKind::SequenceFlow);
                    }
                    prev_step_id = Some(step_id);
                }
                FlowStep::Parallel(par) => {
                    let par_id = graph.add_node(
                        NodeKind::ParallelGateway,
                        "parallel".to_string(),
                        par.span,
                    );
                    graph.nodes.last_mut().unwrap().metadata.parent = Some(flow_id);
                    graph.add_edge(flow_id, par_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        graph.add_edge(prev, par_id, EdgeKind::SequenceFlow);
                    }
                    for s in &par.steps {
                        let sub_id = graph.add_node(NodeKind::Step, s.name.clone(), s.span);
                        graph.nodes.last_mut().unwrap().metadata.parent = Some(par_id);
                        graph.add_edge(par_id, sub_id, EdgeKind::Contains);
                    }
                    prev_step_id = Some(par_id);
                }
                FlowStep::Match(_) => {}
            }
        }
    }

    graph
}

fn type_to_display(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Result(Some(inner)) => format!("Result<{}>", type_to_display(inner)),
        TypeExpr::Result(None) => "Result<()>".to_string(),
        TypeExpr::Optional(inner) => format!("Option<{}>", type_to_display(inner)),
        TypeExpr::List(inner) => format!("Vec<{}>", type_to_display(inner)),
        TypeExpr::Generic(name, args) => {
            let a = args.iter().map(|a| type_to_display(a)).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, a)
        }
        _ => "?".to_string(),
    }
}
