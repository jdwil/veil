//! VEIL IR Builder — transforms AST into a graph model for visualization and codegen.
//!
//! Subkind assignment is data-driven: the `CONSTRUCT_SUBKINDS` table provides
//! the mapping from AST construct type to IR subkind string. This replaces
//! hardcoded DDD-specific strings and aligns with layer schema construct names.

use crate::ast::*;
use crate::ir::*;
use crate::span::Span;

// ─── Data-driven subkind registry ─────────────────────────────────────────────
//
// These constants match the construct names defined in .layer schema files.
// They are the single source of truth for subkind strings used in the IR graph.

/// Capitalize a keyword to get its display name (e.g., "val" -> "ValueObject", "svc" -> "DomainService")
fn capitalize(keyword: &str) -> String {
    match keyword {
        "val" => "ValueObject".to_string(),
        "ent" => "Entity".to_string(),
        "agg" => "Aggregate".to_string(),
        "evt" => "Event".to_string(),
        "cmd" => "Command".to_string(),
        "port" => "Port".to_string(),
        "repo" => "Repository".to_string(),
        "adapter" => "Adapter".to_string(),
        "svc" => "DomainService".to_string(),
        "saga" => "Saga".to_string(),
        "orchestrator" => "Orchestrator".to_string(),
        "ctx" => "Context".to_string(),
        other => other.to_string(),
    }
}
/// Build an IR graph from a parsed Solution AST.
pub fn build_ir(solution: &Solution) -> IrGraph {
    let mut builder = IrBuilder::new();
    builder.build_solution(solution);
    builder.resolve_adapter_bindings();
    builder.graph
}

/// Extract the port name from a call label like "call PaymentGateway.create_customer"
fn type_to_display(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Generic(name, args) => {
            let a = args.iter().map(|t| type_to_display(t)).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, a)
        }
        TypeExpr::Result(Some(inner)) => format!("Res!<{}>", type_to_display(inner)),
        TypeExpr::Result(None) => "Res!".to_string(),
        TypeExpr::Optional(inner) => format!("Opt<{}>", type_to_display(inner)),
        TypeExpr::List(inner) => format!("List<{}>", type_to_display(inner)),
        TypeExpr::Map(k, v) => format!("Map<{}, {}>", type_to_display(k), type_to_display(v)),
        TypeExpr::Set(inner) => format!("Set<{}>", type_to_display(inner)),
    }
}

fn binop_to_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn unaryop_to_str(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "!",
        UnaryOp::Neg => "-",
    }
}

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
        Expr::Dispatch(d) => {
            let fields = d.fields.iter().map(|(k, v)| {
                let vs = expr_to_display(v);
                if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
            }).collect::<Vec<_>>().join(", ");
            format!("dispatch {}{{{}}}", d.event_name, fields)
        }
        Expr::Invoke(inv) => {
            let params = inv.params.iter().map(|(k, v)| {
                format!("{}: {}", k, expr_to_display(v))
            }).collect::<Vec<_>>().join(", ");
            if inv.command.is_empty() {
                format!("invoke {}{{{}}}", inv.target, params)
            } else {
                format!("invoke {}.{}{{{}}}", inv.target, inv.command, params)
            }
        }
        Expr::Request(req) => {
            let args = req.args.iter().map(|a| expr_to_display(a)).collect::<Vec<_>>().join(", ");
            if req.method.is_empty() {
                format!("request {}({})", req.port, args)
            } else {
                format!("request {}.{}({})", req.port, req.method, args)
            }
        }
        Expr::Guard(g) => {
            let cond = expr_to_display(&g.condition);
            if let Some(msg) = &g.message {
                format!("guard {}, \"{}\"", cond, msg)
            } else {
                format!("guard {}", cond)
            }
        }
        Expr::Assign(name, rhs) => format!("{} = {}", name, expr_to_display(rhs)),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_display(inner)),
        Expr::BinaryOp(op) => format!("{} {} {}", expr_to_display(&op.left), binop_to_str(&op.op), expr_to_display(&op.right)),
        Expr::UnaryOp(op) => format!("{}{}", unaryop_to_str(&op.op), expr_to_display(&op.expr)),
        Expr::IfExpr(ie) => format!("if {}", expr_to_display(&ie.condition)),
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
        let ctx_id = self.graph.add_node(NodeKind::Module, ctx.name.clone(), ctx.span);
        self.set_parent(ctx_id, parent_id);

        // Detect orchestrator: all items are saga-marked constructs or groups containing only sagas
        let is_orchestrator = !ctx.items.is_empty() && ctx.items.iter().all(|item| {
            match item {
                ContextItem::Construct(c) => c.keyword == "saga" || c.annotations.iter().any(|a| a.name == "__saga"),
                ContextItem::Group(g) => g.items.iter().all(|gi| {
                    matches!(gi, ContextItem::Construct(c) if c.keyword == "saga" || c.annotations.iter().any(|a| a.name == "__saga"))
                }),
            }
        });

        if is_orchestrator {
            self.set_subkind(ctx_id, "Orchestrator");
        } else {
            self.set_subkind(ctx_id, "Context");
        }
        self.graph.add_edge(parent_id, ctx_id, EdgeKind::Contains);

        for item in &ctx.items {
            match item {
                ContextItem::Construct(c) => self.build_construct(c, ctx_id),
                ContextItem::Group(group) => {
                    let group_id = self.graph.add_node(NodeKind::Group, group.name.clone(), group.span);
                    self.set_parent(group_id, ctx_id);
                    self.graph.add_edge(ctx_id, group_id, EdgeKind::Contains);
                    for gi in &group.items {
                        match gi {
                            ContextItem::Construct(c) => self.build_construct(c, group_id),
                            ContextItem::Group(_) => {} // nested groups not supported yet
                        }
                    }
                }
            }
        }
    }

    /// Build any construct generically — dispatches based on keyword/category.
    fn build_construct(&mut self, c: &Construct, parent_id: NodeId) {
        // Determine NodeKind and subkind from the keyword
        let (node_kind, subkind) = match c.keyword.as_str() {
            "val" | "ent" | "evt" | "cmd" => (NodeKind::TypeDef, capitalize(&c.keyword)),
            "agg" => (NodeKind::TypeDef, "Aggregate".to_string()),
            "port" | "repo" => (NodeKind::Interface, capitalize(&c.keyword)),
            "adapter" => (NodeKind::Implementation, "Adapter".to_string()),
            "svc" | "saga" => (NodeKind::Flow, capitalize(&c.keyword)),
            _ => (NodeKind::TypeDef, c.keyword.clone()),
        };

        match c.keyword.as_str() {
            // Struct-like: val, ent, evt, cmd
            "val" | "ent" | "evt" | "cmd" => {
                let id = self.graph.add_node(node_kind, c.name.clone(), c.span);
                self.set_parent(id, parent_id);
                self.set_subkind(id, &subkind);
                let fields_str = c.fields.iter()
                    .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                    .collect::<Vec<_>>().join(", ");
                if !fields_str.is_empty() {
                    self.set_property(id, "fields", &fields_str);
                }
                self.graph.add_edge(parent_id, id, EdgeKind::Contains);
            }
            // Aggregate — struct-like with sub-constructs
            "agg" => {
                let agg_id = self.graph.add_node(node_kind, c.name.clone(), c.span);
                self.set_parent(agg_id, parent_id);
                self.set_subkind(agg_id, &subkind);
                self.graph.add_edge(parent_id, agg_id, EdgeKind::Contains);
                // Root fields
                let fields_str = c.fields.iter()
                    .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                    .collect::<Vec<_>>().join(", ");
                if !fields_str.is_empty() {
                    self.set_property(agg_id, "fields", &fields_str);
                }
                // Events and commands as sub-nodes
                for sub in &c.sub_constructs {
                    let sub_kind_label = capitalize(&sub.keyword);
                    let sub_id = self.graph.add_node(NodeKind::TypeDef, sub.name.clone(), sub.span);
                    self.set_parent(sub_id, agg_id);
                    self.set_subkind(sub_id, &sub_kind_label);
                    let sub_fields = sub.fields.iter()
                        .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                        .collect::<Vec<_>>().join(", ");
                    if !sub_fields.is_empty() {
                        self.set_property(sub_id, "fields", &sub_fields);
                    }
                    self.graph.add_edge(agg_id, sub_id, EdgeKind::Contains);
                }
            }
            // Trait-like: port, repo
            "port" | "repo" => {
                let port_id = self.graph.add_node(node_kind, c.name.clone(), c.span);
                self.set_parent(port_id, parent_id);
                self.set_subkind(port_id, &subkind);
                self.graph.add_edge(parent_id, port_id, EdgeKind::Contains);
                let methods_str = c.methods.iter()
                    .map(|m| {
                        let params = m.params.iter()
                            .map(|p| format!("{}: {}", p.name, type_to_display(&p.type_expr)))
                            .collect::<Vec<_>>().join(", ");
                        let ret = m.return_type.as_ref()
                            .map(|t| format!(" -> {}", type_to_display(t)))
                            .unwrap_or_default();
                        format!("{}({}){}", m.name, params, ret)
                    })
                    .collect::<Vec<_>>().join("; ");
                if !methods_str.is_empty() {
                    self.set_property(port_id, "methods", &methods_str);
                }
            }
            // Impl-like: adapter
            "adapter" => {
                let adapter_id = self.graph.add_node(node_kind, c.name.clone(), c.span);
                self.set_parent(adapter_id, parent_id);
                self.set_subkind(adapter_id, &subkind);
                if let Some(target) = &c.target {
                    self.set_property(adapter_id, "implements", target);
                }
                self.graph.add_edge(parent_id, adapter_id, EdgeKind::Contains);
            }
            // Fn-like: svc, saga
            "svc" | "saga" => {
                let flow_id = self.graph.add_node(node_kind, c.name.clone(), c.span);
                self.set_parent(flow_id, parent_id);
                // Check if saga via __saga annotation
                if c.annotations.iter().any(|a| a.name == "__saga") {
                    self.set_subkind(flow_id, "Saga");
                    if let Some(saga_ann) = c.annotations.iter().find(|a| a.name == "__saga") {
                        if !saga_ann.args.is_empty() {
                            self.set_property(flow_id, "contexts", &saga_ann.args.join(", "));
                        }
                    }
                } else {
                    self.set_subkind(flow_id, &subkind);
                }
                self.graph.add_edge(parent_id, flow_id, EdgeKind::Contains);
                // Build inputs
                if !c.inputs.is_empty() {
                    let inputs_str = c.inputs.iter()
                        .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                        .collect::<Vec<_>>().join(", ");
                    let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), c.span);
                    self.set_parent(inputs_id, flow_id);
                    self.set_property(inputs_id, "params", &inputs_str);
                    self.graph.add_edge(flow_id, inputs_id, EdgeKind::Contains);
                }
                // Build steps
                let mut prev_step_id: Option<NodeId> = None;
                for step in &c.steps {
                    if let FlowStep::Step(s) = step {
                        let step_id = self.graph.add_node(NodeKind::Step, s.name.clone(), s.span);
                        self.set_parent(step_id, flow_id);
                        self.graph.add_edge(flow_id, step_id, EdgeKind::Contains);
                        if let Some(prev) = prev_step_id {
                            self.graph.add_edge(prev, step_id, EdgeKind::SequenceFlow);
                        }
                        self.build_step_body(&s.body, step_id);
                        prev_step_id = Some(step_id);
                    }
                }
            }
            // Unknown keyword — generic node
            _ => {
                let id = self.graph.add_node(NodeKind::TypeDef, c.name.clone(), c.span);
                self.set_parent(id, parent_id);
                self.set_subkind(id, &c.keyword);
                self.graph.add_edge(parent_id, id, EdgeKind::Contains);
            }
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

        // Build inputs node
        if !flow.inputs.is_empty() {
            let inputs_str = flow.inputs.iter()
                .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                .collect::<Vec<_>>().join(", ");
            let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), flow.span);
            self.set_parent(inputs_id, flow_id);
            self.set_property(inputs_id, "params", &inputs_str);
            self.graph.add_edge(flow_id, inputs_id, EdgeKind::Contains);
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
                    let fields_str = emit.fields.iter().map(|(name, val)| {
                        let val_str = expr_to_display(val);
                        if name == &val_str { name.clone() } else { format!("{}: {}", name, val_str) }
                    }).collect::<Vec<_>>().join(", ");
                    if !fields_str.is_empty() {
                        self.set_property(id, "fields", &format!("{{{}}}", fields_str));
                    }
                    Some(id)
                }
                Expr::Dispatch(d) => {
                    let label = format!("dispatch {}", d.event_name);
                    let id = self.graph.add_node(NodeKind::DispatchAction, label, d.span);
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    let fields_str = d.fields.iter().map(|(name, val)| {
                        let val_str = expr_to_display(val);
                        if name == &val_str { name.clone() } else { format!("{}: {}", name, val_str) }
                    }).collect::<Vec<_>>().join(", ");
                    if !fields_str.is_empty() {
                        self.set_property(id, "fields", &format!("{{{}}}", fields_str));
                    }
                    Some(id)
                }
                Expr::Invoke(inv) => {
                    let label = if inv.command.is_empty() {
                        format!("invoke {}", inv.target)
                    } else {
                        format!("invoke {}.{}", inv.target, inv.command)
                    };
                    let id = self.graph.add_node(NodeKind::InvokeAction, label, inv.span);
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    let params_str = inv.params.iter().map(|(k, v)| {
                        format!("{}: {}", k, expr_to_display(v))
                    }).collect::<Vec<_>>().join(", ");
                    if !params_str.is_empty() {
                        self.set_property(id, "params", &format!("{{{}}}", params_str));
                    }
                    Some(id)
                }
                Expr::Request(req) => {
                    let label = if req.method.is_empty() {
                        format!("request {}", req.port)
                    } else {
                        format!("request {}.{}", req.port, req.method)
                    };
                    let id = self.graph.add_node(NodeKind::RequestAction, label, req.span);
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    let args_str = req.args.iter().map(|a| expr_to_display(a)).collect::<Vec<_>>().join(", ");
                    if !args_str.is_empty() {
                        self.set_property(id, "args", &format!("({})", args_str));
                    }
                    self.annotate_adapter_binding(id, &req.port);
                    Some(id)
                }
                Expr::Guard(g) => {
                    let label = format!("guard {}", expr_to_display(&g.condition));
                    let id = self.graph.add_node(NodeKind::GuardAction, label, g.span);
                    self.set_parent(id, step_id);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    if let Some(msg) = &g.message {
                        self.set_property(id, "message", msg);
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
                            n.kind == NodeKind::Interface && n.name == call.target
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

    fn set_subkind(&mut self, node_id: NodeId, subkind: &str) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == node_id) {
            node.metadata.subkind = Some(subkind.to_string());
        }
    }

    #[allow(dead_code)]
    fn set_doc(&mut self, node_id: NodeId, doc: &str) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == node_id) {
            node.metadata.doc = Some(doc.to_string());
        }
    }

    /// Find which adapter implements the given port and annotate the node.
    fn annotate_adapter_binding(&mut self, node_id: NodeId, port_name: &str) {
        // Find the port node
        let port_id = self.graph.nodes.iter()
            .find(|n| n.kind == NodeKind::Interface && n.name == port_name)
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
        self.set_subkind(saga_id, "Saga");
        self.graph.add_edge(parent_id, saga_id, EdgeKind::Contains);

        // Add annotations
        for ann in &saga.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == saga_id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }

        // Build inputs node for the saga
        if !saga.inputs.is_empty() {
            let inputs_str = saga.inputs.iter()
                .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                .collect::<Vec<_>>().join(", ");
            let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), saga.span);
            self.set_parent(inputs_id, saga_id);
            self.set_property(inputs_id, "params", &inputs_str);
            self.graph.add_edge(saga_id, inputs_id, EdgeKind::Contains);
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
            NodeKind::Implementation,
            adapter.name.clone(),
            adapter.span,
        );
        self.set_parent(adapter_id, parent_id);
        self.set_subkind(adapter_id, "Adapter");
        self.graph.add_edge(parent_id, adapter_id, EdgeKind::Contains);

        // Find port node and add implements edge
        let port_name = &adapter.target_port;
        if let Some(port_node) = self.graph.nodes.iter().find(|n| {
            n.kind == NodeKind::Interface && n.name == *port_name
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
