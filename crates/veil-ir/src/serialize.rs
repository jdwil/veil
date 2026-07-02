//! VEIL Serializer — emits valid .veil text from AST.
//!
//! This is the inverse of the parser: takes a Solution/Package AST and
//! produces properly indented VEIL source code.

use crate::ast::*;

/// Serialize a Solution AST into VEIL source text.
pub fn serialize_solution(sol: &Solution) -> String {
    let mut s = Serializer::new();
    s.emit_solution(sol);
    s.output
}

/// Serialize a Package AST into VEIL source text.
pub fn serialize_package(pkg: &Package) -> String {
    let mut s = Serializer::new();
    s.emit_package(pkg);
    s.output
}

/// Serialize a Composition AST into VEIL source text.
pub fn serialize_composition(comp: &Composition) -> String {
    let mut s = Serializer::new();
    s.emit_composition(comp);
    s.output
}

struct Serializer {
    output: String,
    indent: usize,
}

impl Serializer {
    fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
        }
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        self.output.push('\n');
    }

    fn blank(&mut self) {
        self.output.push('\n');
    }

    fn indent(&mut self) {
        self.indent += 1;
    }

    fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    // ─── Solution ─────────────────────────────────────────────────────

    fn emit_solution(&mut self, sol: &Solution) {
        self.line(&format!("sol {}", sol.name));
        self.indent();
        for (i, item) in sol.items.iter().enumerate() {
            if i > 0 {
                self.blank();
            }
            self.emit_top_level_item(item);
        }
        self.dedent();
    }

    fn emit_top_level_item(&mut self, item: &TopLevelItem) {
        match item {
            TopLevelItem::Lang(lang) => self.emit_lang(lang),
            TopLevelItem::Context(ctx) => self.emit_context(ctx),
            TopLevelItem::Flow(flow) => self.emit_flow(flow),
            TopLevelItem::Adapter(adapter) => self.emit_adapter(adapter),
            TopLevelItem::Saga(saga) => self.emit_saga(saga),
        }
    }

    // ─── Package ──────────────────────────────────────────────────────

    fn emit_package(&mut self, pkg: &Package) {
        let version_str = pkg.version.as_deref().unwrap_or("");
        if version_str.is_empty() {
            self.line(&format!("pkg {}", pkg.name));
        } else {
            self.line(&format!("pkg {} {}", pkg.name, version_str));
        }
        self.indent();

        for meta in &pkg.metadata {
            self.line(&format!("{} \"{}\"", meta.key, meta.value));
        }
        if !pkg.metadata.is_empty() {
            self.blank();
        }

        for (i, item) in pkg.items.iter().enumerate() {
            if i > 0 {
                self.blank();
            }
            self.emit_top_level_item(item);
        }

        if let Some(expose) = &pkg.expose {
            self.blank();
            self.emit_expose(expose);
        }

        self.dedent();
    }

    // ─── Composition ──────────────────────────────────────────────────

    fn emit_composition(&mut self, comp: &Composition) {
        for imp in &comp.imports {
            if let Some(alias) = &imp.alias {
                self.line(&format!("use {} as {}", imp.package_name, alias));
            } else {
                self.line(&format!("use {}", imp.package_name));
            }
        }
        if !comp.imports.is_empty() {
            self.blank();
        }
        for (i, flow) in comp.flows.iter().enumerate() {
            if i > 0 {
                self.blank();
            }
            self.emit_flow(flow);
        }
    }

    // ─── Lang ─────────────────────────────────────────────────────────

    fn emit_lang(&mut self, lang: &LangBlock) {
        self.line("lang");
        self.indent();
        for entry in &lang.entries {
            self.line(&format!("{}: {}", entry.term, entry.definition));
        }
        self.dedent();
    }

    // ─── Context ──────────────────────────────────────────────────────

    fn emit_context(&mut self, ctx: &Context) {
        self.line(&format!("ctx {}", ctx.name));
        self.indent();
        for (i, item) in ctx.items.iter().enumerate() {
            if i > 0 {
                self.blank();
            }
            match item {
                ContextItem::ValueObject(vo) => self.emit_value_object(vo),
                ContextItem::Entity(ent) => self.emit_entity(ent),
                ContextItem::Aggregate(agg) => self.emit_aggregate(agg),
                ContextItem::Port(port) => self.emit_port(port),
                ContextItem::Service(svc) => self.emit_service(svc),
                ContextItem::Adapter(adapter) => self.emit_adapter(adapter),
                ContextItem::Group(group) => {
                    self.line(&format!("group {}", group.name));
                    self.indent();
                    for item in &group.items {
                        match item {
                            ContextItem::ValueObject(vo) => self.emit_value_object(vo),
                            ContextItem::Entity(ent) => self.emit_entity(ent),
                            ContextItem::Aggregate(agg) => self.emit_aggregate(agg),
                            ContextItem::Port(port) => self.emit_port(port),
                            ContextItem::Service(svc) => self.emit_service(svc),
                            ContextItem::Adapter(adapter) => self.emit_adapter(adapter),
                            ContextItem::Group(_) => {} // nested not supported
                        }
                    }
                    self.dedent();
                }
            }
        }
        self.dedent();
    }

    fn emit_value_object(&mut self, vo: &ValueObject) {
        self.line(&format!("val {}", vo.name));
        self.indent();
        for field in &vo.fields {
            self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
        }
        for ann in &vo.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        self.dedent();
    }

    fn emit_entity(&mut self, ent: &Entity) {
        self.line(&format!("ent {}", ent.name));
        self.indent();
        for field in &ent.fields {
            self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
        }
        for ann in &ent.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        self.dedent();
    }

    fn emit_aggregate(&mut self, agg: &Aggregate) {
        self.line(&format!("agg {}", agg.name));
        self.indent();
        for field in &agg.fields {
            self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
        }
        for ann in &agg.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        if !agg.fields.is_empty() && (!agg.events.is_empty() || !agg.commands.is_empty()) {
            self.blank();
        }
        for evt in &agg.events {
            self.emit_event(evt);
            self.blank();
        }
        for cmd in &agg.commands {
            self.emit_command(cmd);
            self.blank();
        }
        self.dedent();
    }

    fn emit_event(&mut self, evt: &Event) {
        self.line(&format!("evt {}", evt.name));
        self.indent();
        // Emit fields — use shorthand if type matches name
        let shorthand: Vec<&Field> = evt.fields.iter()
            .filter(|f| matches!(&f.type_expr, TypeExpr::Named(n) if n == &f.name))
            .collect();
        let typed: Vec<&Field> = evt.fields.iter()
            .filter(|f| !matches!(&f.type_expr, TypeExpr::Named(n) if n == &f.name))
            .collect();

        if !shorthand.is_empty() && typed.is_empty() {
            // All shorthand — emit on one line
            let names: Vec<&str> = shorthand.iter().map(|f| f.name.as_str()).collect();
            self.line(&names.join(" "));
        } else {
            for f in &evt.fields {
                if matches!(&f.type_expr, TypeExpr::Named(n) if n == &f.name) {
                    self.line(&f.name);
                } else {
                    self.line(&format!("{}: {}", f.name, type_to_veil(&f.type_expr)));
                }
            }
        }
        self.dedent();
    }

    fn emit_command(&mut self, cmd: &Command) {
        self.line(&format!("cmd {}", cmd.name));
        self.indent();
        for field in &cmd.fields {
            self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
        }
        if let Some(rt) = &cmd.return_type {
            self.line(&format!("-> {}", type_to_veil(rt)));
        }
        self.dedent();
    }

    // ─── Port ─────────────────────────────────────────────────────────

    fn emit_port(&mut self, port: &Port) {
        self.line(&format!("port {}", port.name));
        self.indent();
        for method in &port.methods {
            let params = method.params.iter()
                .map(|p| format!("{}: {}", p.name, type_to_veil(&p.type_expr)))
                .collect::<Vec<_>>()
                .join(", ");
            let ret = match &method.return_type {
                Some(t) => format!(" -> {}", type_to_veil(t)),
                None => String::new(),
            };
            self.line(&format!("{}({}){}", method.name, params, ret));
        }
        self.dedent();
    }

    fn emit_service(&mut self, svc: &Service) {
        for ann in &svc.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        self.line(&format!("svc {}", svc.name));
        self.indent();
        if !svc.inputs.is_empty() {
            self.line("input");
            self.indent();
            for field in &svc.inputs {
                self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
            }
            self.dedent();
        }
        for step in &svc.steps {
            self.emit_flow_step(step);
        }
        if let Some(ret) = &svc.return_expr {
            self.line(&format!("ret {}", expr_to_veil(ret)));
        }
        self.dedent();
    }

    // ─── Adapter ──────────────────────────────────────────────────────

    fn emit_adapter(&mut self, adapter: &Adapter) {
        self.line(&format!("adapter {} for {}", adapter.name, adapter.target_port));
        self.indent();
        for ann in &adapter.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        for imp in &adapter.impls {
            let params = imp.params.join(", ");
            self.line(&format!("impl {}({})", imp.method_name, params));
            self.indent();
            for expr in &imp.body {
                self.line(&expr_to_veil(expr));
            }
            self.dedent();
        }
        self.dedent();
    }

    // ─── Flow ─────────────────────────────────────────────────────────

    fn emit_flow(&mut self, flow: &Flow) {
        // Annotations above the flow (decorator style)
        for ann in &flow.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        self.line(&format!("flow {}", flow.name));
        self.indent();

        if !flow.inputs.is_empty() {
            self.line("input");
            self.indent();
            for field in &flow.inputs {
                self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
            }
            self.dedent();
            self.blank();
        }

        if let Some(eb) = &flow.error_boundary {
            self.emit_error_boundary(eb);
            self.blank();
        }

        for step in &flow.steps {
            self.emit_flow_step(step);
            self.blank();
        }

        if let Some(ret) = &flow.return_expr {
            self.line(&format!("ret {}", expr_to_veil(ret)));
        }

        self.dedent();
    }

    fn emit_error_boundary(&mut self, eb: &ErrorBoundary) {
        self.line("err boundary");
        self.indent();
        for ann in &eb.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        if let Some(fb) = &eb.fallback {
            self.line(&format!("fallback -> {}", expr_to_veil(fb)));
        }
        self.dedent();
    }

    fn emit_flow_step(&mut self, step: &FlowStep) {
        match step {
            FlowStep::Step(s) => {
                self.line(&format!("step {}", s.name));
                self.indent();
                for expr in &s.body {
                    self.line(&expr_to_veil(expr));
                }
                self.dedent();
            }
            FlowStep::Parallel(par) => {
                self.line("par");
                self.indent();
                for s in &par.steps {
                    self.line(&format!("step {}", s.name));
                    self.indent();
                    for expr in &s.body {
                        self.line(&expr_to_veil(expr));
                    }
                    self.dedent();
                }
                self.dedent();
            }
            FlowStep::Match(m) => {
                self.line(&format!("match {}", expr_to_veil(&m.expr)));
                self.indent();
                for arm in &m.arms {
                    self.line(&format!("{} ->", arm.pattern));
                    self.indent();
                    for expr in &arm.body {
                        self.line(&expr_to_veil(expr));
                    }
                    self.dedent();
                }
                self.dedent();
            }
        }
    }

    // ─── Saga ──────────────────────────────────────────────────────────

    fn emit_saga(&mut self, saga: &Saga) {
        for ann in &saga.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
        self.line(&format!("saga {}", saga.name));
        self.indent();

        if !saga.context_refs.is_empty() {
            self.line(&format!("contexts {}", saga.context_refs.join(", ")));
            self.blank();
        }

        if !saga.inputs.is_empty() {
            self.line("input");
            self.indent();
            for field in &saga.inputs {
                self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
            }
            self.dedent();
            self.blank();
        }

        for step in &saga.steps {
            self.line(&format!("step {}", step.name));
            self.indent();
            if let Some(ctx) = &step.context {
                self.line(&format!("ctx {}", ctx));
            }
            for expr in &step.body {
                self.line(&expr_to_veil(expr));
            }
            if !step.compensate.is_empty() {
                self.line("compensate");
                self.indent();
                for expr in &step.compensate {
                    self.line(&expr_to_veil(expr));
                }
                self.dedent();
            }
            self.dedent();
            self.blank();
        }

        self.dedent();
    }

    // ─── Expose ───────────────────────────────────────────────────────

    fn emit_expose(&mut self, expose: &ExposeBlock) {
        self.line("expose");
        self.indent();
        for node in &expose.nodes {
            self.emit_exposed_node(node);
            self.blank();
        }
        if !expose.constraints.is_empty() {
            self.line("constraints");
            self.indent();
            for c in &expose.constraints {
                self.line(c);
            }
            self.dedent();
        }
        self.dedent();
    }

    fn emit_exposed_node(&mut self, node: &ExposedNode) {
        self.line(&format!("node {}", node.name));
        self.indent();
        if let Some(desc) = &node.description {
            self.line(&format!("desc \"{}\"", desc));
        }
        if !node.inputs.is_empty() {
            self.line("input");
            self.indent();
            for f in &node.inputs {
                self.line(&format!("{}: {}", f.name, type_to_veil(&f.type_expr)));
            }
            self.dedent();
        }
        if !node.outputs.is_empty() {
            self.line("output");
            self.indent();
            for f in &node.outputs {
                self.line(&format!("{}: {}", f.name, type_to_veil(&f.type_expr)));
            }
            self.dedent();
        }
        self.dedent();
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────

fn type_to_veil(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Generic(name, args) => {
            let a = args.iter().map(|t| type_to_veil(t)).collect::<Vec<_>>().join(", ");
            format!("{}<{}>", name, a)
        }
        TypeExpr::Result(Some(inner)) => format!("Res!<{}>", type_to_veil(inner)),
        TypeExpr::Result(None) => "Res!".to_string(),
        TypeExpr::Optional(inner) => format!("Opt<{}>", type_to_veil(inner)),
        TypeExpr::List(inner) => format!("List<{}>", type_to_veil(inner)),
        TypeExpr::Map(k, v) => format!("Map<{}, {}>", type_to_veil(k), type_to_veil(v)),
        TypeExpr::Set(inner) => format!("Set<{}>", type_to_veil(inner)),
    }
}

fn annotation_to_veil(ann: &Annotation) -> String {
    if ann.args.is_empty() {
        ann.name.clone()
    } else {
        format!("{}({})", ann.name, ann.args.join(", "))
    }
}

fn expr_to_veil(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::FieldAccess(base, field) => format!("{}.{}", expr_to_veil(base), field),
        Expr::Call(call) => {
            let args = call.args.iter().map(|a| expr_to_veil(a)).collect::<Vec<_>>().join(", ");
            if call.method.is_empty() {
                format!("call {}({})", call.target, args)
            } else {
                format!("call {}.{}({})", call.target, call.method, args)
            }
        }
        Expr::Emit(emit) => {
            let fields = emit.fields.iter().map(|(k, v)| {
                let vs = expr_to_veil(v);
                if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
            }).collect::<Vec<_>>().join(", ");
            format!("emit {}{{{}}}", emit.event_name, fields)
        }
        Expr::Dispatch(d) => {
            let fields = d.fields.iter().map(|(k, v)| {
                let vs = expr_to_veil(v);
                if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
            }).collect::<Vec<_>>().join(", ");
            format!("dispatch {}{{{}}}", d.event_name, fields)
        }
        Expr::Invoke(inv) => {
            let params = inv.params.iter().map(|(k, v)| {
                format!("{}: {}", k, expr_to_veil(v))
            }).collect::<Vec<_>>().join(", ");
            if inv.command.is_empty() {
                format!("invoke {}{{{}}}", inv.target, params)
            } else {
                format!("invoke {}.{}{{{}}}", inv.target, inv.command, params)
            }
        }
        Expr::Request(req) => {
            let args = req.args.iter().map(|a| expr_to_veil(a)).collect::<Vec<_>>().join(", ");
            if req.method.is_empty() {
                format!("request {}({})", req.port, args)
            } else {
                format!("request {}.{}({})", req.port, req.method, args)
            }
        }
        Expr::Guard(g) => {
            let cond = expr_to_veil(&g.condition);
            if let Some(msg) = &g.message {
                format!("guard {}, \"{}\"", cond, msg)
            } else {
                format!("guard {}", cond)
            }
        }
        Expr::Assign(name, rhs) => format!("{} = {}", name, expr_to_veil(rhs)),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_veil(inner)),
        Expr::BinaryOp(op) => {
            let left = expr_to_veil(&op.left);
            let right = expr_to_veil(&op.right);
            let op_str = match &op.op {
                BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*",
                BinOp::Div => "/", BinOp::Mod => "%", BinOp::Eq => "==",
                BinOp::NotEq => "!=", BinOp::Lt => "<", BinOp::Gt => ">",
                BinOp::LtEq => "<=", BinOp::GtEq => ">=",
                BinOp::And => "&&", BinOp::Or => "||",
            };
            format!("{} {} {}", left, op_str, right)
        }
        Expr::UnaryOp(op) => {
            let op_str = match &op.op {
                UnaryOp::Not => "!", UnaryOp::Neg => "-",
            };
            format!("{}{}", op_str, expr_to_veil(&op.expr))
        }
        Expr::IfExpr(ie) => {
            format!("if {}", expr_to_veil(&ie.condition))
        }
    }
}
