//! VEIL IR Builder — transforms AST into a graph model for visualization and codegen.
//!
//! The builder is fully generic: node kinds come from the construct's core
//! shape and subkinds come from the construct's layer-stamped name. There is
//! no domain vocabulary in this file.

use crate::ast::*;
use crate::ir::*;
use crate::layer::{LayerRegistry, Shape};
use crate::span::Span;

/// Build an IR graph from a parsed Solution AST (no layer policy).
/// Prefer [`build_ir_with_registry`] when DI / dependency roles matter (INV-001).
pub fn build_ir(solution: &Solution) -> IrGraph {
    build_ir_with_registry(solution, None)
}

/// Build IR, using `registry` for layer policy (e.g. dependency annotation roles).
pub fn build_ir_with_registry(solution: &Solution, registry: Option<&LayerRegistry>) -> IrGraph {
    let mut builder = IrBuilder::new(registry);
    builder.build_solution(solution);
    builder.resolve_impl_bindings();
    builder.resolve_references();
    builder.graph
}

/// True if field carries a dependency-role annotation (INV-001).
fn field_is_dep(field: &Field, registry: Option<&LayerRegistry>) -> bool {
    if let Some(reg) = registry {
        return reg.field_is_dependency(field);
    }
    // Without a registry, treat no field as a special dependency (no magic "dep").
    false
}

pub fn type_to_display(ty: &TypeExpr) -> String {
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
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_to_display).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}; {}]", type_to_display(inner), size),
        TypeExpr::Ref(inner, is_mut) => if *is_mut { format!("&mut {}", type_to_display(inner)) } else { format!("&{}", type_to_display(inner)) },
        TypeExpr::Dyn(inner) => format!("dyn {}", type_to_display(inner)),
        TypeExpr::ImplTrait(inner) => format!("impl {}", type_to_display(inner)),
        TypeExpr::FnPtr(params, ret) => { let p = params.iter().map(type_to_display).collect::<Vec<_>>().join(", "); let r = ret.as_ref().map(|t| format!(" -> {}", type_to_display(t))).unwrap_or_default(); format!("fn({}){}", p, r) }
        TypeExpr::LitStr(s) => format!("\"{s}\""),
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
        Expr::Stock => "stock".to_string(),
        Expr::Ident(name) => name.clone(),
        Expr::FieldAccess(base, field) => format!("{}.{}", expr_to_display(base), field),
        Expr::Call(call) => {
            let args = call.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
            if let Some(recv) = &call.receiver {
                format!("{}.{}({})", expr_to_display(recv), call.method, args)
            } else if call.method.is_empty() {
                format!("{}({})", call.target, args)
            } else {
                format!("{}.{}({})", call.target, call.method, args)
            }
        }
        Expr::Action(a) => action_to_display(a),
        Expr::Assign(name, rhs, ty) => {
            if let Some(t) = ty {
                format!("{}: {} = {}", name, type_to_display(t), expr_to_display(rhs))
            } else {
                format!("{} = {}", name, expr_to_display(rhs))
            }
        }
        Expr::MutAssign(name, rhs, _) => format!("mut {} = {}", name, expr_to_display(rhs)),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_display(inner)),
        Expr::Await(inner) => format!("await {}", expr_to_display(inner)),
        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),
        Expr::Index(base, idx) => format!("{}[{}]", expr_to_display(base), expr_to_display(idx)),
        Expr::ArrayLit(items) => { let s = items.iter().map(expr_to_display).collect::<Vec<_>>().join(", "); format!("[{}]", s) }
        Expr::Range { start, end, inclusive } => { let s = start.as_ref().map(|e| expr_to_display(e)).unwrap_or_default(); let e = end.as_ref().map(|e| expr_to_display(e)).unwrap_or_default(); let op = if *inclusive { "..=" } else { ".." }; format!("{}{}{}", s, op, e) }
        Expr::Loop(_) => "loop { ... }".to_string(),
        Expr::DoBlock(_) => "do { ... }".to_string(),
        Expr::Cast(expr, ty) => format!("{} as {}", expr_to_display(expr), ty),
        Expr::Try(expr) => format!("{}?", expr_to_display(expr)),
        Expr::StructUpdate { name, fields, base } => { let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_display(v))).collect::<Vec<_>>().join(", "); format!("{} {{ {}, ..{} }}", name, fs, expr_to_display(base)) }
        Expr::IfLet { pattern, .. } => format!("if let {} = ...", pattern),
        Expr::WhileLet { pattern, .. } => format!("while let {} = ...", pattern),
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
        Expr::Tuple(items) => {
            let parts = items.iter().map(expr_to_display).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        Expr::StringInterp(_parts) => {
            "f\"...\"".to_string()
        }
        Expr::Closure { params, body: _ } => {
            let p = params.join(", ");
            format!("|{}| ...", p)
        }
        Expr::LetPattern(pattern, expr, _) => {
            format!("let {} = {}", pattern.to_string_repr(), expr_to_display(expr))
        }
    }
}

/// Render a layer statement as display text: `dispatch Evt{...}`, `guard cond, "msg"`.
pub fn action_to_display(a: &ActionExpr) -> String {
    let bound = |s: String| -> String {
        if let Some(b) = &a.result_binding {
            format!("{} = {}", b, s)
        } else {
            s
        }
    };
    match a.shape {
        crate::layer::StmtShape::Call
        | crate::layer::StmtShape::Assign
        | crate::layer::StmtShape::Infix => {
            let head = if a.target.is_empty() {
                a.keyword.clone()
            } else if a.method.is_empty() {
                format!("{} {}", a.keyword, a.target)
            } else {
                format!("{} {}.{}", a.keyword, a.target, a.method)
            };
            let core = if !a.named_args.is_empty() {
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
                if a.target.is_empty() {
                    format!("{} {}", a.keyword, args)
                } else {
                    format!("{}({})", head, args)
                }
            } else {
                head
            };
            bound(core)
        }
        crate::layer::StmtShape::If => {
            let cond = a
                .condition
                .as_ref()
                .map(|c| expr_to_display(c))
                .unwrap_or_default();
            let core = if let Some(msg) = &a.message {
                format!("{} {}, \"{}\"", a.keyword, cond, msg)
            } else {
                format!("{} {}", a.keyword, cond)
            };
            bound(core)
        }
        crate::layer::StmtShape::Block => {
            let head = if a.target.is_empty() {
                a.keyword.clone()
            } else {
                format!("{} {}", a.keyword, a.target)
            };
            let args = if a.args.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    a.args.iter().map(expr_to_display).collect::<Vec<_>>().join(", ")
                )
            };
            bound(format!("{}{} …", head, args))
        }
    }
}

struct IrBuilder<'a> {
    graph: IrGraph,
    registry: Option<&'a LayerRegistry>,
}

// note: registry carries IdentityPolicy for FK edges (INV-006)

impl<'a> IrBuilder<'a> {
    fn new(registry: Option<&'a LayerRegistry>) -> Self {
        Self {
            graph: IrGraph::new(),
            registry,
        }
    }

    fn is_dep(&self, field: &Field) -> bool {
        field_is_dep(field, self.registry)
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
                TopLevelItem::Function(f) => {
                    // A free function (e.g. a layer-declared coordinator) shows
                    // as a Flow-kind node with its signature as a property.
                    let id = self.graph.add_node(NodeKind::Flow, f.name.clone(), f.span);
                    self.set_parent(id, sol_id);
                    self.graph.add_edge(sol_id, id, EdgeKind::Contains);
                    let sig = f
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, type_to_display(&p.type_expr)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.set_property(id, "params", &sig);
                    if f.layer_provided {
                        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == id) {
                            node.metadata.annotations.push("layer-provided".to_string());
                        }
                    }
                    // Build steps (typed steps + plain steps) as child nodes.
                    if !f.steps.is_empty() {
                        let _ = self.build_steps(&f.steps, id);
                    }
                }
                TopLevelItem::TypeAlias { .. } | TopLevelItem::Const { .. } | TopLevelItem::Static { .. } => {}
                TopLevelItem::TestBlock(_) | TopLevelItem::Fixture(_)
                | TopLevelItem::Integration(_) | TopLevelItem::Scenario(_) => {}
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
        // Surface layer-provided provenance to the viewer so it can visually
        // distinguish injected infrastructure (e.g. the Bus port) from
        // user-authored constructs.
        if c.layer_provided {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == id) {
                node.metadata.annotations.push("layer-provided".to_string());
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
                // Emit each field as a drillable child node.
                for f in &all_fields {
                    let field_id = self.graph.add_node(
                        NodeKind::Field,
                        format!("{}: {}", f.name, type_to_display(&f.type_expr)),
                        c.span,
                    );
                    self.set_parent(field_id, id);
                    self.set_property(field_id, "name", &f.name);
                    self.set_property(field_id, "type", &type_to_display(&f.type_expr));
                    self.graph.add_edge(id, field_id, EdgeKind::Contains);
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
                // Business logic fns as InterfaceMethod children so the viewer
                // can list signatures, show @invariant, and open bodies (UX-025).
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
                    let sig = format!("({}){}", params, ret);
                    // Keep summary property for outline/search.
                    self.set_property(
                        id,
                        &format!("fn:{}", f.name),
                        &format!("({}){}", params, ret),
                    );

                    let method_id = self.graph.add_node(
                        NodeKind::InterfaceMethod,
                        f.name.clone(),
                        f.span,
                    );
                    self.set_parent(method_id, id);
                    self.set_property(method_id, "params", &format!("({})", params));
                    if let Some(rt) = &f.return_type {
                        self.set_property(method_id, "returns", &type_to_display(rt));
                    }
                    self.set_property(method_id, "signature", &sig);
                    if f.body.is_empty() {
                        self.set_property(method_id, "abstract", "true");
                    } else {
                        self.set_property(method_id, "has_body", "true");
                    }
                    for ann in &f.annotations {
                        if let Some(node) =
                            self.graph.nodes.iter_mut().find(|n| n.id == method_id)
                        {
                            node.metadata
                                .annotations
                                .push(annotation_to_ir_string(ann));
                        }
                    }
                    if f.layer_provided {
                        if let Some(node) =
                            self.graph.nodes.iter_mut().find(|n| n.id == method_id)
                        {
                            node.metadata
                                .annotations
                                .push("layer-provided".to_string());
                        }
                    }
                    self.graph.add_edge(id, method_id, EdgeKind::Contains);

                    if !f.params.is_empty() {
                        let inputs_id = self.graph.add_node(
                            NodeKind::Inputs,
                            "Inputs".to_string(),
                            f.span,
                        );
                        self.set_parent(inputs_id, method_id);
                        self.set_property(inputs_id, "params", &params);
                        self.graph.add_edge(method_id, inputs_id, EdgeKind::Contains);
                    }
                    if !f.body.is_empty() {
                        self.build_step_body(&f.body, method_id);
                    }
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
                // Emit each method as a child InterfaceMethod node (drillable).
                for m in &c.methods {
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
                    let sig = format!("({}){}", params, ret);

                    let method_id = self.graph.add_node(
                        NodeKind::InterfaceMethod,
                        m.name.clone(),
                        m.span,
                    );
                    self.set_parent(method_id, id);
                    self.set_property(method_id, "params", &format!("({})", params));
                    self.set_property(method_id, "returns", &ret.trim_start_matches(" -> ").to_string());
                    self.set_property(method_id, "signature", &sig);
                    self.set_property(method_id, "abstract", "true");
                    self.graph.add_edge(id, method_id, EdgeKind::Contains);

                    // Emit Inputs child node for the method parameters.
                    if !m.params.is_empty() {
                        let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), m.span);
                        self.set_parent(inputs_id, method_id);
                        self.set_property(inputs_id, "params", &params);
                        self.graph.add_edge(method_id, inputs_id, EdgeKind::Contains);
                    }

                    // Emit Return child node for the return type.
                    let ret_display = if ret.is_empty() { "→ void".to_string() } else { format!("→ {}", ret.trim_start_matches(" -> ")) };
                    let ret_id = self.graph.add_node(NodeKind::Return, ret_display, m.span);
                    self.set_parent(ret_id, method_id);
                    self.graph.add_edge(method_id, ret_id, EdgeKind::Contains);
                }

                // Also keep a summary "methods" property for backward compat
                // (other parts of the system may still read it).
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
                // Emit each implemented method as a child node.
                for imp in &c.impls {
                    let params_str = imp.params.join(", ");
                    let method_id = self.graph.add_node(
                        NodeKind::InterfaceMethod,
                        imp.method_name.clone(),
                        imp.span,
                    );
                    self.set_parent(method_id, id);
                    self.set_property(method_id, "params", &format!("({})", params_str));
                    if imp.body.is_empty() {
                        self.set_property(method_id, "abstract", "true");
                    } else {
                        self.set_property(method_id, "has_body", "true");
                    }
                    self.graph.add_edge(id, method_id, EdgeKind::Contains);

                    // Emit Inputs child node for the method parameters.
                    if !imp.params.is_empty() {
                        let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), imp.span);
                        self.set_parent(inputs_id, method_id);
                        self.set_property(inputs_id, "params", &params_str);
                        self.graph.add_edge(method_id, inputs_id, EdgeKind::Contains);
                    }

                    // Emit body expressions as Action child nodes.
                    if !imp.body.is_empty() {
                        self.build_step_body(&imp.body, method_id);
                    }

                    // Emit Return node — scan body for ret expressions.
                    let mut ret_expr_str = String::new();
                    for expr in &imp.body {
                        if let Expr::Return(inner) = expr {
                            ret_expr_str = expr_to_display(inner);
                        }
                    }
                    let ret_display = if !ret_expr_str.is_empty() {
                        format!("→ {}", ret_expr_str)
                    } else {
                        "→ void".to_string()
                    };
                    let ret_id = self.graph.add_node(NodeKind::Return, ret_display, imp.span);
                    self.set_parent(ret_id, method_id);
                    if !ret_expr_str.is_empty() {
                        self.set_property(ret_id, "expr", &ret_expr_str);
                    }
                    self.graph.add_edge(method_id, ret_id, EdgeKind::Contains);
                }
            }
            Shape::Fn => {
                // Reference lines (e.g. `contexts Identity, Billing`) as
                // properties, prefixed with `ref:` so the viewer renders them
                // generically without knowing the layer keyword.
                for r in &c.refs {
                    self.set_property(id, &format!("ref:{}", r.keyword), &r.values.join(", "));
                }
                if !c.inputs.is_empty() {
                    let inputs_str = c
                        .inputs
                        .iter()
                        .map(|f| {
                            let prefix = if self.is_dep(f) { "@dep " } else { "" };
                            format!("{}{}: {}", prefix, f.name, type_to_display(&f.type_expr))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), c.span);
                    self.set_parent(inputs_id, id);
                    self.set_property(inputs_id, "params", &inputs_str);
                    // Set dep_params for dependency-role inputs (INV-001)
                    let dep_params: Vec<String> = c
                        .inputs
                        .iter()
                        .filter(|f| self.is_dep(f))
                        .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                        .collect();
                    if !dep_params.is_empty() {
                        self.set_property(inputs_id, "dep_params", &dep_params.join(", "));
                    }
                    self.graph.add_edge(id, inputs_id, EdgeKind::Contains);
                }
                let (first_step, last_step) = self.build_steps(&c.steps, id);
                // Sequence: Inputs → first step (when both present)
                if let (Some(inputs_id), Some(first)) = (
                    self.graph
                        .nodes
                        .iter()
                        .find(|n| {
                            n.metadata.parent == Some(id) && n.kind == NodeKind::Inputs
                        })
                        .map(|n| n.id),
                    first_step,
                ) {
                    self.graph
                        .add_edge(inputs_id, first, EdgeKind::SequenceFlow);
                }
                // Emit a Return node showing the return type/expression.
                // Check construct-level return type, or scan steps for ret exprs.
                let ret_label = if let Some(rt) = &c.return_type {
                    type_to_display(rt)
                } else {
                    "void".to_string()
                };
                // Scan steps for ret expr before creating the node
                let mut scanned_ret_expr = String::new();
                if c.return_expr.is_none() {
                    for flow_step in &c.steps {
                        if let FlowStep::Step(step) = flow_step {
                            for expr in &step.body {
                                if let Expr::Return(inner) = expr {
                                    scanned_ret_expr = expr_to_display(inner);
                                }
                            }
                        }
                    }
                }
                // Use a descriptive label: declared type > ret expr > void
                let display_label = if ret_label != "void" {
                    format!("→ {}", ret_label)
                } else if !scanned_ret_expr.is_empty() {
                    format!("→ {}", scanned_ret_expr)
                } else {
                    "→ void".to_string()
                };
                // Place Return after last step in span so layout sorts left→right correctly
                let ret_span = last_step
                    .and_then(|sid| self.graph.nodes.iter().find(|n| n.id == sid))
                    .map(|n| Span {
                        start: n.span.end.saturating_add(1),
                        end: n.span.end.saturating_add(2),
                    })
                    .unwrap_or(c.span);
                let ret_id = self.graph.add_node(NodeKind::Return, display_label, ret_span);
                self.set_parent(ret_id, id);
                if let Some(expr) = &c.return_expr {
                    self.set_property(ret_id, "expr", &expr_to_display(expr));
                } else if !scanned_ret_expr.is_empty() {
                    self.set_property(ret_id, "expr", &scanned_ret_expr);
                }
                self.graph.add_edge(id, ret_id, EdgeKind::Contains);
                // Sequence: last step → Return (marching ants in the flow graph)
                if let Some(prev) = last_step {
                    self.graph
                        .add_edge(prev, ret_id, EdgeKind::SequenceFlow);
                }
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
                .map(|f| {
                    let prefix = if self.is_dep(f) { "@dep " } else { "" };
                    format!("{}{}: {}", prefix, f.name, type_to_display(&f.type_expr))
                })
                .collect::<Vec<_>>()
                .join(", ");
            let inputs_id = self.graph.add_node(NodeKind::Inputs, "Inputs".to_string(), flow.span);
            self.set_parent(inputs_id, flow_id);
            self.set_property(inputs_id, "params", &inputs_str);
            // Set dep_params for dependency-role inputs (INV-001)
            let dep_params: Vec<String> = flow
                .inputs
                .iter()
                .filter(|f| self.is_dep(f))
                .map(|f| format!("{}: {}", f.name, type_to_display(&f.type_expr)))
                .collect();
            if !dep_params.is_empty() {
                self.set_property(inputs_id, "dep_params", &dep_params.join(", "));
            }
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

        let (first_step, last_step) = self.build_steps(&flow.steps, flow_id);
        if let (Some(inputs_id), Some(first)) = (
            self.graph
                .nodes
                .iter()
                .find(|n| n.metadata.parent == Some(flow_id) && n.kind == NodeKind::Inputs)
                .map(|n| n.id),
            first_step,
        ) {
            self.graph
                .add_edge(inputs_id, first, EdgeKind::SequenceFlow);
        }
        // Emit a Return node for the flow (after last step in sequence + span order).
        let ret_span = last_step
            .and_then(|sid| self.graph.nodes.iter().find(|n| n.id == sid))
            .map(|n| Span {
                start: n.span.end.saturating_add(1),
                end: n.span.end.saturating_add(2),
            })
            .unwrap_or(flow.span);
        let ret_id = self
            .graph
            .add_node(NodeKind::Return, "Return".to_string(), ret_span);
        self.set_parent(ret_id, flow_id);
        self.set_property(ret_id, "type", "inferred");
        self.graph.add_edge(flow_id, ret_id, EdgeKind::Contains);
        if let Some(prev) = last_step {
            self.graph
                .add_edge(prev, ret_id, EdgeKind::SequenceFlow);
        }
    }

    /// Build sequential steps. Returns `(first_step_id, last_step_id)` for
    /// chaining Inputs → … → Return with SequenceFlow edges.
    fn build_steps(
        &mut self,
        steps: &[FlowStep],
        parent_id: NodeId,
    ) -> (Option<NodeId>, Option<NodeId>) {
        let mut first_step_id: Option<NodeId> = None;
        let mut prev_step_id: Option<NodeId> = None;
        for step in steps {
            match step {
                FlowStep::Step(s) => {
                    let step_id = self.graph.add_node(NodeKind::Step, s.name.clone(), s.span);
                    self.set_parent(step_id, parent_id);
                    self.graph.add_edge(parent_id, step_id, EdgeKind::Contains);
                    if first_step_id.is_none() {
                        first_step_id = Some(step_id);
                    }
                    if let Some(prev) = prev_step_id {
                        self.graph.add_edge(prev, step_id, EdgeKind::SequenceFlow);
                    }
                    // Typed step: set subkind from the layer-defined kind.
                    if let Some(kind) = &s.kind {
                        self.set_subkind(step_id, kind);
                    }
                    // Typed step config fields as properties.
                    for f in &s.fields {
                        self.set_property(step_id, &f.name, &f.value);
                    }
                    // Typed step edge routing (on label: target).
                    // Stored as properties; resolved to edges after all steps
                    // are created (see below).
                    for e in &s.edges {
                        self.set_property(step_id, &format!("on:{}", e.label), &e.target);
                    }
                    // Reference lines within the step (e.g. `ctx Identity`).
                    // Prefixed with `ref:` so the viewer can render them
                    // generically without knowing the layer keyword.
                    for r in &s.refs {
                        self.set_property(step_id, &format!("ref:{}", r.keyword), &r.values.join(", "));
                    }
                    // Named sub-blocks (e.g. compensate): annotate parent + emit
                    // nested Step so bodies are visible in the graph (UX-026).
                    for sb in &s.sub_blocks {
                        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == step_id) {
                            node.metadata.annotations.push(format!("has_{}", sb.keyword));
                        }
                        let sub_id = self.graph.add_node(
                            NodeKind::Step,
                            sb.keyword.clone(),
                            sb.span,
                        );
                        self.set_parent(sub_id, step_id);
                        self.set_subkind(sub_id, &sb.keyword);
                        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == sub_id) {
                            node.metadata.annotations.push("sub_block".to_string());
                            node.metadata
                                .annotations
                                .push(format!("has_{}", sb.keyword));
                        }
                        self.graph.add_edge(step_id, sub_id, EdgeKind::Contains);
                        self.build_step_body(&sb.body, sub_id);
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
                    if first_step_id.is_none() {
                        first_step_id = Some(par_id);
                    }
                    for s in &par.steps {
                        let sub_id = self.graph.add_node(NodeKind::Step, s.name.clone(), s.span);
                        self.set_parent(sub_id, par_id);
                        self.graph.add_edge(par_id, sub_id, EdgeKind::Contains);
                        self.build_step_body(&s.body, sub_id);
                    }
                    prev_step_id = Some(par_id);
                }
                FlowStep::Match(m) => {
                    // Match blocks appear as decision nodes. The scrutinee is the
                    // node label; each arm is a child step whose name is the pattern.
                    let scrutinee = expr_to_display(&m.expr);
                    let match_id = self.graph.add_node(NodeKind::Step, format!("match {}", scrutinee), m.span);
                    self.set_parent(match_id, parent_id);
                    self.set_subkind(match_id, "decision");
                    self.graph.add_edge(parent_id, match_id, EdgeKind::Contains);
                    if let Some(prev) = prev_step_id {
                        self.graph.add_edge(prev, match_id, EdgeKind::SequenceFlow);
                    }
                    for arm in &m.arms {
                        let arm_id = self.graph.add_node(NodeKind::Step, arm.pattern.clone(), arm.span);
                        self.set_parent(arm_id, match_id);
                        self.graph.add_edge(match_id, arm_id, EdgeKind::Contains);
                        self.build_step_body(&arm.body, arm_id);
                    }
                    if first_step_id.is_none() {
                        first_step_id = Some(match_id);
                    }
                    prev_step_id = Some(match_id);
                }
            }
        }
        (first_step_id, prev_step_id)
    }

    /// Lower a statement list into sequential Action nodes under `parent_id`.
    ///
    /// Handles control flow (`for` / `if` / `ret` / …) that previously fell
    /// through the `_` arm — those left method bodies empty in the IDE even
    /// when `has_body` was true (e.g. aggregate `fn` with for/if/ret).
    ///
    /// Each Action gets monotonic `seq` + nesting `depth` so the IDE can order
    /// bodies and re-indent view mode (flat sibling Actions still carry depth).
    fn build_step_body(&mut self, body: &[Expr], parent_id: NodeId) {
        let mut prev_action: Option<NodeId> = None;
        let mut seq: u32 = 0;
        self.emit_body_exprs(body, parent_id, 0, &mut prev_action, &mut seq);
    }

    fn emit_body_exprs(
        &mut self,
        body: &[Expr],
        parent_id: NodeId,
        depth: u32,
        prev_action: &mut Option<NodeId>,
        seq: &mut u32,
    ) {
        for expr in body {
            self.emit_body_expr(expr, parent_id, depth, prev_action, seq);
        }
    }

    fn link_seq(&mut self, prev: &mut Option<NodeId>, curr: NodeId) {
        if let Some(p) = *prev {
            self.graph.add_edge(p, curr, EdgeKind::SequenceFlow);
        }
        *prev = Some(curr);
    }

    fn push_action(
        &mut self,
        parent_id: NodeId,
        label: String,
        subkind: &str,
        prefer_span: Span,
        expr_prop: Option<&str>,
        depth: u32,
        seq: &mut u32,
    ) -> NodeId {
        // Prefer real source span when present; otherwise sequential synthetic spans
        // so sort-by-span matches emission order (not name alphabetization).
        let span = if prefer_span.start != 0 || prefer_span.end != 0 {
            prefer_span
        } else {
            let s = (*seq as usize + 1) * 10;
            Span::new(s, s + 1)
        };
        let id = self.graph.add_node(NodeKind::Action, label.clone(), span);
        self.set_parent(id, parent_id);
        self.set_subkind(id, subkind);
        self.graph.add_edge(parent_id, id, EdgeKind::Contains);
        self.set_property(id, "seq", &seq.to_string());
        self.set_property(id, "depth", &depth.to_string());
        *seq += 1;
        if let Some(e) = expr_prop {
            self.set_property(id, "expr", e);
        } else {
            self.set_property(id, "expr", &label);
        }
        id
    }

    fn emit_body_expr(
        &mut self,
        expr: &Expr,
        parent_id: NodeId,
        depth: u32,
        prev_action: &mut Option<NodeId>,
        seq: &mut u32,
    ) {
        match expr {
            Expr::Call(call) => {
                let label = if call.method.is_empty() {
                    format!("call {}", call.target)
                } else {
                    format!("call {}.{}", call.target, call.method)
                };
                let id = self.push_action(
                    parent_id,
                    label,
                    "call",
                    call.span,
                    Some(&expr_to_display(expr)),
                    depth,
                    seq,
                );
                let args_str = call
                    .args
                    .iter()
                    .map(expr_to_display)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !args_str.is_empty() {
                    self.set_property(id, "args", &args_str);
                }
                self.annotate_impl_binding(id, &call.target);
                self.link_seq(prev_action, id);
            }
            Expr::Action(a) => {
                let label = action_to_display(a);
                let id =
                    self.push_action(parent_id, label, &a.keyword, a.span, None, depth, seq);
                if !a.named_args.is_empty() {
                    let fields_str = a
                        .named_args
                        .iter()
                        .map(|(k, v)| {
                            let vs = expr_to_display(v);
                            if k == &vs {
                                k.clone()
                            } else {
                                format!("{}: {}", k, vs)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.set_property(id, "fields", &format!("{{{}}}", fields_str));
                }
                if !a.args.is_empty() {
                    let args_str = a
                        .args
                        .iter()
                        .map(expr_to_display)
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.set_property(id, "args", &format!("({})", args_str));
                }
                if let Some(msg) = &a.message {
                    self.set_property(id, "message", msg);
                }
                if !a.target.is_empty() {
                    self.annotate_impl_binding(id, &a.target);
                }
                self.link_seq(prev_action, id);
            }
            Expr::Assign(name, rhs, _ty) | Expr::MutAssign(name, rhs, _ty) => {
                let rhs_display = expr_to_display(rhs);
                let mut_kw = matches!(expr, Expr::MutAssign(_, _, _));
                let core = if mut_kw {
                    format!("mut {} = {}", name, rhs_display)
                } else {
                    format!("{} = {}", name, rhs_display)
                };
                let id = self.push_action(
                    parent_id,
                    core.clone(),
                    if mut_kw { "mut_assign" } else { "assign" },
                    Span::new(0, 0),
                    Some(&core),
                    depth,
                    seq,
                );
                if let Expr::Call(call) = rhs.as_ref() {
                    let args_str = call
                        .args
                        .iter()
                        .map(expr_to_display)
                        .collect::<Vec<_>>()
                        .join(", ");
                    if !args_str.is_empty() {
                        self.set_property(id, "args", &format!("({})", args_str));
                    }
                    self.annotate_impl_binding(id, &call.target);
                    if let Some(port_node) = self
                        .graph
                        .nodes
                        .iter()
                        .find(|n| n.kind == NodeKind::Interface && n.name == call.target)
                    {
                        let port_id = port_node.id;
                        self.graph.add_edge(id, port_id, EdgeKind::Calls);
                    }
                }
                self.link_seq(prev_action, id);
            }
            Expr::Return(inner) => {
                // Label is the return *value* only — subkind is "return" so the
                // editor can map to Expr::return without "return ret …" doubling.
                let value = expr_to_display(inner);
                let expr_full = format!("ret {}", value);
                let id = self.push_action(
                    parent_id,
                    value.clone(),
                    "return",
                    Span::new(0, 0),
                    Some(&expr_full),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
            }
            Expr::ForLoop {
                binding,
                index,
                iterable,
                body,
            } => {
                let core = if let Some(idx) = index {
                    format!(
                        "for {}, {} in {}",
                        binding,
                        idx,
                        expr_to_display(iterable)
                    )
                } else {
                    format!("for {} in {}", binding, expr_to_display(iterable))
                };
                let id = self.push_action(
                    parent_id,
                    core.clone(),
                    "for",
                    Span::new(0, 0),
                    Some(&core),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                self.emit_body_exprs(body, parent_id, depth + 1, prev_action, seq);
            }
            Expr::WhileLoop { condition, body } => {
                let core = format!("while {}", expr_to_display(condition));
                let id = self.push_action(
                    parent_id,
                    core.clone(),
                    "while",
                    Span::new(0, 0),
                    Some(&core),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                self.emit_body_exprs(body, parent_id, depth + 1, prev_action, seq);
            }
            Expr::Loop(body) => {
                let id = self.push_action(
                    parent_id,
                    "loop".into(),
                    "loop",
                    Span::new(0, 0),
                    Some("loop"),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                self.emit_body_exprs(body, parent_id, depth + 1, prev_action, seq);
            }
            Expr::IfExpr(ie) => {
                // Name = condition only (subkind "if") — avoids "if if cond" in UI
                let cond = expr_to_display(&ie.condition);
                let expr_full = format!("if {}", cond);
                let id = self.push_action(
                    parent_id,
                    cond,
                    "if",
                    Span::new(0, 0),
                    Some(&expr_full),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                self.emit_body_exprs(&ie.then_body, parent_id, depth + 1, prev_action, seq);
                if let Some(else_body) = &ie.else_body {
                    let else_id = self.push_action(
                        parent_id,
                        "else".into(),
                        "else",
                        Span::new(0, 0),
                        Some("else"),
                        depth,
                        seq,
                    );
                    self.link_seq(prev_action, else_id);
                    self.emit_body_exprs(else_body, parent_id, depth + 1, prev_action, seq);
                }
            }
            Expr::IfLet {
                pattern,
                expr: scrut,
                then_body,
                else_body,
            } => {
                let core = format!("if let {} = {}", pattern, expr_to_display(scrut));
                let id = self.push_action(
                    parent_id,
                    core.clone(),
                    "if_let",
                    Span::new(0, 0),
                    Some(&core),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                self.emit_body_exprs(then_body, parent_id, depth + 1, prev_action, seq);
                if let Some(else_body) = else_body {
                    let else_id = self.push_action(
                        parent_id,
                        "else".into(),
                        "else",
                        Span::new(0, 0),
                        Some("else"),
                        depth,
                        seq,
                    );
                    self.link_seq(prev_action, else_id);
                    self.emit_body_exprs(else_body, parent_id, depth + 1, prev_action, seq);
                }
            }
            Expr::Match(scrutinee, arms) => {
                let core = format!("match {}", expr_to_display(scrutinee));
                let id = self.push_action(
                    parent_id,
                    core.clone(),
                    "match",
                    Span::new(0, 0),
                    Some(&core),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                for arm in arms {
                    let arm_label = format!("→ {}", arm.pattern);
                    let arm_id = self.push_action(
                        parent_id,
                        arm_label,
                        "match_arm",
                        Span::new(0, 0),
                        Some(&format!("{} -> …", arm.pattern)),
                        depth + 1,
                        seq,
                    );
                    self.link_seq(prev_action, arm_id);
                    self.emit_body_exprs(&arm.body, parent_id, depth + 2, prev_action, seq);
                }
            }
            Expr::DoBlock(stmts) => {
                let id = self.push_action(
                    parent_id,
                    "do".into(),
                    "do",
                    Span::new(0, 0),
                    Some("do { … }"),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
                self.emit_body_exprs(stmts, parent_id, depth + 1, prev_action, seq);
            }
            Expr::Break => {
                let id = self.push_action(
                    parent_id,
                    "break".into(),
                    "break",
                    Span::new(0, 0),
                    Some("break"),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
            }
            Expr::Continue => {
                let id = self.push_action(
                    parent_id,
                    "continue".into(),
                    "continue",
                    Span::new(0, 0),
                    Some("continue"),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
            }
            // Expressions that are valid statements in a body but not Call/Action/Assign
            other => {
                let core = expr_to_display(other);
                if core.is_empty() || core == "…" {
                    return;
                }
                let id = self.push_action(
                    parent_id,
                    core.clone(),
                    "expr",
                    Span::new(0, 0),
                    Some(&core),
                    depth,
                    seq,
                );
                self.link_seq(prev_action, id);
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

    /// Detect FK-style references between constructs.
    /// When a TypeDef node has a field like `cohort_id: Id`, look for a construct
    /// INV-006: FK / reference edges only when layer identity_policy enables a ref_suffix.
    fn resolve_references(&mut self) {
        let Some(reg) = self.registry else {
            return;
        };
        let Some(suffix) = reg.identity_policy.ref_suffix.clone() else {
            // Default off: no magic `*_id` inference without layer opt-in.
            return;
        };
        let mut ref_edges: Vec<(NodeId, NodeId)> = Vec::new();

        // Collect all construct names (TypeDef, Module, Flow, Interface) and their IDs.
        let constructs: Vec<(NodeId, String)> = self
            .graph
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::TypeDef | NodeKind::Module | NodeKind::Flow))
            .map(|n| (n.id, n.name.clone()))
            .collect();

        // For each TypeDef node, look at its "fields" property for suffix patterns.
        for node in &self.graph.nodes {
            if node.kind != NodeKind::TypeDef {
                continue;
            }
            let fields_str = node
                .metadata
                .properties
                .iter()
                .find(|(k, _)| k == "fields")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();

            // Parse field names: "cohort_id: Id, name: Str, ..." when suffix is "_id"
            for field in fields_str.split(", ") {
                let field_name = field.split(':').next().unwrap_or("").trim();
                if !field_name.ends_with(&suffix) {
                    continue;
                }
                let id_field = reg
                    .identity_policy
                    .identity_field
                    .as_deref()
                    .unwrap_or("id");
                if field_name == id_field {
                    continue;
                }
                // Strip suffix and convert to PascalCase to match construct names.
                let ref_name = field_name.trim_end_matches(&suffix);
                let pascal = ref_name
                    .split('_')
                    .map(|part| {
                        let mut c = part.chars();
                        match c.next() {
                            Some(first) => first.to_uppercase().to_string() + c.as_str(),
                            None => String::new(),
                        }
                    })
                    .collect::<String>();

                // Find the target construct.
                if let Some((target_id, _)) = constructs.iter().find(|(_, name)| *name == pascal) {
                    // Don't add self-references or duplicate edges.
                    if *target_id != node.id {
                        ref_edges.push((node.id, *target_id));
                    }
                }
            }
        }

        for (from, to) in ref_edges {
            // Avoid duplicate edges.
            let exists = self.graph.edges.iter().any(|e| {
                e.from == from && e.to == to && e.kind == EdgeKind::References
            });
            if !exists {
                self.graph.add_edge(from, to, EdgeKind::References);
            }
        }
    }
}

#[cfg(test)]
mod ref_edge_tests {
    use super::*;
    use crate::ast::{Construct, Field, Solution, TopLevelItem, TypeExpr};
    use crate::layer::LayerRegistry;
    use crate::span::Span;

    fn field(name: &str, ty: &str) -> Field {
        Field {
            annotations: vec![],
            name: name.into(),
            type_expr: TypeExpr::Named(ty.into()),
            default_expr: None,
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn fk_fields_emit_references_with_identity_policy() {
        let mut reg = LayerRegistry::builtin();
        let layer = r#"
pkg testddd v1
  identity_policy
    ref_suffix _id
    identity_field id
  construct Aggregate
    kw agg
    mt struct
  construct Entity
    kw ent
    mt struct
"#;
        reg.load_content("testddd", layer).unwrap();
        assert_eq!(reg.identity_policy.ref_suffix.as_deref(), Some("_id"));

        let mut cohort = Construct::new(
            "agg",
            "Aggregate",
            Shape::Struct,
            "Cohort".into(),
            Span::new(0, 1),
        );
        cohort.fields.push(field("id", "Id"));

        let mut member = Construct::new(
            "ent",
            "Entity",
            Shape::Struct,
            "Member".into(),
            Span::new(0, 1),
        );
        member.fields.push(field("id", "Id"));
        member.fields.push(field("cohort_id", "Id"));

        let sol = Solution {
            name: "T".into(),
            span: Span::new(0, 1),
            uses: vec![],

            links: vec![],
items: vec![
                TopLevelItem::Construct(cohort),
                TopLevelItem::Construct(member),
            ],
            expose: None,
            guidance: Vec::new(),
        };
        let graph = build_ir_with_registry(&sol, Some(&reg));
        let refs: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References)
            .collect();
        assert!(
            !refs.is_empty(),
            "expected References edges; kinds: {:?}",
            graph
                .edges
                .iter()
                .map(|e| format!("{:?}", e.kind))
                .collect::<Vec<_>>()
        );
        let by_id: std::collections::HashMap<_, _> =
            graph.nodes.iter().map(|n| (n.id, n.name.as_str())).collect();
        let pairs: Vec<_> = refs
            .iter()
            .map(|e| (by_id[&e.from], by_id[&e.to]))
            .collect();
        assert!(
            pairs.iter().any(|(f, t)| *f == "Member" && *t == "Cohort"),
            "pairs: {:?}",
            pairs
        );
    }

    #[test]
    fn no_references_without_identity_policy() {
        let reg = LayerRegistry::builtin();
        assert!(reg.identity_policy.ref_suffix.is_none());
        let mut member = Construct::new(
            "struct",
            "Struct",
            Shape::Struct,
            "Member".into(),
            Span::new(0, 1),
        );
        member.fields.push(field("cohort_id", "Id"));
        let sol = Solution {
            name: "T".into(),
            span: Span::new(0, 1),
            uses: vec![],

            links: vec![],
items: vec![TopLevelItem::Construct(member)],
            expose: None,
            guidance: Vec::new(),
        };
        let graph = build_ir_with_registry(&sol, Some(&reg));
        assert!(!graph.edges.iter().any(|e| e.kind == EdgeKind::References));
    }

    /// Aggregate/struct methods with for/if/ret must lower to Action children
    /// so the IDE flow graph is not empty (has_body alone is not enough).
    #[test]
    fn method_body_for_if_ret_emits_actions() {
        use crate::ast::{
            BinOp, BinaryOpExpr, CallExpr, Expr, FnDef, IfExprData, Param, TypeExpr,
        };
        let mut c = Construct::new(
            "struct",
            "Struct",
            Shape::Struct,
            "ApiProvider".into(),
            Span::new(0, 1),
        );
        c.fields.push(field("api_endpoints", "List<ApiEndpoint>"));
        let f = FnDef {
            name: "get_endpoint".into(),
            span: Span::new(0, 1),
            params: vec![Param {
                name: "endpoint_id".into(),
                type_expr: TypeExpr::Named("Id".into()),
                span: Span::new(0, 0),
            }],
            return_type: Some(TypeExpr::Optional(Box::new(TypeExpr::Named(
                "ApiEndpoint".into(),
            )))),
            annotations: vec![],
            body: vec![
                Expr::ForLoop {
                    binding: "ep".into(),
                    index: None,
                    iterable: Box::new(Expr::FieldAccess(
                        Box::new(Expr::Ident("self".into())),
                        "api_endpoints".into(),
                    )),
                    body: vec![Expr::IfExpr(IfExprData {
                        condition: Box::new(Expr::BinaryOp(BinaryOpExpr {
                            left: Box::new(Expr::FieldAccess(
                                Box::new(Expr::Ident("ep".into())),
                                "id".into(),
                            )),
                            op: BinOp::Eq,
                            right: Box::new(Expr::Ident("endpoint_id".into())),
                        })),
                        then_body: vec![Expr::Return(Box::new(Expr::Call(CallExpr {
                            target: "Opt".into(),
                            method: "some".into(),
                            args: vec![Expr::Ident("ep".into())],
                            receiver: None,
                            sugar: None,
                            span: Span::new(0, 1),
                        })))],
                        else_body: None,
                    })],
                },
                Expr::Return(Box::new(Expr::Call(CallExpr {
                    target: "Opt".into(),
                    method: "none".into(),
                    args: vec![],
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 1),
                }))),
            ],
            steps: vec![],
            layer_provided: false,
        };
        c.fns.push(f);
        let sol = Solution {
            name: "T".into(),
            span: Span::new(0, 1),
            uses: vec![],
            links: vec![],
            items: vec![TopLevelItem::Construct(c)],
            expose: None,
            guidance: Vec::new(),
        };
        let graph = build_ir(&sol);
        let method = graph
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::InterfaceMethod && n.name == "get_endpoint")
            .expect("method node");
        let kids: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.metadata.parent == Some(method.id))
            .collect();
        let actions: Vec<_> = kids
            .iter()
            .filter(|n| n.kind == NodeKind::Action)
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            actions.iter().any(|a| a.contains("for ")),
            "expected for-loop action, got {:?}",
            actions
        );
        assert!(
            actions.iter().any(|a| a.contains("Opt.some") || a.contains("Opt.none")),
            "expected return-value actions, got {:?}",
            actions
        );
        assert!(
            actions.len() >= 3,
            "for + if + rets should emit multiple actions, got {:?}",
            actions
        );
        // Source order: for → if → ret some → ret none (not alphabetical)
        let for_i = actions.iter().position(|a| a.contains("for ")).unwrap();
        let if_i = actions.iter().position(|a| a.contains("ep.id")).unwrap();
        let some_i = actions.iter().position(|a| a.contains("Opt.some")).unwrap();
        let none_i = actions.iter().position(|a| a.contains("Opt.none")).unwrap();
        assert!(
            for_i < if_i && if_i < some_i && some_i < none_i,
            "wrong body order: {:?}",
            actions
        );
    }
}


