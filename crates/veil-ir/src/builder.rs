//! VEIL IR Builder — transforms AST into a graph model for visualization and codegen.
//!
//! The builder is fully generic: node kinds come from the construct's core
//! shape and subkinds come from the construct's layer-stamped name. There is
//! no domain vocabulary in this file.

use crate::ast::*;
use crate::ir::*;
use crate::layer::Shape;
use crate::span::Span;

/// Build an IR graph from a parsed Solution AST.
pub fn build_ir(solution: &Solution) -> IrGraph {
    let mut builder = IrBuilder::new();
    builder.build_solution(solution);
    builder.resolve_impl_bindings();
    builder.graph
}

fn type_to_display(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Generic(name, args) => {
            let a = args.iter().map(type_to_display).collect::<Vec<_>>().join(", ");
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

/// Extract the interface name from an action label like
/// "call PaymentGateway.create_customer" or "c = CustomerRepo.save(...)".
fn extract_target_from_label(label: &str) -> String {
    let s = label.split_whitespace().nth(1).unwrap_or(label);
    let s = if let Some(idx) = label.find(" = ") {
        &label[idx + 3..]
    } else {
        s
    };
    let s = s.split('.').next().unwrap_or(s);
    let s = s.split('(').next().unwrap_or(s);
    s.to_string()
}

/// Render an expression as a human-readable display string.
pub fn expr_to_display(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::FieldAccess(base, field) => format!("{}.{}", expr_to_display(base), field),
        Expr::Call(call) => {
            let args = call.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
            if call.method.is_empty() {
                format!("{}({})", call.target, args)
            } else {
                format!("{}.{}({})", call.target, call.method, args)
            }
        }
        Expr::Action(a) => action_to_display(a),
        Expr::Assign(name, rhs) => format!("{} = {}", name, expr_to_display(rhs)),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_display(inner)),
        Expr::BinaryOp(op) => format!(
            "{} {} {}",
            expr_to_display(&op.left),
            binop_to_str(&op.op),
            expr_to_display(&op.right)
        ),
        Expr::UnaryOp(op) => format!("{}{}", unaryop_to_str(&op.op), expr_to_display(&op.expr)),
        Expr::IfExpr(ie) => format!("if {}", expr_to_display(&ie.condition)),
        Expr::StructLit(name, fields) => {
            let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_display(v))).collect::<Vec<_>>().join(", ");
            format!("{}{{{}}}", name, fs)
        }
        Expr::Match(scrutinee, arms) => {
            let arms_str = arms.iter().map(|a| format!("{} -> ...", a.pattern)).collect::<Vec<_>>().join(", ");
            format!("match {} {{ {} }}", expr_to_display(scrutinee), arms_str)
        }
        Expr::ForLoop { binding, iterable, .. } => {
            format!("for {} in {}", binding, expr_to_display(iterable))
        }
        Expr::WhileLoop { condition, .. } => {
            format!("while {}", expr_to_display(condition))
        }
        Expr::Closure { params, body } => {
            let p = params.join(", ");
            format!("|{}| ...", p)
        }
    }
}

/// Render a layer statement as display text: `dispatch Evt{...}`, `guard cond, "msg"`.
pub fn action_to_display(a: &ActionExpr) -> String {
    match a.shape {
        crate::layer::StmtShape::Call => {
            let head = if a.method.is_empty() {
                format!("{} {}", a.keyword, a.target)
            } else {
                format!("{} {}.{}", a.keyword, a.target, a.method)
            };
            if !a.named_args.is_empty() {
                let fields = a
                    .named_args
                    .iter()
                    .map(|(k, v)| {
                        let vs = expr_to_display(v);
                        if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}{{{}}}", head, fields)
            } else if !a.args.is_empty() {
                let args = a.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
                format!("{}({})", head, args)
            } else {
                head
            }
        }
        crate::layer::StmtShape::If => {
            let cond = a
                .condition
                .as_ref()
                .map(|c| expr_to_display(c))
                .unwrap_or_default();
            if let Some(msg) = &a.message {
                format!("{} {}, \"{}\"", a.keyword, cond, msg)
            } else {
                format!("{} {}", a.keyword, cond)
            }
        }
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
                    // Lang blocks are metadata, not visualized as nodes.
                }
                TopLevelItem::Construct(c) => {
                    self.build_construct(c, sol_id);
                }
                TopLevelItem::Flow(flow) => {
                    self.build_flow(flow, sol_id);
                }
            }
        }
    }

    /// Map a construct's core shape to its IR node kind.
    fn node_kind_for(shape: Shape) -> NodeKind {
        match shape {
            Shape::Mod => NodeKind::Module,
            Shape::Group => NodeKind::Group,
            Shape::Struct | Shape::Enum => NodeKind::TypeDef,
            Shape::Trait => NodeKind::Interface,
            Shape::Impl => NodeKind::Implementation,
            Shape::Fn => NodeKind::Flow,
        }
    }

    /// Build any construct generically, dispatching on its core shape.
    fn build_construct(&mut self, c: &Construct, parent_id: NodeId) {
        let kind = Self::node_kind_for(c.shape);
        let id = self.graph.add_node(kind, c.name.clone(), c.span);
        self.set_parent(id, parent_id);
        self.set_subkind(id, &c.subkind);
        self.graph.add_edge(parent_id, id, EdgeKind::Contains);

        for ann in &c.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }

        match c.shape {
            Shape::Mod | Shape::Group => {
                for child in &c.children {
                    self.build_construct(child, id);
                }
            }
            Shape::Struct => {
                // Fields: direct fields plus struct-shaped named blocks (e.g. root).
                let mut all_fields: Vec<&Field> = c.fields.iter().collect();
                for block in &c.blocks {
                    if block.shape != Shape::Enum {
                        all_fields.extend(block.fields.iter());
                    }
                }
                let fields_str = all_fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                    .collect::<Vec<_>>()
                    .join(", ");
                if !fields_str.is_empty() {
                    self.set_property(id, "fields", &fields_str);
                }
                // Enum-shaped named blocks (state machines) as properties.
                for block in &c.blocks {
                    if block.shape == Shape::Enum {
                        let transitions = block
                            .transitions
                            .iter()
                            .map(|t| format!("{} -> {}", t.from, t.to))
                            .collect::<Vec<_>>()
                            .join("; ");
                        let label = block.name.clone().unwrap_or_else(|| block.keyword.clone());
                        self.set_property(id, &format!("{}:{}", block.keyword, label), &transitions);
                    }
                }
                // Nested constructs (events, commands, ...) as child nodes.
                for child in &c.children {
                    self.build_construct(child, id);
                }
                // Business logic fns as properties.
                for f in &c.fns {
                    let params = f
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, type_to_display(&p.type_expr)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret = f
                        .return_type
                        .as_ref()
                        .map(|t| format!(" -> {}", type_to_display(t)))
                        .unwrap_or_default();
                    self.set_property(id, &format!("fn:{}", f.name), &format!("({}){}", params, ret));
                }
            }
            Shape::Enum => {
                if !c.variants.is_empty() {
                    self.set_property(id, "variants", &c.variants.join(", "));
                }
                if !c.transitions.is_empty() {
                    let t = c
                        .transitions
                        .iter()
                        .map(|t| format!("{} -> {}", t.from, t.to))
                        .collect::<Vec<_>>()
                        .join("; ");
                    self.set_property(id, "transitions", &t);
                }
            }
            Shape::Trait => {
                let methods_str = c
                    .methods
                    .iter()
                    .map(|m| {
                        let params = m
                            .params
                            .iter()
                            .map(|p| format!("{}: {}", p.name, type_to_display(&p.type_expr)))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let ret = m
                            .return_type
                            .as_ref()
                            .map(|t| format!(" -> {}", type_to_display(t)))
                            .unwrap_or_default();
                        format!("{}({}){}", m.name, params, ret)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if !methods_str.is_empty() {
                    self.set_property(id, "methods", &methods_str);
                }
            }
            Shape::Impl => {
                if let Some(target) = &c.target {
                    self.set_property(id, "implements", target);
                    // Add Implements edge if the target interface is already built.
                    if let Some(target_node) = self
                        .graph
                        .nodes
                        .iter()
                        .find(|n| n.kind == NodeKind::Interface && n.name == *target)
                    {
                        let target_id = target_node.id;
                        self.graph.add_edge(id, target_id, EdgeKind::Implements);
                    }
                }
            }
            Shape::Fn => {
                // Reference lines (e.g. contexts) as properties.
                for r in &c.refs {
                    self.set_property(id, &r.keyword, &r.values.join(", "));
                }
                if !c.inputs.is_empty() {
                    let inputs_str = c
                        .inputs
                        .iter()
                        .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), c.span);
                    self.set_parent(inputs_id, id);
                    self.set_property(inputs_id, "params", &inputs_str);
                    self.graph.add_edge(id, inputs_id, EdgeKind::Contains);
                }
                self.build_steps(&c.steps, id);
            }
        }
    }

    fn build_flow(&mut self, flow: &Flow, parent_id: NodeId) {
        let flow_id = self.graph.add_node(NodeKind::Flow, flow.name.clone(), flow.span);
        self.set_parent(flow_id, parent_id);
        self.graph.add_edge(parent_id, flow_id, EdgeKind::Contains);

        for ann in &flow.annotations {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == flow_id) {
                node.metadata.annotations.push(annotation_to_ir_string(ann));
            }
        }

        if !flow.inputs.is_empty() {
            let inputs_str = flow
                .inputs
                .iter()
                .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                .collect::<Vec<_>>()
                .join(", ");
            let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), flow.span);
            self.set_parent(inputs_id, flow_id);
            self.set_property(inputs_id, "params", &inputs_str);
            self.graph.add_edge(flow_id, inputs_id, EdgeKind::Contains);
        }

        if let Some(eb) = &flow.error_boundary {
            let eb_id = self.graph.add_node(
                NodeKind::ErrorBoundary,
                "error_boundary".to_string(),
                eb.span,
            );
            self.set_parent(eb_id, flow_id);
            self.graph.add_edge(flow_id, eb_id, EdgeKind::Contains);
        }

        self.build_steps(&flow.steps, flow_id);
    }

    fn build_steps(&mut self, steps: &[FlowStep], parent_id: NodeId) {
        let mut prev_step_id: Option<NodeId> = None;
        for step in steps {
            match step {
                FlowStep::Step(s) => {
                    let step_id = self.graph.add_node(NodeKind::Step, s.name.clone(), s.span);
                    self.set_parent(step_id, parent_id);
                    self.graph.add_edge(parent_id, step_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        self.graph.add_edge(prev, step_id, EdgeKind::SequenceFlow);
                    }
                    // Reference lines within the step (e.g. `ctx Identity`).
                    for r in &s.refs {
                        self.set_property(step_id, &r.keyword, &r.values.join(", "));
                    }
                    // Named sub-blocks (e.g. compensate).
                    for sb in &s.sub_blocks {
                        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == step_id) {
                            node.metadata.annotations.push(format!("has_{}", sb.keyword));
                        }
                    }
                    self.build_step_body(&s.body, step_id);
                    prev_step_id = Some(step_id);
                }
                FlowStep::Parallel(par) => {
                    let par_id = self.graph.add_node(
                        NodeKind::ParallelGateway,
                        "parallel".to_string(),
                        par.span,
                    );
                    self.set_parent(par_id, parent_id);
                    self.graph.add_edge(parent_id, par_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        self.graph.add_edge(prev, par_id, EdgeKind::SequenceFlow);
                    }
                    for s in &par.steps {
                        let sub_id = self.graph.add_node(NodeKind::Step, s.name.clone(), s.span);
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
                    let id = self.graph.add_node(NodeKind::Action, label, call.span);
                    self.set_parent(id, step_id);
                    self.set_subkind(id, "call");
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    let args_str = call.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
                    if !args_str.is_empty() {
                        self.set_property(id, "args", &args_str);
                    }
                    self.annotate_impl_binding(id, &call.target);
                    Some(id)
                }
                Expr::Action(a) => {
                    let label = action_to_display(a);
                    let id = self.graph.add_node(NodeKind::Action, label, a.span);
                    self.set_parent(id, step_id);
                    self.set_subkind(id, &a.keyword);
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);
                    if !a.named_args.is_empty() {
                        let fields_str = a
                            .named_args
                            .iter()
                            .map(|(k, v)| {
                                let vs = expr_to_display(v);
                                if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        self.set_property(id, "fields", &format!("{{{}}}", fields_str));
                    }
                    if !a.args.is_empty() {
                        let args_str = a.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
                        self.set_property(id, "args", &format!("({})", args_str));
                    }
                    if let Some(msg) = &a.message {
                        self.set_property(id, "message", msg);
                    }
                    if !a.target.is_empty() {
                        self.annotate_impl_binding(id, &a.target);
                    }
                    Some(id)
                }
                Expr::Assign(name, rhs) => {
                    let rhs_display = expr_to_display(rhs);
                    let label = format!("{} = {}", name, rhs_display);
                    let id = self.graph.add_node(NodeKind::Action, label, Span::new(0, 0));
                    self.set_parent(id, step_id);
                    self.set_subkind(id, "assign");
                    self.graph.add_edge(step_id, id, EdgeKind::Contains);

                    if let Expr::Call(call) = rhs.as_ref() {
                        let args_str = call.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
                        if !args_str.is_empty() {
                            self.set_property(id, "args", &format!("({})", args_str));
                        }
                        self.annotate_impl_binding(id, &call.target);
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

    fn set_parent(&mut self, child_id: NodeId, parent_id: NodeId) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == child_id) {
            node.metadata.parent = Some(parent_id);
        }
    }

    /// Find which implementation targets the given interface and annotate the node.
    fn annotate_impl_binding(&mut self, node_id: NodeId, target_name: &str) {
        let target_id = self
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Interface && n.name == target_name)
            .map(|n| n.id);

        if let Some(target_id) = target_id {
            let impl_name = self
                .graph
                .edges
                .iter()
                .find(|e| e.to == target_id && e.kind == EdgeKind::Implements)
                .and_then(|e| self.graph.nodes.iter().find(|n| n.id == e.from))
                .map(|n| n.name.clone());

            if let Some(name) = impl_name {
                self.set_property(node_id, "via", &name);
            }
        }
    }

    /// Post-processing pass: connect impl-shaped constructs to their target
    /// interfaces (order-independent) and annotate actions with bindings.
    fn resolve_impl_bindings(&mut self) {
        // First: add any Implements edges that couldn't be resolved during
        // the build because the interface appeared later in the file.
        let mut new_edges = Vec::new();
        for node in &self.graph.nodes {
            if node.kind == NodeKind::Implementation {
                let target = node
                    .metadata
                    .properties
                    .iter()
                    .find(|(k, _)| k == "implements")
                    .map(|(_, v)| v.clone());
                if let Some(target) = target {
                    let already = self
                        .graph
                        .edges
                        .iter()
                        .any(|e| e.from == node.id && e.kind == EdgeKind::Implements);
                    if !already {
                        if let Some(t) = self
                            .graph
                            .nodes
                            .iter()
                            .find(|n| n.kind == NodeKind::Interface && n.name == target)
                        {
                            new_edges.push((node.id, t.id));
                        }
                    }
                }
            }
        }
        for (from, to) in new_edges {
            self.graph.add_edge(from, to, EdgeKind::Implements);
        }

        // Build interface -> implementation map.
        let mut target_to_impl: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for edge in &self.graph.edges {
            if edge.kind == EdgeKind::Implements {
                let impl_name = self.graph.nodes.iter().find(|n| n.id == edge.from).map(|n| n.name.clone());
                let target_name = self.graph.nodes.iter().find(|n| n.id == edge.to).map(|n| n.name.clone());
                if let (Some(i), Some(t)) = (impl_name, target_name) {
                    target_to_impl.insert(t, i);
                }
            }
        }

        for node in &mut self.graph.nodes {
            if node.kind == NodeKind::Action {
                let already = node.metadata.properties.iter().any(|(k, _)| k == "via");
                if already {
                    continue;
                }
                let target = extract_target_from_label(&node.name);
                if let Some(impl_name) = target_to_impl.get(&target) {
                    node.metadata
                        .properties
                        .push(("via".to_string(), impl_name.clone()));
                }
            }
        }
    }
}
