//! VEIL IR Builder — transforms AST into a graph model for visualization and codegen.

use crate::ast::*;
use crate::ir::*;
use crate::span::Span;
/// Build an IR graph from a parsed Solution AST.
pub fn build_ir(solution: &Solution) -> IrGraph {
    let mut builder = IrBuilder::new();
    builder.build_solution(solution);
    builder.resolve_adapter_bindings();
    builder.graph
}

/// Extract the port name from a call label like "call PaymentGateway.create_customer"
/// Format an annotation for IR metadata, preserving args.
fn annotation_to_ir_string(ann: &Annotation) -> String {
    if ann.args.is_empty() {
        format!("@{}", ann.name)
    } else {
        format!("@{}({})", ann.name, ann.args.join(", "))
    }
}

/// or "c = CustomerRepo.save(...)".
fn extract_port_from_label(label: &str) -> String {
    let s = label.strip_prefix("call ").unwrap_or(label);
    // Handle "name = Target.method(...)" format
    let s = if let Some(idx) = s.find(" = ") {
        &s[idx + 3..]
    } else {
        s
    };
    // Get the target (before the first dot or paren)
    let s = s.split('.').next().unwrap_or(s);
    let s = s.split('(').next().unwrap_or(s);
    s.to_string()
}

/// Render an expression as a human-readable display string.
fn expr_to_display(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::FieldAccess(base, field) => format!("{}.{}", expr_to_display(base), field),
        Expr::Call(call) => {
            let args = call.args.iter().map(|a| expr_to_display(a)).collect::<Vec<_>>().join(", ");
            if call.method.is_empty() {
                format!("{}({})", call.target, args)
            } else {
                format!("{}.{}({})", call.target, call.method, args)
            }
        }
        Expr::Emit(emit) => {
            let fields = emit.fields.iter().map(|(k, v)| {
                let vs = expr_to_display(v);
                if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
            }).collect::<Vec<_>>().join(", ");
            format!("emit {}{{{}}}", emit.event_name, fields)
        }
        Expr::Assign(name, rhs) => format!("{} = {}", name, expr_to_display(rhs)),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_display(inner)),
    }
}

struct IrBuilder {
    graph: IrGraph,
}

impl IrBuilder {
    fn new() -> Self {
        Self {
            graph: IrGraph::new(),
        }
    }

    fn build_solution(&mut self, sol: &Solution) {
        let sol_id = self.graph.add_node(NodeKind::Solution, sol.name.clone(), sol.span);

        for item in &sol.items {
            match item {
                TopLevelItem::Lang(_) => {
                    // Lang blocks are metadata, not visualized as nodes
                }
                TopLevelItem::Context(ctx) => {
                    self.build_context(ctx, sol_id);
                }
                TopLevelItem::Flow(flow) => {
                    self.build_flow(flow, sol_id);
                }
                TopLevelItem::Adapter(adapter) => {
                    self.build_adapter(adapter, sol_id);
                }
                TopLevelItem::Saga(saga) => {
                    self.build_saga(saga, sol_id);
                }
            }
        }
    }

    fn build_context(&mut self, ctx: &Context, parent_id: NodeId) {
        let ctx_id = self.graph.add_node(NodeKind::Context, ctx.name.clone(), ctx.span);
        self.set_parent(ctx_id, parent_id);
        self.graph.add_edge(parent_id, ctx_id, EdgeKind::Contains);

        for item in &ctx.items {
            match item {
                ContextItem::ValueObject(vo) => {
                    let vo_id = self.graph.add_node(
                        NodeKind::ValueObject,
                        vo.name.clone(),
                        vo.span,
                    );
                    self.set_parent(vo_id, ctx_id);
                    self.graph.add_edge(ctx_id, vo_id, EdgeKind::Contains);
                }
                ContextItem::Entity(ent) => {
                    let ent_id = self.graph.add_node(
                        NodeKind::Entity,
                        ent.name.clone(),
                        ent.span,
                    );
                    self.set_parent(ent_id, ctx_id);
                    self.graph.add_edge(ctx_id, ent_id, EdgeKind::Contains);
                }
                ContextItem::Aggregate(agg) => {
                    self.build_aggregate(agg, ctx_id);
                }
                ContextItem::Port(port) => {
                    self.build_port(port, ctx_id);
                }
                ContextItem::Service(svc) => {
                    let svc_id = self.graph.add_node(
                        NodeKind::Service,
                        svc.name.clone(),
                        svc.span,
                    );
                    self.set_parent(svc_id, ctx_id);
                    self.graph.add_edge(ctx_id, svc_id, EdgeKind::Contains);
                }
            }
        }
    }

    fn build_aggregate(&mut self, agg: &Aggregate, parent_id: NodeId) {
        let agg_id = self.graph.add_node(NodeKind::Aggregate, agg.name.clone(), agg.span);
        self.set_parent(agg_id, parent_id);
        self.graph.add_edge(parent_id, agg_id, EdgeKind::Contains);

        // Add annotations as metadata
        for ann in &agg.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == agg_id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }

        // Events
        for evt in &agg.events {
            let evt_id = self.graph.add_node(NodeKind::Event, evt.name.clone(), evt.span);
            self.set_parent(evt_id, agg_id);
            self.graph.add_edge(agg_id, evt_id, EdgeKind::Contains);
        }

        // Commands
        for cmd in &agg.commands {
            let cmd_id = self.graph.add_node(NodeKind::Command, cmd.name.clone(), cmd.span);
            self.set_parent(cmd_id, agg_id);
            self.graph.add_edge(agg_id, cmd_id, EdgeKind::Contains);
        }
    }

    fn build_port(&mut self, port: &Port, parent_id: NodeId) {
        let port_id = self.graph.add_node(NodeKind::Port, port.name.clone(), port.span);
        self.set_parent(port_id, parent_id);
        self.graph.add_edge(parent_id, port_id, EdgeKind::Contains);

        for method in &port.methods {
            let method_id = self.graph.add_node(
                NodeKind::PortMethod,
                method.name.clone(),
                method.span,
            );
            self.set_parent(method_id, port_id);
            self.graph.add_edge(port_id, method_id, EdgeKind::Contains);
        }
    }

    fn build_flow(&mut self, flow: &Flow, parent_id: NodeId) {
        let flow_id = self.graph.add_node(NodeKind::Flow, flow.name.clone(), flow.span);
        self.set_parent(flow_id, parent_id);
        self.graph.add_edge(parent_id, flow_id, EdgeKind::Contains);

        // Add annotations
        for ann in &flow.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == flow_id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }

        // Error boundary
        if let Some(eb) = &flow.error_boundary {
            let eb_id = self.graph.add_node(
                NodeKind::ErrorBoundary,
                "error_boundary".to_string(),
                eb.span,
            );
            self.set_parent(eb_id, flow_id);
            self.graph.add_edge(flow_id, eb_id, EdgeKind::Contains);
        }

        // Steps
        let mut prev_step_id: Option<NodeId> = None;
        for step in &flow.steps {
            match step {
                FlowStep::Step(s) => {
                    let step_id = self.graph.add_node(
                        NodeKind::Step,
                        s.name.clone(),
                        s.span,
                    );
                    self.set_parent(step_id, flow_id);
                    self.graph.add_edge(flow_id, step_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        self.graph.add_edge(prev, step_id, EdgeKind::SequenceFlow);
                    }
                    // Add body expressions as child nodes
                    self.build_step_body(&s.body, step_id);
                    prev_step_id = Some(step_id);
                }
                FlowStep::Parallel(par) => {
                    let par_id = self.graph.add_node(
                        NodeKind::ParallelGateway,
                        "parallel".to_string(),
                        par.span,
                    );
                    self.set_parent(par_id, flow_id);
                    self.graph.add_edge(flow_id, par_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        self.graph.add_edge(prev, par_id, EdgeKind::SequenceFlow);
                    }
                    for s in &par.steps {
                        let sub_id = self.graph.add_node(
                            NodeKind::Step,
                            s.name.clone(),
                            s.span,
                        );
                        self.set_parent(sub_id, par_id);
                        self.graph.add_edge(par_id, sub_id, EdgeKind::Contains);
                        self.build_step_body(&s.body, sub_id);
                    }
                    prev_step_id = Some(par_id);
                }
                FlowStep::Match(_) => {
                    // TODO: match blocks as decision nodes
                }
            }
        }
    }

    fn build_step_body(&mut self, body: &[Expr], step_id: NodeId) {
        let mut prev_action: Option<NodeId> = None;
        for expr in body {
            let action_id = match expr {
                Expr::Call(call) => {
                    let label = if call.method.is_empty() {
                        format!("call {}", call.target)
                    } else {
                        format!("call {}.{}", call.target, call.method)
                    };
                    let id = self.graph.add_node(NodeKind::CallAction, label, call.span);
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    // Add args as properties
                    let args_str = call.args.iter().map(|a| expr_to_display(a)).collect::<Vec<_>>().join(", ");
                    if !args_str.is_empty() {
                        self.set_property(id, "args", &args_str);
                    }
                    // Resolve adapter for this port
                    self.annotate_adapter_binding(id, &call.target);
                    Some(id)
                }
                Expr::Emit(emit) => {
                    let label = format!("emit {}", emit.event_name);
                    let id = self.graph.add_node(NodeKind::EmitAction, label, emit.span);
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    // Add fields as properties
                    let fields_str = emit.fields.iter().map(|(name, val)| {
                        let val_str = expr_to_display(val);
                        if name == &val_str {
                            name.clone()
                        } else {
                            format!("{}: {}", name, val_str)
                        }
                    }).collect::<Vec<_>>().join(", ");
                    if !fields_str.is_empty() {
                        self.set_property(id, "fields", &format!("{{{}}}", fields_str));
                    }
                    Some(id)
                }
                Expr::Assign(name, rhs) => {
                    let rhs_display = expr_to_display(rhs);
                    let label = format!("{} = {}", name, rhs_display);
                    let id = self.graph.add_node(NodeKind::AssignAction, label.clone(), Span::new(0, 0));
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);

                    // If RHS is a call, add args detail
                    if let Expr::Call(call) = rhs.as_ref() {
                        let args_str = call.args.iter().map(|a| expr_to_display(a)).collect::<Vec<_>>().join(", ");
                        if !args_str.is_empty() {
                            self.set_property(id, "args", &format!("({})", args_str));
                        }
                        // Resolve adapter for this port
                        self.annotate_adapter_binding(id, &call.target);
                        // Add Calls edge to port if visible
                        if let Some(port_node) = self.graph.nodes.iter().find(|n| {
                            n.kind == NodeKind::Port && n.name == call.target
                        }) {
                            let port_id = port_node.id;
                            self.graph.add_edge(id, port_id, EdgeKind::Calls);
                        }
                    }
                    Some(id)
                }
                _ => None,
            };
            // Link sequentially within the step body
            if let (Some(prev), Some(curr)) = (prev_action, action_id) {
                self.graph.add_edge(prev, curr, EdgeKind::SequenceFlow);
            }
            if action_id.is_some() {
                prev_action = action_id;
            }
        }
    }

    fn set_property(&mut self, node_id: NodeId, key: &str, value: &str) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == node_id) {
            node.metadata.properties.push((key.to_string(), value.to_string()));
        }
    }

    /// Find which adapter implements the given port and annotate the node.
    fn annotate_adapter_binding(&mut self, node_id: NodeId, port_name: &str) {
        // Find the port node
        let port_id = self.graph.nodes.iter()
            .find(|n| n.kind == NodeKind::Port && n.name == port_name)
            .map(|n| n.id);

        if let Some(port_id) = port_id {
            // Find adapter that has an Implements edge to this port
            let adapter_name = self.graph.edges.iter()
                .find(|e| e.to == port_id && e.kind == EdgeKind::Implements)
                .and_then(|e| self.graph.nodes.iter().find(|n| n.id == e.from))
                .map(|n| n.name.clone());

            if let Some(adapter) = adapter_name {
                self.set_property(node_id, "via", &adapter);
            }
        }
    }

    /// Post-processing pass: annotate all CallAction/AssignAction nodes
    /// with which adapter implements their target port.
    fn resolve_adapter_bindings(&mut self) {
        // Build a map: port_name -> adapter_name
        let mut port_to_adapter: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for edge in &self.graph.edges {
            if edge.kind == EdgeKind::Implements {
                let adapter_name = self.graph.nodes.iter()
                    .find(|n| n.id == edge.from)
                    .map(|n| n.name.clone());
                let port_name = self.graph.nodes.iter()
                    .find(|n| n.id == edge.to)
                    .map(|n| n.name.clone());
                if let (Some(adapter), Some(port)) = (adapter_name, port_name) {
                    port_to_adapter.insert(port, adapter);
                }
            }
        }

        // Annotate call/assign action nodes
        for node in &mut self.graph.nodes {
            if matches!(node.kind, NodeKind::CallAction | NodeKind::AssignAction) {
                // Extract port name from the node name (e.g., "call PaymentGateway.create_customer" -> "PaymentGateway")
                let port_name = extract_port_from_label(&node.name);
                if let Some(adapter) = port_to_adapter.get(&port_name) {
                    node.metadata.properties.push(("via".to_string(), adapter.clone()));
                }
            }
        }
    }

    fn build_saga(&mut self, saga: &Saga, parent_id: NodeId) {
        let saga_id = self.graph.add_node(NodeKind::Saga, saga.name.clone(), saga.span);
        self.set_parent(saga_id, parent_id);
        self.graph.add_edge(parent_id, saga_id, EdgeKind::Contains);

        // Add annotations
        for ann in &saga.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == saga_id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }

        // Add context references as properties
        if !saga.context_refs.is_empty() {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == saga_id) {
                node.metadata.properties.push((
                    "contexts".to_string(),
                    saga.context_refs.join(", "),
                ));
            }
        }

        // Add steps with context associations
        let mut prev_step_id: Option<NodeId> = None;
        for step in &saga.steps {
            let step_id = self.graph.add_node(NodeKind::Step, step.name.clone(), step.span);
            self.set_parent(step_id, saga_id);
            self.graph.add_edge(saga_id, step_id, EdgeKind::Contains);

            // Mark which context this step belongs to
            if let Some(ctx_name) = &step.context {
                if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == step_id) {
                    node.metadata.properties.push(("ctx".to_string(), ctx_name.clone()));
                }
            }

            // Mark if it has compensation
            if !step.compensate.is_empty() {
                if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == step_id) {
                    node.metadata.annotations.push("has_compensate".to_string());
                }
            }

            if let Some(prev) = prev_step_id {
                self.graph.add_edge(prev, step_id, EdgeKind::SequenceFlow);
            }
            self.build_step_body(&step.body, step_id);
            prev_step_id = Some(step_id);
        }
    }

    fn build_adapter(&mut self, adapter: &Adapter, parent_id: NodeId) {
        let adapter_id = self.graph.add_node(
            NodeKind::Adapter,
            adapter.name.clone(),
            adapter.span,
        );
        self.set_parent(adapter_id, parent_id);
        self.graph.add_edge(parent_id, adapter_id, EdgeKind::Contains);

        // Find port node and add implements edge
        let port_name = &adapter.target_port;
        if let Some(port_node) = self.graph.nodes.iter().find(|n| {
            n.kind == NodeKind::Port && n.name == *port_name
        }) {
            let port_id = port_node.id;
            self.graph.add_edge(adapter_id, port_id, EdgeKind::Implements);
        }

        // Add annotations
        for ann in &adapter.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == adapter_id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }
    }

    fn set_parent(&mut self, child_id: NodeId, parent_id: NodeId) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == child_id) {
            node.metadata.parent = Some(parent_id);
        }
    }
}
