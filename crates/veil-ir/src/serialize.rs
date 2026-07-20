//! VEIL Serializer — emits valid .veil text from AST.
//!
//! This is the inverse of the parser: shape-driven, zero domain knowledge.
//! Each construct is emitted according to its core shape, using the keyword
//! recorded at parse time.
//!
//! # Canonical format (SER-003)
//!
//! | Rule | Choice |
//! |------|--------|
//! | Indent | 2 spaces per level |
//! | Package keyword | `pkg` (not deprecated `sol`) |
//! | Export | `+` prefix (not `export `) |
//! | Calls | bare `Target.method(args)` — never the `call` keyword |
//! | Item order | AST / source order (no alphabetical sort) |
//! | Annotations | Source order; one `@name` or `@name(args)` per line before the item |
//! | Blank lines | One blank between emitted top-level items; none for skipped layer-provided |
//! | Typed assigns | `name: Type = expr` / `mut name: Type = expr` |
//! | Newline | File ends with a single `\n` |
//!
//! Span-preserving partial rewrite is not implemented; full re-emit is canonical.
//! See `docs/SERIALIZE.md`.

use crate::ast::*;
use crate::layer::{Shape, StmtShape};

/// Serialize a Solution AST into VEIL source text (canonical form).
pub fn serialize_solution(sol: &Solution) -> String {
    let mut s = Serializer::new();
    s.emit_solution(sol);
    s.finish()
}

/// Serialize a Package AST into VEIL source text (canonical form).
pub fn serialize_package(pkg: &Package) -> String {
    let mut s = Serializer::new();
    s.emit_package(pkg);
    s.finish()
}

/// Serialize a Composition AST into VEIL source text (canonical form).
pub fn serialize_composition(comp: &Composition) -> String {
    let mut s = Serializer::new();
    s.emit_composition(comp);
    s.finish()
}

/// Whether a top-level item produces any serialized output.
fn item_emits(item: &TopLevelItem) -> bool {
    match item {
        TopLevelItem::Construct(c) if c.layer_provided => false,
        TopLevelItem::Function(f) if f.layer_provided => false,
        _ => true,
    }
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

    /// Finalize: strip trailing blank lines; ensure single trailing newline.
    fn finish(mut self) -> String {
        while self.output.ends_with("\n\n") {
            self.output.pop();
        }
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output
    }

    /// Emit top-level items with one blank between **emitted** items only.
    fn emit_items_spaced(&mut self, items: &[TopLevelItem]) {
        let mut first = true;
        for item in items {
            if !item_emits(item) {
                continue;
            }
            if !first {
                self.blank();
            }
            first = false;
            self.emit_top_level_item(item);
        }
    }

    // ─── Solution / Package / Composition ────────────────────────────

    fn emit_solution(&mut self, sol: &Solution) {
        // Canonical keyword is `pkg` (`sol` is a deprecated parse alias).
        self.line(&format!("pkg {}", sol.name));
        self.indent();
        for u in &sol.uses {
            match &u.alias {
                Some(alias) => self.line(&format!("use {} as {}", u.package_name, alias)),
                None => self.line(&format!("use {}", u.package_name)),
            }
        }
        for link in &sol.links {
            self.emit_link(link);
        }
        if !sol.uses.is_empty() || !sol.links.is_empty() {
            self.blank();
        }
        self.emit_items_spaced(&sol.items);
        if let Some(expose) = &sol.expose {
            if sol.items.iter().any(item_emits) {
                self.blank();
            }
            self.emit_expose(expose);
        }
        self.dedent();
    }

    fn emit_link(&mut self, link: &LinkDecl) {
        let mut line = format!("link {}", link.name);
        if let Some(path) = &link.path {
            line.push_str(&format!(" path \"{}\"", path));
        }
        if !link.features.is_empty() {
            line.push_str(&format!(" features \"{}\"", link.features.join(", ")));
        }
        self.line(&line);
    }

    fn emit_top_level_item(&mut self, item: &TopLevelItem) {
        match item {
            TopLevelItem::Lang(lang) => self.emit_lang(lang),
            // Layer-provided constructs (e.g. the injected Bus port) are not
            // part of the user's source and must not be written back.
            TopLevelItem::Construct(c) if c.layer_provided => {}
            TopLevelItem::Construct(c) => self.emit_construct(c),
            TopLevelItem::Flow(flow) => self.emit_flow(flow),
            TopLevelItem::TypeAlias { name, target } => self.line(&format!("type {} = {}", name, type_to_veil(target))),
            TopLevelItem::Const { name, value } => self.line(&format!("const {} = {}", name, expr_to_veil(value))),
            TopLevelItem::Static { name, mutable, value } => { let m = if *mutable { "mut " } else { "" }; self.line(&format!("static {}{} = {}", m, name, expr_to_veil(value))); }
            // Layer-provided functions (declared coordinators) are not user source.
            TopLevelItem::Function(f) if f.layer_provided => {}
            TopLevelItem::Function(f) => self.emit_function(f),
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

        for u in &pkg.uses {
            match &u.alias {
                Some(alias) => self.line(&format!("use {} as {}", u.package_name, alias)),
                None => self.line(&format!("use {}", u.package_name)),
            }
        }
        for link in &pkg.links {
            self.emit_link(link);
        }
        for a in &pkg.adapts {
            self.line(&format!("adapt {}", a.package_name));
        }
        if !pkg.uses.is_empty() || !pkg.links.is_empty() || !pkg.adapts.is_empty() {
            self.blank();
        }

        for patch in &pkg.patches {
            self.emit_adapt_patch(patch);
            self.blank();
        }

        self.emit_items_spaced(&pkg.items);

        if let Some(expose) = &pkg.expose {
            if pkg.items.iter().any(item_emits)
                || !pkg.metadata.is_empty()
                || !pkg.uses.is_empty()
                || !pkg.links.is_empty()
                || !pkg.adapts.is_empty()
                || !pkg.patches.is_empty()
            {
                self.blank();
            }
            self.emit_expose(expose);
        }

        self.dedent();
    }

    fn emit_adapt_patch(&mut self, patch: &AdaptPatch) {
        match patch {
            AdaptPatch::Ins {
                path,
                items,
                ..
            } => {
                self.line(&format!("ins {}", path.display()));
                self.indent();
                for item in items {
                    match item {
                        AdaptInsItem::Step { step, position } => {
                            if let FlowStep::Step(sd) = step {
                                let mut head = if sd.name.is_empty() {
                                    "step".to_string()
                                } else {
                                    format!("step {}", sd.name)
                                };
                                match position {
                                    StepPosition::AtEnd => {}
                                    StepPosition::AtStart => head.push_str(" at start"),
                                    StepPosition::Before(n) => {
                                        head.push_str(&format!(" before {n}"));
                                    }
                                    StepPosition::After(n) => {
                                        head.push_str(&format!(" after {n}"));
                                    }
                                }
                                self.line(&head);
                                self.indent();
                                for e in &sd.body {
                                    self.line(&expr_to_veil(e));
                                }
                                self.dedent();
                            }
                        }
                        AdaptInsItem::Function(f) => {
                            self.emit_function(f);
                        }
                        AdaptInsItem::Construct(c) => {
                            self.emit_construct(c);
                        }
                    }
                }
                self.dedent();
            }
            AdaptPatch::Rfn { path, steps, body, .. } => {
                self.line(&format!("rfn {}", path.display()));
                self.indent();
                self.emit_adapt_body(steps, body);
                self.dedent();
            }
            AdaptPatch::Rpl { path, steps, body, .. } => {
                self.line(&format!("rpl {}", path.display()));
                self.indent();
                self.emit_adapt_body(steps, body);
                self.dedent();
            }
            AdaptPatch::Omit { path, .. } => {
                self.line(&format!("omit {}", path.display()));
            }
            AdaptPatch::Ren {
                path, new_name, ..
            } => {
                self.line(&format!("ren {} {}", path.display(), new_name));
            }
        }
    }

    fn emit_adapt_body(&mut self, steps: &[FlowStep], body: &[Expr]) {
        for st in steps {
            if let FlowStep::Step(sd) = st {
                let head = if sd.name.is_empty() {
                    "step".to_string()
                } else {
                    format!("step {}", sd.name)
                };
                self.line(&head);
                self.indent();
                for e in &sd.body {
                    self.line(&expr_to_veil(e));
                }
                self.dedent();
            }
        }
        for e in body {
            self.line(&expr_to_veil(e));
        }
    }

    fn emit_composition(&mut self, comp: &Composition) {
        for imp in &comp.imports {
            if let Some(alias) = &imp.alias {
                self.line(&format!("use {} as {}", imp.package_name, alias));
            } else {
                self.line(&format!("use {}", imp.package_name));
            }
        }
        if !comp.imports.is_empty() && !comp.flows.is_empty() {
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

    /// Emit a field with leading annotations and optional default expression.
    ///
    /// Canonical form (SER-001):
    /// ```text
    /// @dep
    /// @env(FOO)
    /// name: Type = default_expr
    /// ```
    fn emit_field(&mut self, field: &Field) {
        for ann in &field.annotations {
            if !ann.name.starts_with("__") {
                self.line(&format!("@{}", annotation_to_veil(ann)));
            }
        }
        let mut line = format!("{}: {}", field.name, type_to_veil(&field.type_expr));
        if let Some(def) = &field.default_expr {
            line.push_str(" = ");
            line.push_str(&expr_to_veil(def));
        }
        self.line(&line);
    }

    fn emit_construct(&mut self, c: &Construct) {
        for ann in &c.annotations {
            if !ann.name.starts_with("__") {
                self.line(&format!("@{}", annotation_to_veil(ann)));
            }
        }
        // Canonical export mark is `+` (LANGUAGE.md token-efficiency form).
        let export_prefix = if c.exported { "+" } else { "" };

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
                    self.emit_field(field);
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
                            self.emit_field(field);
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
                        self.emit_expr(expr);
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
                        self.emit_expr(expr);
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
                        self.emit_field(field);
                    }
                    self.dedent();
                }
                for step in &c.steps {
                    self.emit_flow_step(step);
                }
                if let Some(rt) = &c.return_type {
                    self.line(&format!("-> {}", type_to_veil(rt)));
                }
                if let Some(ret) = &c.return_expr {
                    self.line(&format!("ret {}", expr_to_veil(ret)));
                }
                self.dedent();
            }
        }
    }

    // ─── Flow (core language) ─────────────────────────────────────────

    fn emit_function(&mut self, f: &FnDef) {
        for ann in &f.annotations {
            self.line(&format!("@{}", annotation_to_veil(ann)));
        }
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
        for step in &f.steps {
            self.emit_flow_step(step);
        }
        for expr in &f.body {
            self.emit_expr(expr);
        }
        self.dedent();
    }

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
                self.emit_field(field);
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
                self.emit_match_expr(&m.expr, &m.arms);
            }
        }
    }

    fn emit_step_def(&mut self, s: &StepDef) {
        // Typed step: emit `<kind> <name>` instead of `step <name>`.
        let header = if let Some(kind) = &s.kind {
            format!("{} {}", kind, s.name)
        } else {
            format!("step {}", s.name)
        };
        self.line(&header);
        self.indent();
        // Typed step config fields: `key: value`
        for f in &s.fields {
            self.line(&format!("{}: {}", f.name, f.value));
        }
        // Edge routing: `on <label>: <target>`
        for e in &s.edges {
            self.line(&format!("on {}: {}", e.label, e.target));
        }
        for r in &s.refs {
            self.line(&format!("{} {}", r.keyword, r.values.join(", ")));
        }
        for expr in &s.body {
            self.emit_expr(expr);
        }
        for sb in &s.sub_blocks {
            self.line(&sb.keyword);
            self.indent();
            for expr in &sb.body {
                self.emit_expr(expr);
            }
            self.dedent();
        }
        self.dedent();
    }

    /// Emit an expression, expanding multi-line control flow with proper indent.
    ///
    /// SER-002: no `"..."` placeholders for supported control-flow forms.
    fn emit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::IfExpr(ie) => {
                self.line(&format!("if {}", expr_to_veil(&ie.condition)));
                self.indent();
                for e in &ie.then_body {
                    self.emit_expr(e);
                }
                self.dedent();
                if let Some(else_body) = &ie.else_body {
                    // Collapse `else` + single nested if into `else if`
                    if else_body.len() == 1 {
                        if let Expr::IfExpr(nested) = &else_body[0] {
                            self.line(&format!("else if {}", expr_to_veil(&nested.condition)));
                            self.indent();
                            for e in &nested.then_body {
                                self.emit_expr(e);
                            }
                            self.dedent();
                            if let Some(eb) = &nested.else_body {
                                self.line("else");
                                self.indent();
                                for e in eb {
                                    self.emit_expr(e);
                                }
                                self.dedent();
                            }
                            return;
                        }
                    }
                    self.line("else");
                    self.indent();
                    for e in else_body {
                        self.emit_expr(e);
                    }
                    self.dedent();
                }
            }
            Expr::IfLet {
                pattern,
                expr: scrut,
                then_body,
                else_body,
            } => {
                self.line(&format!("if let {} = {}", pattern, expr_to_veil(scrut)));
                self.indent();
                for e in then_body {
                    self.emit_expr(e);
                }
                self.dedent();
                if let Some(eb) = else_body {
                    self.line("else");
                    self.indent();
                    for e in eb {
                        self.emit_expr(e);
                    }
                    self.dedent();
                }
            }
            Expr::WhileLet {
                pattern,
                expr: scrut,
                body,
            } => {
                self.line(&format!("while let {} = {}", pattern, expr_to_veil(scrut)));
                self.indent();
                for e in body {
                    self.emit_expr(e);
                }
                self.dedent();
            }
            Expr::WhileLoop { condition, body } => {
                self.line(&format!("while {}", expr_to_veil(condition)));
                self.indent();
                for e in body {
                    self.emit_expr(e);
                }
                self.dedent();
            }
            Expr::Loop(body) => {
                self.line("loop");
                self.indent();
                for e in body {
                    self.emit_expr(e);
                }
                self.dedent();
            }
            Expr::ForLoop {
                binding,
                index,
                iterable,
                body,
            } => {
                let idx = index
                    .as_ref()
                    .map(|i| format!("{}, ", i))
                    .unwrap_or_default();
                self.line(&format!(
                    "for {}{} in {}",
                    idx,
                    binding,
                    expr_to_veil(iterable)
                ));
                self.indent();
                for e in body {
                    self.emit_expr(e);
                }
                self.dedent();
            }
            Expr::Match(scrutinee, arms) => {
                self.emit_match_expr(scrutinee, arms);
            }
            Expr::Closure { params, body } => {
                // Multi-line closure body when more than one statement
                if body.len() <= 1 {
                    self.line(&expr_to_veil(expr));
                } else {
                    let p = params.join(", ");
                    self.line(&format!("|{}|", p));
                    self.indent();
                    for e in body {
                        self.emit_expr(e);
                    }
                    self.dedent();
                }
            }
            // Single-line forms
            other => self.line(&expr_to_veil(other)),
        }
    }

    fn emit_match_expr(&mut self, scrutinee: &Expr, arms: &[MatchArm]) {
        self.line(&format!("match {}", expr_to_veil(scrutinee)));
        self.indent();
        for arm in arms {
            let pat = arm
                .rich_pattern
                .as_ref()
                .map(|p| p.to_string_repr())
                .unwrap_or_else(|| arm.pattern.clone());
            let head = if let Some(g) = &arm.guard {
                format!("{} if {} ->", pat, expr_to_veil(g))
            } else {
                format!("{} ->", pat)
            };
            if arm.body.len() == 1 && !expr_is_multiline(&arm.body[0]) {
                // Single simple expression on the same conceptual arm
                self.line(&format!("{} {}", head, expr_to_veil(&arm.body[0])));
            } else {
                self.line(&head);
                self.indent();
                for expr in &arm.body {
                    self.emit_expr(expr);
                }
                self.dedent();
            }
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
                self.emit_field(f);
            }
            self.dedent();
        }
        if !node.outputs.is_empty() {
            self.line("output");
            self.indent();
            for f in &node.outputs {
                self.emit_field(f);
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
        TypeExpr::Tuple(items) => {
            let parts = items.iter().map(type_to_veil).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        TypeExpr::Array(inner, size) => format!("[{}; {}]", type_to_veil(inner), size),
        TypeExpr::Ref(inner, is_mut) => if *is_mut { format!("&mut {}", type_to_veil(inner)) } else { format!("&{}", type_to_veil(inner)) },
        TypeExpr::Dyn(inner) => format!("dyn {}", type_to_veil(inner)),
        TypeExpr::ImplTrait(inner) => format!("impl {}", type_to_veil(inner)),
        TypeExpr::FnPtr(params, ret) => { let p = params.iter().map(type_to_veil).collect::<Vec<_>>().join(", "); let r = ret.as_ref().map(|t| format!(" -> {}", type_to_veil(t))).unwrap_or_default(); format!("fn({}){}", p, r) }
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
        Expr::Stock => "stock".to_string(),
        Expr::Ident(name) => name.clone(),
        Expr::FieldAccess(base, field) => format!("{}.{}", expr_to_veil(base), field),
        Expr::Call(call) => {
            let args = call.args.iter().map(expr_to_veil).collect::<Vec<_>>().join(", ");
            // Preserve the original statement sugar (e.g. `dispatch Evt{...}`)
            // for round-trip fidelity when this call was desugared from a
            // layer statement.
            if let Some(kw) = &call.sugar {
                if let Some(Expr::StructLit(name, fields)) = call.args.first() {
                    let display_name = name.replace("::", ".");
                    let field_str = fields.iter()
                        .map(|(k, v)| {
                            let vs = expr_to_veil(v);
                            if k == &vs { k.clone() } else { format!("{}: {}", k, vs) }
                        })
                        .collect::<Vec<_>>().join(", ");
                    return format!("{} {}{{{}}}", kw, display_name, field_str);
                }
                if let Some(Expr::Ident(evt)) = call.args.first() {
                    return format!("{} {}", kw, evt);
                }
                return format!("{} {}({})", kw, call.target, args);
            }
            // Canonical form: bare invocation (no `call` keyword) — LANGUAGE.md.
            // Emitting `call` causes reparse churn (`call call …`).
            if let Some(recv) = &call.receiver {
                format!("{}.{}({})", expr_to_veil(recv), call.method, args)
            } else if call.method.is_empty() {
                format!("{}({})", call.target, args)
            } else {
                format!("{}.{}({})", call.target, call.method, args)
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
        Expr::Assign(name, rhs, ty) => {
            if let Some(t) = ty {
                format!("{}: {} = {}", name, type_to_veil(t), expr_to_veil(rhs))
            } else {
                format!("{} = {}", name, expr_to_veil(rhs))
            }
        }
        Expr::MutAssign(name, rhs, ty) => {
            if let Some(t) = ty {
                format!("mut {}: {} = {}", name, type_to_veil(t), expr_to_veil(rhs))
            } else {
                format!("mut {} = {}", name, expr_to_veil(rhs))
            }
        }
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::IntLit(n) => n.to_string(),
        Expr::FloatLit(f) => f.to_string(),
        Expr::BoolLit(b) => b.to_string(),
        Expr::Return(inner) => format!("ret {}", expr_to_veil(inner)),
        Expr::Await(inner) => format!("await {}", expr_to_veil(inner)),
        Expr::Break => "break".to_string(),
        Expr::Continue => "continue".to_string(),
        Expr::Index(base, idx) => format!("{}[{}]", expr_to_veil(base), expr_to_veil(idx)),
        Expr::ArrayLit(items) => {
            let s = items.iter().map(expr_to_veil).collect::<Vec<_>>().join(", ");
            format!("[{}]", s)
        }
        Expr::Range {
            start,
            end,
            inclusive,
        } => {
            let s = start
                .as_ref()
                .map(|e| expr_to_veil(e))
                .unwrap_or_default();
            let e = end.as_ref().map(|e| expr_to_veil(e)).unwrap_or_default();
            let op = if *inclusive { "..=" } else { ".." };
            format!("{}{}{}", s, op, e)
        }
        // Multi-line control flow: compact single-line fallback for embedding
        // (prefer `emit_expr` for top-level statements so indent is correct).
        Expr::Loop(body) => {
            if body.is_empty() {
                "loop".to_string()
            } else if body.len() == 1 && !expr_is_multiline(&body[0]) {
                format!("loop\n  {}", expr_to_veil(&body[0]))
            } else {
                let b = body.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
                format!("loop {{ {} }}", b)
            }
        }
        Expr::Cast(expr, ty) => format!("{} as {}", expr_to_veil(expr), ty),
        Expr::Try(expr) => format!("{}?", expr_to_veil(expr)),
        Expr::StructUpdate { name, fields, base } => {
            let fs = fields
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_veil(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} {{ {}, ..{} }}", name, fs, expr_to_veil(base))
        }
        Expr::IfLet {
            pattern,
            expr,
            then_body,
            else_body,
        } => {
            let e = expr_to_veil(expr);
            let then_s = then_body
                .iter()
                .map(expr_to_veil)
                .collect::<Vec<_>>()
                .join("; ");
            if let Some(eb) = else_body {
                let else_s = eb.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
                format!("if let {} = {} {{ {} }} else {{ {} }}", pattern, e, then_s, else_s)
            } else {
                format!("if let {} = {} {{ {} }}", pattern, e, then_s)
            }
        }
        Expr::WhileLet {
            pattern,
            expr,
            body,
        } => {
            let e = expr_to_veil(expr);
            let b = body.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
            format!("while let {} = {} {{ {} }}", pattern, e, b)
        }
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
            format!(
                "{} {} {}",
                expr_to_veil(&op.left),
                op_str,
                expr_to_veil(&op.right)
            )
        }
        Expr::UnaryOp(op) => {
            let op_str = match &op.op {
                UnaryOp::Not => "!",
                UnaryOp::Neg => "-",
            };
            format!("{}{}", op_str, expr_to_veil(&op.expr))
        }
        Expr::IfExpr(ie) => {
            let then_s = ie
                .then_body
                .iter()
                .map(expr_to_veil)
                .collect::<Vec<_>>()
                .join("; ");
            if let Some(eb) = &ie.else_body {
                let else_s = eb.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
                format!(
                    "if {} {{ {} }} else {{ {} }}",
                    expr_to_veil(&ie.condition),
                    then_s,
                    else_s
                )
            } else {
                format!("if {} {{ {} }}", expr_to_veil(&ie.condition), then_s)
            }
        }
        Expr::StructLit(name, fields) => {
            // Parser stores enum variants as `Enum::Variant`; canonical surface
            // form is `Enum.Variant{...}` so re-lex/parse stays idempotent
            // (there is no `::` token in the lexer).
            let display_name = name.replace("::", ".");
            let fs = fields
                .iter()
                .map(|(k, v)| {
                    let v_str = expr_to_veil(v);
                    if k == &v_str {
                        k.clone()
                    } else {
                        format!("{}: {}", k, v_str)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}{{{}}}", display_name, fs)
        }
        Expr::Match(scrutinee, arms) => {
            let mut s = format!("match {}", expr_to_veil(scrutinee));
            for arm in arms {
                let pat = arm
                    .rich_pattern
                    .as_ref()
                    .map(|p| p.to_string_repr())
                    .unwrap_or_else(|| arm.pattern.clone());
                let body_str = arm
                    .body
                    .iter()
                    .map(expr_to_veil)
                    .collect::<Vec<_>>()
                    .join("; ");
                if let Some(g) = &arm.guard {
                    s.push_str(&format!(
                        "\n  {} if {} -> {}",
                        pat,
                        expr_to_veil(g),
                        body_str
                    ));
                } else {
                    s.push_str(&format!("\n  {} -> {}", pat, body_str));
                }
            }
            s
        }
        Expr::ForLoop {
            binding,
            index,
            iterable,
            body,
        } => {
            let idx = index
                .as_ref()
                .map(|i| format!("{}, ", i))
                .unwrap_or_default();
            let body_str = body.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
            format!(
                "for {}{} in {} {{ {} }}",
                idx,
                binding,
                expr_to_veil(iterable),
                body_str
            )
        }
        Expr::WhileLoop { condition, body } => {
            let body_str = body.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
            format!("while {} {{ {} }}", expr_to_veil(condition), body_str)
        }
        Expr::Tuple(items) => {
            let parts = items.iter().map(expr_to_veil).collect::<Vec<_>>().join(", ");
            format!("({})", parts)
        }
        Expr::StringInterp(parts) => {
            use crate::ast::StringPart;
            let s: String = parts
                .iter()
                .map(|p| match p {
                    StringPart::Literal(l) => l.clone(),
                    StringPart::Expr(e) => format!("{{{}}}", expr_to_veil(e)),
                })
                .collect();
            format!("f\"{}\"", s)
        }
        Expr::Closure { params, body } => {
            let p = params.join(", ");
            let b = body.iter().map(expr_to_veil).collect::<Vec<_>>().join("; ");
            format!("|{}| {}", p, b)
        }
        Expr::LetPattern(pattern, expr, ty) => {
            if let Some(t) = ty {
                format!(
                    "let {}: {} = {}",
                    pattern.to_string_repr(),
                    type_to_veil(t),
                    expr_to_veil(expr)
                )
            } else {
                format!("let {} = {}", pattern.to_string_repr(), expr_to_veil(expr))
            }
        }
    }
}

/// Whether an expression should force a multi-line arm/body layout.
fn expr_is_multiline(expr: &Expr) -> bool {
    match expr {
        Expr::IfExpr(_)
        | Expr::IfLet { .. }
        | Expr::WhileLet { .. }
        | Expr::WhileLoop { .. }
        | Expr::Loop(_)
        | Expr::ForLoop { .. }
        | Expr::Match(_, _) => true,
        Expr::Closure { body, .. } => body.len() > 1,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Shape;
    use crate::span::Span;

    fn field(name: &str, ty: &str) -> Field {
        Field {
            annotations: Vec::new(),
            name: name.into(),
            type_expr: TypeExpr::Named(ty.into()),
            default_expr: None,
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn emits_field_annotations() {
        let mut f = field("pool", "Pool");
        f.annotations.push(Annotation {
            name: "dep".into(),
            args: Vec::new(),
            span: Span::new(0, 0),
        });
        f.annotations.push(Annotation {
            name: "env".into(),
            args: vec!["DATABASE_URL".into()],
            span: Span::new(0, 0),
        });
        let mut c = Construct::new("struct", "Struct", Shape::Struct, "Repo".into(), Span::new(0, 0));
        c.fields.push(f);
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(c)],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(
            out.contains("@dep\n") || out.contains("@dep\r\n"),
            "missing @dep:\n{}",
            out
        );
        assert!(
            out.contains("@env(DATABASE_URL)"),
            "missing @env:\n{}",
            out
        );
        assert!(
            out.contains("pool: Pool"),
            "missing field:\n{}",
            out
        );
    }

    #[test]
    fn emits_field_default_expr() {
        let mut f = field("count", "Int");
        f.default_expr = Some(Expr::IntLit(0));
        let mut c = Construct::new("struct", "Struct", Shape::Struct, "Stats".into(), Span::new(0, 0));
        c.fields.push(f);
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(c)],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(
            out.contains("count: Int = 0"),
            "missing default:\n{}",
            out
        );
    }

    #[test]
    fn emits_input_annotations() {
        let mut f = field("repo", "UserRepo");
        f.annotations.push(Annotation {
            name: "dep".into(),
            args: Vec::new(),
            span: Span::new(0, 0),
        });
        let mut c = Construct::new("svc", "Service", Shape::Fn, "Create".into(), Span::new(0, 0));
        c.inputs.push(f);
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(c)],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(out.contains("input"), "missing input block:\n{}", out);
        assert!(out.contains("@dep"), "missing @dep on input:\n{}", out);
        assert!(out.contains("repo: UserRepo"), "missing input field:\n{}", out);
    }

    #[test]
    fn does_not_emit_internal_annotations() {
        let mut f = field("x", "Str");
        f.annotations.push(Annotation {
            name: "__internal".into(),
            args: Vec::new(),
            span: Span::new(0, 0),
        });
        let mut c = Construct::new("struct", "Struct", Shape::Struct, "S".into(), Span::new(0, 0));
        c.fields.push(f);
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(c)],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(!out.contains("__internal"), "leaked internal ann:\n{}", out);
        assert!(out.contains("x: Str"), "{}", out);
    }

    fn fn_with_body(body: Vec<Expr>) -> Solution {
        let mut c = Construct::new("fn", "Fn", Shape::Fn, "F".into(), Span::new(0, 0));
        c.steps.push(FlowStep::Step(StepDef {
            name: "s".into(),
            span: Span::new(0, 0),
            body,
            refs: Vec::new(),
            sub_blocks: Vec::new(), kind: None, fields: Vec::new(), edges: Vec::new(),
        }));
        Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(c)],
            expose: None,
        }
    }

    /// SER-002: no `"..."` placeholders for control-flow bodies.
    #[test]
    fn control_flow_emits_full_bodies_not_placeholders() {
        let out = serialize_solution(&fn_with_body(vec![
            Expr::IfExpr(IfExprData {
                condition: Box::new(Expr::BoolLit(true)),
                then_body: vec![Expr::Assign(
                    "x".into(),
                    Box::new(Expr::IntLit(1)),
                    None,
                )],
                else_body: Some(vec![Expr::Assign(
                    "x".into(),
                    Box::new(Expr::IntLit(0)),
                    None,
                )]),
            }),
            Expr::IfLet {
                pattern: "Some(v)".into(),
                expr: Box::new(Expr::Ident("opt".into())),
                then_body: vec![Expr::Ident("v".into())],
                else_body: None,
            },
            Expr::WhileLet {
                pattern: "Some(v)".into(),
                expr: Box::new(Expr::Ident("it".into())),
                body: vec![Expr::Call(CallExpr {
                    target: "use".into(),
                    method: String::new(),
                    args: vec![Expr::Ident("v".into())],
                    receiver: None,
                    sugar: None,
                    span: Span::new(0, 0),
                })],
            },
            Expr::WhileLoop {
                condition: Box::new(Expr::BoolLit(true)),
                body: vec![Expr::Break],
            },
            Expr::Loop(vec![Expr::Break]),
            Expr::ForLoop {
                binding: "i".into(),
                index: None,
                iterable: Box::new(Expr::Ident("items".into())),
                body: vec![Expr::Ident("i".into())],
            },
            Expr::Match(
                Box::new(Expr::Ident("x".into())),
                vec![
                    MatchArm {
                        pattern: "A".into(),
                        rich_pattern: Some(Pattern::Ident("A".into())),
                        guard: Some(Expr::BinaryOp(BinaryOpExpr {
                            left: Box::new(Expr::Ident("n".into())),
                            op: BinOp::Gt,
                            right: Box::new(Expr::IntLit(0)),
                        })),
                        span: Span::new(0, 0),
                        body: vec![Expr::IntLit(1)],
                    },
                    MatchArm {
                        pattern: "_".into(),
                        rich_pattern: Some(Pattern::Wildcard),
                        guard: None,
                        span: Span::new(0, 0),
                        body: vec![
                            Expr::Assign("a".into(), Box::new(Expr::IntLit(0)), None),
                            Expr::Assign("b".into(), Box::new(Expr::IntLit(1)), None),
                        ],
                    },
                ],
            ),
            Expr::Closure {
                params: vec!["x".into()],
                body: vec![
                    Expr::Assign("y".into(), Box::new(Expr::Ident("x".into())), None),
                    Expr::Return(Box::new(Expr::Ident("y".into()))),
                ],
            },
        ]));

        assert!(
            !out.contains("..."),
            "placeholder still present:\n{}",
            out
        );
        assert!(out.contains("if true"), "missing if:\n{}", out);
        assert!(out.contains("else"), "missing else:\n{}", out);
        assert!(out.contains("x = 1"), "missing then body:\n{}", out);
        assert!(out.contains("if let Some(v) = opt"), "missing if let:\n{}", out);
        assert!(out.contains("while let Some(v) = it"), "missing while let:\n{}", out);
        assert!(out.contains("while true"), "missing while:\n{}", out);
        assert!(out.contains("loop"), "missing loop:\n{}", out);
        assert!(out.contains("for i in items"), "missing for:\n{}", out);
        assert!(out.contains("match x"), "missing match:\n{}", out);
        assert!(
            out.contains("A if n > 0 ->"),
            "missing match guard:\n{}",
            out
        );
        assert!(out.contains("|x|"), "missing closure:\n{}", out);
    }

    #[test]
    fn mut_assign_emits_type_annotation() {
        let out = serialize_solution(&fn_with_body(vec![Expr::MutAssign(
            "n".into(),
            Box::new(Expr::IntLit(0)),
            Some(TypeExpr::Named("Int".into())),
        )]));
        assert!(
            out.contains("mut n: Int = 0"),
            "missing typed mut:\n{}",
            out
        );
    }

    #[test]
    fn assign_emits_type_annotation() {
        let out = serialize_solution(&fn_with_body(vec![Expr::Assign(
            "cohort".into(),
            Box::new(Expr::Ident("dto".into())),
            Some(TypeExpr::Named("CohortDTO".into())),
        )]));
        assert!(
            out.contains("cohort: CohortDTO = dto"),
            "missing typed assign:\n{}",
            out
        );
    }

    #[test]
    fn enum_variant_struct_lit_uses_dot_not_pathsep() {
        // AST stores Enum::Variant; surface form must use `.` (lexer has no `::`).
        let out = serialize_solution(&fn_with_body(vec![Expr::Assign(
            "response".into(),
            Box::new(Expr::StructLit(
                "OutgoingMessage::AgentResponse".into(),
                vec![("message".into(), Expr::StringLit("hi".into()))],
            )),
            None,
        )]));
        assert!(
            out.contains("OutgoingMessage.AgentResponse{message: \"hi\"}"),
            "expected Enum.Variant form:\n{}",
            out
        );
        assert!(
            !out.contains("OutgoingMessage::"),
            "must not emit :: pathsep:\n{}",
            out
        );
    }

    #[test]
    fn canonical_pkg_keyword_not_sol() {
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: Vec::new(),
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(out.starts_with("pkg App\n"), "expected pkg:\n{}", out);
        assert!(!out.starts_with("sol "), "must not emit sol:\n{}", out);
    }

    #[test]
    fn canonical_export_is_plus() {
        let mut c = Construct::new("struct", "Struct", Shape::Struct, "S".into(), Span::new(0, 0));
        c.exported = true;
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![TopLevelItem::Construct(c)],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(out.contains("+struct S") || out.contains("+struct  S"), "expected +export:\n{}", out);
        assert!(!out.contains("export "), "must not emit export keyword:\n{}", out);
    }

    #[test]
    fn calls_emit_without_call_keyword() {
        let out = serialize_solution(&fn_with_body(vec![Expr::Call(CallExpr {
            target: "Repo".into(),
            method: "save".into(),
            args: vec![Expr::Ident("u".into())],
            receiver: None,
            sugar: None,
            span: Span::new(0, 0),
        })]));
        assert!(
            out.contains("Repo.save(u)"),
            "bare call missing:\n{}",
            out
        );
        assert!(
            !out.contains("call Repo"),
            "must not emit call keyword:\n{}",
            out
        );
    }

    #[test]
    fn no_trailing_blank_lines() {
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: vec![],

            links: vec![],
items: vec![TopLevelItem::Construct(Construct::new(
                "struct",
                "Struct",
                Shape::Struct,
                "S".into(),
                Span::new(0, 0),
            ))],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(out.ends_with('\n'), "missing final newline");
        assert!(!out.ends_with("\n\n"), "trailing blank:\n{:?}", out);
    }

    #[test]
    fn layer_provided_skipped_without_extra_blank() {
        let user = Construct::new("struct", "Struct", Shape::Struct, "User".into(), Span::new(0, 0));
        let mut bus = Construct::new("trait", "Trait", Shape::Trait, "Bus".into(), Span::new(0, 0));
        bus.layer_provided = true;
        let other = Construct::new("struct", "Struct", Shape::Struct, "Other".into(), Span::new(0, 0));
        let sol = Solution {
            name: "App".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),

            links: vec![],
items: vec![
                TopLevelItem::Construct(user),
                TopLevelItem::Construct(bus),
                TopLevelItem::Construct(other),
            ],
            expose: None,
        };
        let out = serialize_solution(&sol);
        assert!(!out.contains("Bus"), "layer-provided leaked:\n{}", out);
        // Package body is indented; exactly one blank between emitted items
        // (not two for the skipped Bus).
        assert!(
            out.contains("struct User\n\n  struct Other"),
            "expected single blank between items:\n{}",
            out.replace(' ', "·")
        );
        assert!(
            !out.contains("struct User\n\n\n  struct Other"),
            "extra blank left by skipped layer-provided:\n{}",
            out.replace(' ', "·")
        );
    }
}
