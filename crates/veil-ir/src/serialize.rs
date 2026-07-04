//! VEIL Serializer — emits valid .veil text from AST.
//!
//! This is the inverse of the parser: shape-driven, zero domain knowledge.
//! Each construct is emitted according to its core shape, using the keyword
//! recorded at parse time.

use crate::ast::*;
use crate::layer::{Shape, StmtShape};

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

    // ─── Solution / Package / Composition ────────────────────────────

    fn emit_solution(&mut self, sol: &Solution) {
        self.line(&format!("sol {}", sol.name));
        self.indent();
        for u in &sol.uses {
            match &u.alias {
                Some(alias) => self.line(&format!("use {} as {}", u.package_name, alias)),
                None => self.line(&format!("use {}", u.package_name)),
            }
        }
        if !sol.uses.is_empty() {
            self.blank();
        }
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
            TopLevelItem::Construct(c) => self.emit_construct(c),
            TopLevelItem::Flow(flow) => self.emit_flow(flow),
        }
    }

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

    fn emit_lang(&mut self, lang: &LangBlock) {
        self.line("lang");
        self.indent();
        for entry in &lang.entries {
            self.line(&format!("{}: {}", entry.term, entry.definition));
        }
        self.dedent();
    }

    // ─── Generic construct emission ───────────────────────────────────

    fn emit_construct(&mut self, c: &Construct) {
        for ann in &c.annotations {
            if !ann.name.starts_with("__") {
                self.line(&format!("@{}", annotation_to_veil(ann)));
            }
        }
        let export_prefix = if c.exported { "export " } else { "" };

        match c.shape {
            Shape::Mod | Shape::Group => {
                self.line(&format!("{}{} {}", export_prefix, c.keyword, c.name));
                self.indent();
                for (i, child) in c.children.iter().enumerate() {
                    if i > 0 {
                        self.blank();
                    }
                    self.emit_construct(child);
                }
                self.dedent();
            }
            Shape::Struct => {
                self.line(&format!("{}{} {}", export_prefix, c.keyword, c.name));
                self.indent();
                for field in &c.fields {
                    self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
                }
                if let Some(rt) = &c.return_type {
                    self.line(&format!("-> {}", type_to_veil(rt)));
                }
                for block in &c.blocks {
                    match &block.name {
                        Some(n) => self.line(&format!("{} {}", block.keyword, n)),
                        None => self.line(&block.keyword),
                    }
                    self.indent();
                    if block.shape == Shape::Enum {
                        for t in &block.transitions {
                            self.line(&format!("{} -> {}", t.from, t.to));
                        }
                        // Bare variants without transitions
                        for v in &block.variants {
                            let in_transition = block
                                .transitions
                                .iter()
                                .any(|t| &t.from == v || &t.to == v);
                            if !in_transition {
                                self.line(v);
                            }
                        }
                    } else {
                        for field in &block.fields {
                            self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
                        }
                    }
                    self.dedent();
                }
                for f in &c.fns {
                    let params = f
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, type_to_veil(&p.type_expr)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret = f
                        .return_type
                        .as_ref()
                        .map(|t| format!(" -> {}", type_to_veil(t)))
                        .unwrap_or_default();
                    self.line(&format!("fn {}({}){}", f.name, params, ret));
                    self.indent();
                    for ann in &f.annotations {
                        self.line(&format!("@{}", annotation_to_veil(ann)));
                    }
                    for expr in &f.body {
                        self.line(&expr_to_veil(expr));
                    }
                    self.dedent();
                }
                for child in &c.children {
                    self.emit_construct(child);
                }
                self.dedent();
            }
            Shape::Enum => {
                self.line(&format!("{}{} {}", export_prefix, c.keyword, c.name));
                self.indent();
                for t in &c.transitions {
                    self.line(&format!("{} -> {}", t.from, t.to));
                }
                for v in &c.variants {
                    let in_transition = c.transitions.iter().any(|t| &t.from == v || &t.to == v);
                    if !in_transition {
                        self.line(v);
                    }
                }
                self.dedent();
            }
            Shape::Trait => {
                self.line(&format!("{}{} {}", export_prefix, c.keyword, c.name));
                self.indent();
                for method in &c.methods {
                    let params = method
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, type_to_veil(&p.type_expr)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ret = method
                        .return_type
                        .as_ref()
                        .map(|t| format!(" -> {}", type_to_veil(t)))
                        .unwrap_or_default();
                    self.line(&format!("{}({}){}", method.name, params, ret));
                }
                self.dedent();
            }
            Shape::Impl => {
                let target = c.target.as_deref().unwrap_or("?");
                self.line(&format!("{}{} {} for {}", export_prefix, c.keyword, c.name, target));
                self.indent();
                for imp in &c.impls {
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
            Shape::Fn => {
                self.line(&format!("{}{} {}", export_prefix, c.keyword, c.name));
                self.indent();
                for r in &c.refs {
                    self.line(&format!("{} {}", r.keyword, r.values.join(", ")));
                }
                if !c.inputs.is_empty() {
                    self.line("input");
                    self.indent();
                    for field in &c.inputs {
                        self.line(&format!("{}: {}", field.name, type_to_veil(&field.type_expr)));
                    }
                    self.dedent();
                }
                for step in &c.steps {
                    self.emit_flow_step(step);
                }
                if let Some(ret) = &c.return_expr {
                    self.line(&format!("ret {}", expr_to_veil(ret)));
                }
                self.dedent();
            }
        }
    }

    // ─── Flow (core language) ─────────────────────────────────────────

    fn emit_flow(&mut self, flow: &Flow) {
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
            FlowStep::Step(s) => self.emit_step_def(s),
            FlowStep::Parallel(par) => {
                self.line("par");
                self.indent();
                for s in &par.steps {
                    self.emit_step_def(s);
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

    fn emit_step_def(&mut self, s: &StepDef) {
        self.line(&format!("step {}", s.name));
        self.indent();
        for r in &s.refs {
            self.line(&format!("{} {}", r.keyword, r.values.join(", ")));
        }
        for expr in &s.body {
            self.line(&expr_to_veil(expr));
        }
        for sb in &s.sub_blocks {
            self.line(&sb.keyword);
            self.indent();
            for expr in &sb.body {
                self.line(&expr_to_veil(expr));
            }
            self.dedent();
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
            let a = args.iter().map(type_to_veil).collect::<Vec<_>>().join(", ");
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
            let args = call.args.iter().map(expr_to_veil).collect::<Vec<_>>().join(", ");
            if call.method.is_empty() {
                format!("call {}({})", call.target, args)
            } else {
                format!("call {}.{}({})", call.target, call.method, args)
            }
        }
        Expr::Action(a) => match a.shape {
            StmtShape::Call => {
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
                            let vs = expr_to_veil(v);
                            if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}{{{}}}", head, fields)
                } else if !a.args.is_empty() {
                    let args = a.args.iter().map(expr_to_veil).collect::<Vec<_>>().join(", ");
                    format!("{}({})", head, args)
                } else {
                    head
                }
            }
            StmtShape::If => {
                let cond = a
                    .condition
                    .as_ref()
                    .map(|c| expr_to_veil(c))
                    .unwrap_or_default();
                if let Some(msg) = &a.message {
                    format!("{} {}, \"{}\"", a.keyword, cond, msg)
                } else {
                    format!("{} {}", a.keyword, cond)
                }
            }
        },
        Expr::Assign(name, rhs) => format!("{} = {}", name, expr_to_veil(rhs)),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_veil(inner)),
        Expr::BinaryOp(op) => {
            let op_str = match &op.op {
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
            };
            format!("{} {} {}", expr_to_veil(&op.left), op_str, expr_to_veil(&op.right))
        }
        Expr::UnaryOp(op) => {
            let op_str = match &op.op {
                UnaryOp::Not => "!",
                UnaryOp::Neg => "-",
            };
            format!("{}{}", op_str, expr_to_veil(&op.expr))
        }
        Expr::IfExpr(ie) => format!("if {}", expr_to_veil(&ie.condition)),
    }
}
