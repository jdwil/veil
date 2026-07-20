//! Escape-hatch debt diagnostics (CHK-006).
//!
//! Flags places where agents/authors leave the structured VEIL surface for
//! raw strings, empty adapter bodies (codegen `todo!`), unstubbed external
//! calls, or untyped `Json` at package boundaries.
//!
//! Default severity is **warning**. Callers may promote to **error** via
//! [`promote_escape_hatches`] when `--deny-escape-hatches` is set.

use std::collections::HashSet;

use crate::ast::*;
use crate::diagnostics::{Diagnostic, Severity};
use crate::layer::{LayerRegistry, Shape};
use crate::span::Span;

/// Diagnostic codes for escape-hatch debt (stable for filtering / metrics).
pub mod codes {
    pub const RAW_SURFACE: &str = "escape_raw_surface";
    pub const EMPTY_ADAPTER: &str = "escape_empty_adapter";
    pub const EXTERNAL_CALL: &str = "escape_external_call";
    pub const JSON_BOUNDARY: &str = "escape_json_boundary";
}

/// All escape-hatch diagnostic codes.
pub fn escape_codes() -> &'static [&'static str] {
    &[
        codes::RAW_SURFACE,
        codes::EMPTY_ADAPTER,
        codes::EXTERNAL_CALL,
        codes::JSON_BOUNDARY,
    ]
}

pub fn is_escape_hatch_code(code: &str) -> bool {
    escape_codes().contains(&code)
}

/// Counts of escape-hatch diagnostics by kind (for CLI summary).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EscapeHatchSummary {
    pub raw_surface: usize,
    pub empty_adapter: usize,
    pub external_call: usize,
    pub json_boundary: usize,
}

impl EscapeHatchSummary {
    pub fn total(&self) -> usize {
        self.raw_surface + self.empty_adapter + self.external_call + self.json_boundary
    }

    pub fn from_diagnostics(diags: &[Diagnostic]) -> Self {
        let mut s = Self::default();
        for d in diags {
            match d.code.as_str() {
                codes::RAW_SURFACE => s.raw_surface += 1,
                codes::EMPTY_ADAPTER => s.empty_adapter += 1,
                codes::EXTERNAL_CALL => s.external_call += 1,
                codes::JSON_BOUNDARY => s.json_boundary += 1,
                _ => {}
            }
        }
        s
    }

    /// One-line metric-friendly summary.
    pub fn format_line(&self) -> String {
        format!(
            "escape-hatch debt: {} total (raw={}, empty_adapter={}, external={}, json_boundary={})",
            self.total(),
            self.raw_surface,
            self.empty_adapter,
            self.external_call,
            self.json_boundary
        )
    }
}

// need Serialize for EscapeHatchSummary
use serde::Serialize;

/// Scan a solution for escape-hatch debt. All diagnostics are **warnings**.
pub fn check_escape_hatches(sol: &Solution, registry: &LayerRegistry) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let stub_names = stub_type_and_crate_names(registry);
    let construct_names = collect_construct_names(sol);
    let free_fns = collect_free_fn_names(sol);

    // Expose block = explicit package boundary
    if let Some(expose) = find_expose(sol) {
        for node in &expose.nodes {
            for f in node.inputs.iter().chain(node.outputs.iter()) {
                flag_json_type(
                    &f.type_expr,
                    &node.name,
                    "expose",
                    Some(f.span),
                    &mut diagnostics,
                );
            }
        }
    }

    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => {
                check_construct_escape(
                    c,
                    &stub_names,
                    &construct_names,
                    &free_fns,
                    true,
                    &mut diagnostics,
                );
            }
            TopLevelItem::Function(f) => {
                // Layer-provided coordinators (e.g. ddd `run_saga` / `unwind`) are
                // platform substrate — do not flag their Json state bag or internal
                // calls as package escape debt.
                if f.layer_provided {
                    continue;
                }
                let mut locals = HashSet::new();
                for p in &f.params {
                    locals.insert(p.name.clone());
                    flag_json_type(
                        &p.type_expr,
                        &f.name,
                        "fn param",
                        Some(p.span),
                        &mut diagnostics,
                    );
                }
                for e in &f.body {
                    check_expr_escape(
                        e,
                        &f.name,
                        &stub_names,
                        &construct_names,
                        &free_fns,
                        &mut locals,
                        &mut diagnostics,
                    );
                }
                if let Some(rt) = &f.return_type {
                    flag_json_type(rt, &f.name, "fn return", None, &mut diagnostics);
                }
            }
            TopLevelItem::Flow(flow) => {
                for f in &flow.inputs {
                    flag_json_type(
                        &f.type_expr,
                        &flow.name,
                        "flow input",
                        Some(f.span),
                        &mut diagnostics,
                    );
                }
                for step in &flow.steps {
                    check_flow_step_escape(
                        step,
                        &flow.name,
                        &stub_names,
                        &construct_names,
                        &free_fns,
                        &mut diagnostics,
                    );
                }
            }
            _ => {}
        }
    }

    diagnostics
}

/// Promote all escape-hatch warnings to errors (for `--deny-escape-hatches`).
pub fn promote_escape_hatches(diagnostics: &mut [Diagnostic]) {
    for d in diagnostics.iter_mut() {
        if is_escape_hatch_code(&d.code) {
            d.severity = Severity::Error;
        }
    }
}

fn find_expose(sol: &Solution) -> Option<&ExposeBlock> {
    sol.expose.as_ref()
}

fn collect_construct_names(sol: &Solution) -> HashSet<String> {
    let mut names = HashSet::new();
    fn walk(c: &Construct, names: &mut HashSet<String>) {
        names.insert(c.name.clone());
        for ch in &c.children {
            walk(ch, names);
        }
    }
    for item in &sol.items {
        if let TopLevelItem::Construct(c) = item {
            walk(c, &mut names);
        }
    }
    names
}

fn collect_free_fn_names(sol: &Solution) -> HashSet<String> {
    sol.items
        .iter()
        .filter_map(|item| match item {
            TopLevelItem::Function(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect()
}

fn stub_type_and_crate_names(registry: &LayerRegistry) -> HashSet<String> {
    let mut s = HashSet::new();
    for stub in &registry.stubs {
        s.insert(stub.name.clone());
        let crate_keys: Vec<String> = std::iter::once(stub.name.clone())
            .chain(stub.alias.iter().cloned())
            .collect();
        if let Some(a) = &stub.alias {
            s.insert(a.clone());
        }
        for st in &stub.structs {
            s.insert(st.name.clone());
            // Qualified paths used in source: `sqlx.Query.new(...)`
            for ck in &crate_keys {
                s.insert(format!("{}.{}", ck, st.name));
            }
        }
        for imp in &stub.impls {
            s.insert(imp.target.clone());
            for ck in &crate_keys {
                s.insert(format!("{}.{}", ck, imp.target));
            }
        }
    }
    s
}

fn check_construct_escape(
    c: &Construct,
    stub_names: &HashSet<String>,
    construct_names: &HashSet<String>,
    free_fns: &HashSet<String>,
    at_boundary: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Raw template/style (and any raw block)
    for (kw, content) in &c.raw_blocks {
        let preview = content.trim();
        let detail = if preview.is_empty() {
            format!("empty raw block '{}'", kw)
        } else {
            format!(
                "raw block '{}' ({} chars)",
                kw,
                content.len()
            )
        };
        diagnostics.push(debt(
            codes::RAW_SURFACE,
            format!(
                "raw surface on '{}': {} — prefer structured constructs when possible",
                c.name, detail
            ),
            &c.name,
            Some(c.span),
            Some("escape hatch: raw blocks are not graphically reviewable as VEIL structure".into()),
        ));
    }

    // Port / trait methods at package boundary — Json params/returns.
    // Skip layer-provided infrastructure (e.g. Bus from ddd.declare) to avoid noise;
    // user-authored ports still flag Json as boundary debt.
    if !c.layer_provided && (c.shape == Shape::Trait || c.exported) {
        for m in &c.methods {
            for p in &m.params {
                flag_json_type(
                    &p.type_expr,
                    &c.name,
                    &format!("port method {}.{}", c.name, m.name),
                    Some(p.span),
                    diagnostics,
                );
            }
            if let Some(rt) = &m.return_type {
                flag_json_type(
                    rt,
                    &c.name,
                    &format!("port method {}.{} return", c.name, m.name),
                    Some(m.span),
                    diagnostics,
                );
            }
        }
    }

    // Exported or top-level struct fields typed Json
    if c.exported || at_boundary {
        for f in &c.fields {
            flag_json_type(
                &f.type_expr,
                &c.name,
                &format!("field {}", f.name),
                Some(f.span),
                diagnostics,
            );
        }
        for inp in &c.inputs {
            flag_json_type(
                &inp.type_expr,
                &c.name,
                &format!("input {}", inp.name),
                Some(inp.span),
                diagnostics,
            );
        }
    }

    // Empty adapter / impl method bodies
    if c.shape == Shape::Impl {
        for imp in &c.impls {
            if imp.body.is_empty() {
                diagnostics.push(debt(
                    codes::EMPTY_ADAPTER,
                    format!(
                        "adapter '{}' method '{}' has empty body — codegen may emit todo!()",
                        c.name, imp.method_name
                    ),
                    &c.name,
                    Some(imp.span),
                    Some("implement the body or call a stubbed SDK with real logic".into()),
                ));
            }
            let mut locals = HashSet::new();
            for p in &imp.params {
                locals.insert(p.clone());
            }
            for e in &imp.body {
                check_expr_escape(e, &c.name, stub_names, construct_names, free_fns, &mut locals, diagnostics);
            }
        }
    }

    for fndef in &c.fns {
        let mut locals = HashSet::new();
        for p in &fndef.params {
            locals.insert(p.name.clone());
        }
        for e in &fndef.body {
            check_expr_escape(e, &c.name, stub_names, construct_names, free_fns, &mut locals, diagnostics);
        }
    }

    // Shared locals across steps so `user = …` in step A covers `user.x` in step B
    let mut flow_locals: HashSet<String> = c.inputs.iter().map(|i| i.name.clone()).collect();
    for step in &c.steps {
        check_flow_step_escape_mut(
            step,
            &c.name,
            stub_names,
            construct_names,
            free_fns,
            &mut flow_locals,
            diagnostics,
        );
    }
    if let Some(ret) = &c.return_expr {
        check_expr_escape(
            ret,
            &c.name,
            stub_names,
            construct_names,
            free_fns,
            &mut flow_locals,
            diagnostics,
        );
    }

    for child in &c.children {
        // Nested constructs are not package root boundary unless exported
        check_construct_escape(child, stub_names, construct_names, free_fns, false, diagnostics);
    }
}

fn check_flow_step_escape(
    step: &FlowStep,
    location: &str,
    stub_names: &HashSet<String>,
    construct_names: &HashSet<String>,
    free_fns: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut locals = HashSet::new();
    check_flow_step_escape_mut(
        step,
        location,
        stub_names,
        construct_names,
        free_fns,
        &mut locals,
        diagnostics,
    );
}

fn check_flow_step_escape_mut(
    step: &FlowStep,
    location: &str,
    stub_names: &HashSet<String>,
    construct_names: &HashSet<String>,
    free_fns: &HashSet<String>,
    locals: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match step {
        FlowStep::Step(sd) => {
            for e in &sd.body {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            for sb in &sd.sub_blocks {
                for e in &sb.body {
                    check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
                }
            }
        }
        FlowStep::Parallel(p) => {
            for s in &p.steps {
                check_flow_step_escape_mut(
                    &FlowStep::Step(s.clone()),
                    location,
                    stub_names,
                    construct_names,
                    free_fns,
                    locals,
                    diagnostics,
                );
            }
        }
        FlowStep::Match(m) => {
            check_expr_escape(&m.expr, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for arm in &m.arms {
                for e in &arm.body {
                    check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
                }
            }
        }
    }
}

fn check_expr_escape(
    expr: &Expr,
    location: &str,
    stub_names: &HashSet<String>,
    construct_names: &HashSet<String>,
    free_fns: &HashSet<String>,
    locals: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call(call) => {
            flag_external_call(
                call,
                location,
                stub_names,
                construct_names,
                free_fns,
                locals,
                diagnostics,
            );
            for a in &call.args {
                check_expr_escape(a, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            if let Some(r) = &call.receiver {
                check_expr_escape(r, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::Action(a) => {
            if !a.target.is_empty() {
                let fake = CallExpr {
                    target: a.target.clone(),
                    method: a.method.clone(),
                    args: Vec::new(),
                    receiver: None,
                    sugar: Some(a.keyword.clone()),
                    span: a.span,
                };
                flag_external_call(
                    &fake,
                    location,
                    stub_names,
                    construct_names,
                    free_fns,
                    locals,
                    diagnostics,
                );
            }
            for a in &a.args {
                check_expr_escape(a, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            for (_, e) in &a.named_args {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            if let Some(c) = &a.condition {
                check_expr_escape(c, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::Assign(name, e, _) | Expr::MutAssign(name, e, _) => {
            check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            locals.insert(name.clone());
        }
        Expr::Return(e) | Expr::Await(e) | Expr::Try(e) | Expr::FieldAccess(e, _)
        | Expr::Cast(e, _) | Expr::UnaryOp(UnaryOpExpr { expr: e, .. }) => {
            check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
        }
        Expr::LetPattern(pat, e, _) => {
            check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            bind_pattern_locals(pat, locals);
        }
        Expr::BinaryOp(op) => {
            check_expr_escape(&op.left, location, stub_names, construct_names, free_fns, locals, diagnostics);
            check_expr_escape(&op.right, location, stub_names, construct_names, free_fns, locals, diagnostics);
        }
        Expr::IfExpr(ie) => {
            check_expr_escape(&ie.condition, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for e in &ie.then_body {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            if let Some(eb) = &ie.else_body {
                for e in eb {
                    check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
                }
            }
        }
        Expr::Match(s, arms) => {
            check_expr_escape(s, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for arm in arms {
                for e in &arm.body {
                    check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
                }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            check_expr_escape(iterable, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for e in body {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::WhileLoop { condition, body } => {
            check_expr_escape(condition, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for e in body {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::Loop(body) => {
            for e in body {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::Closure { body, .. } => {
            for e in body {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::Tuple(items) | Expr::ArrayLit(items) => {
            for e in items {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::Index(a, b) => {
            check_expr_escape(a, location, stub_names, construct_names, free_fns, locals, diagnostics);
            check_expr_escape(b, location, stub_names, construct_names, free_fns, locals, diagnostics);
        }
        Expr::StructLit(_, fields) | Expr::StructUpdate { fields, .. } => {
            if let Expr::StructUpdate { base, .. } = expr {
                check_expr_escape(base, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            for (_, e) in fields {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
                }
            }
        }
        Expr::Range { start, end, .. } => {
            if let Some(s) = start {
                check_expr_escape(s, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            if let Some(e) = end {
                check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        Expr::IfLet {
            expr: e,
            then_body,
            else_body,
            ..
        } => {
            check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for x in then_body {
                check_expr_escape(x, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
            if let Some(eb) = else_body {
                for x in eb {
                    check_expr_escape(x, location, stub_names, construct_names, free_fns, locals, diagnostics);
                }
            }
        }
        Expr::WhileLet { expr: e, body, .. } => {
            check_expr_escape(e, location, stub_names, construct_names, free_fns, locals, diagnostics);
            for x in body {
                check_expr_escape(x, location, stub_names, construct_names, free_fns, locals, diagnostics);
            }
        }
        _ => {}
    }
}

fn flag_external_call(
    call: &CallExpr,
    location: &str,
    stub_names: &HashSet<String>,
    construct_names: &HashSet<String>,
    free_fns: &HashSet<String>,
    locals: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let target = call.target.as_str();
    if target.is_empty() || target == "self" || target.starts_with("self.") {
        return;
    }
    // Local binding (user.greet, repo.save after bind)
    if locals.contains(target) {
        return;
    }
    // Builtins
    let base = target.trim_end_matches('!');
    if matches!(
        base,
        "now" | "env" | "panic" | "todo" | "unreachable" | "assert"
    ) {
        return;
    }
    // Known construct
    if construct_names.contains(target) {
        return;
    }
    // Free function (layer-declared coordinators, package fns)
    if free_fns.contains(target) {
        return;
    }
    // Has stub coverage
    if stub_names.contains(target) {
        return;
    }
    // Capitalized unknown is unresolved_name (CHK-003), not escape debt
    if target
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        return;
    }
    // Lowercase external without stub — escape hatch
    diagnostics.push(debt(
        codes::EXTERNAL_CALL,
        format!(
            "external call '{}'{} — no construct or .stub; codegen cannot type this",
            target,
            if call.method.is_empty() {
                String::new()
            } else {
                format!(".{}", call.method)
            }
        ),
        location,
        Some(call.span),
        Some(format!(
            "add a .stub for '{}' or wrap the call in a typed adapter",
            target
        )),
    ));
}

fn bind_pattern_locals(pat: &Pattern, locals: &mut HashSet<String>) {
    match pat {
        Pattern::Ident(n) if n != "_" => {
            locals.insert(n.clone());
        }
        Pattern::Tuple(parts) | Pattern::Variant(_, parts) | Pattern::Or(parts) => {
            for p in parts {
                bind_pattern_locals(p, locals);
            }
        }
        Pattern::Struct(_, fields, _) => {
            for (name, inner) in fields {
                if let Some(p) = inner {
                    bind_pattern_locals(p, locals);
                } else {
                    locals.insert(name.clone());
                }
            }
        }
        _ => {}
    }
}

fn flag_json_type(
    ty: &TypeExpr,
    location: &str,
    context: &str,
    span: Option<Span>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if type_contains_json(ty) {
        diagnostics.push(debt(
            codes::JSON_BOUNDARY,
            format!(
                "untyped Json at boundary ({}) on '{}' — prefer a named type",
                context, location
            ),
            location,
            span,
            Some("Json hides structure from check/review; introduce a domain type or DTO".into()),
        ));
    }
}

fn type_contains_json(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Named(n) if n == "Json" || n == "serde_json::Value" => true,
        TypeExpr::Generic(_, args) => args.iter().any(type_contains_json),
        TypeExpr::Optional(t)
        | TypeExpr::List(t)
        | TypeExpr::Set(t)
        | TypeExpr::Result(Some(t))
        | TypeExpr::Ref(t, _)
        | TypeExpr::Dyn(t)
        | TypeExpr::ImplTrait(t)
        | TypeExpr::Array(t, _) => type_contains_json(t),
        TypeExpr::Map(k, v) => type_contains_json(k) || type_contains_json(v),
        TypeExpr::Tuple(items) => items.iter().any(type_contains_json),
        TypeExpr::FnPtr(args, ret) => {
            args.iter().any(type_contains_json)
                || ret.as_ref().map(|t| type_contains_json(t)).unwrap_or(false)
        }
        TypeExpr::Result(None) => false,
        TypeExpr::Named(_) => false,
    }
}

fn debt(
    code: &str,
    message: String,
    location: &str,
    span: Option<Span>,
    hint: Option<String>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        message,
        node_id: None,
        node_name: Some(location.to_string()),
        code: code.to_string(),
        constraint: code.to_string(),
        parent: None,
        hint,
        span_start: span.map(|s| s.start),
        span_end: span.map(|s| s.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{ConstructSpec, Visual};
    use crate::span::Span;

    fn empty_visual() -> Visual {
        Visual {
            icon: String::new(),
            color: String::new(),
            label: String::new(),
        }
    }

    fn spec(kw: &str, name: &str, shape: Shape) -> ConstructSpec {
        ConstructSpec {
            name: name.to_string(),
            keyword: kw.to_string(),
            maps_to: shape.name().to_string(),
            shape,
            layer: "test".to_string(),
            desc: String::new(),
            contains: Vec::new(),
            blocks: Vec::new(),
            raw_block_keywords: Vec::new(),
            constraints: Vec::new(),
            allowed_in: "any".to_string(),
            group: String::new(),
            visual: empty_visual(),
            au: false,
                is_step: false,
                step_fields: Vec::new(),
            annotations: Vec::new(),
            runtime: None,
            tgt: String::new(),
            dg: String::new(),
            presentation: Default::default(),
        }
    }

    fn reg() -> LayerRegistry {
        let mut r = LayerRegistry::builtin();
        for s in [
            spec("comp", "Component", Shape::Struct),
            spec("adapter", "Adapter", Shape::Impl),
            spec("port", "Port", Shape::Trait),
            spec("svc", "Service", Shape::Fn),
        ] {
            if let Some(i) = r.constructs.iter().position(|c| c.keyword == s.keyword) {
                r.constructs[i] = s;
            } else {
                r.constructs.push(s);
            }
        }
        r
    }

    fn sol(items: Vec<TopLevelItem>) -> Solution {
        Solution {
            name: "T".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            links: vec![],
            items,
            expose: None,
        }
    }

    #[test]
    fn flags_raw_template() {
        let mut c = Construct::new(
            "comp",
            "Component",
            Shape::Struct,
            "Card".into(),
            Span::new(0, 10),
        );
        c.raw_blocks
            .push(("template".into(), "<div>hi</div>".into()));
        let diags = check_escape_hatches(&sol(vec![TopLevelItem::Construct(c)]), &reg());
        assert!(
            diags.iter().any(|d| d.code == codes::RAW_SURFACE),
            "{:?}",
            diags
        );
    }

    #[test]
    fn flags_empty_adapter() {
        let mut c = Construct::new(
            "adapter",
            "Adapter",
            Shape::Impl,
            "Pg".into(),
            Span::new(0, 0),
        );
        c.target = Some("Repo".into());
        c.impls.push(MethodImpl {
            method_name: "save".into(),
            params: vec!["x".into()],
            span: Span::new(1, 5),
            body: Vec::new(),
        });
        let diags = check_escape_hatches(&sol(vec![TopLevelItem::Construct(c)]), &reg());
        assert!(
            diags.iter().any(|d| d.code == codes::EMPTY_ADAPTER),
            "{:?}",
            diags
        );
    }

    #[test]
    fn flags_external_http_call() {
        let mut svc = Construct::new("svc", "Service", Shape::Fn, "S".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(StepDef {
            name: "go".into(),
            span: Span::new(0, 0),
            body: vec![Expr::Call(CallExpr {
                target: "http".into(),
                method: "post".into(),
                args: Vec::new(),
                receiver: None,
                sugar: None,
                span: Span::new(2, 8),
            })],
            refs: Vec::new(),
            sub_blocks: Vec::new(), kind: None, fields: Vec::new(), edges: Vec::new(),
        }));
        let diags = check_escape_hatches(&sol(vec![TopLevelItem::Construct(svc)]), &reg());
        assert!(
            diags.iter().any(|d| d.code == codes::EXTERNAL_CALL && d.message.contains("http")),
            "{:?}",
            diags
        );
    }

    #[test]
    fn flags_json_on_port() {
        let mut port = Construct::new("port", "Port", Shape::Trait, "Bus".into(), Span::new(0, 0));
        port.methods.push(Method {
            name: "dispatch".into(),
            span: Span::new(0, 0),
            params: vec![Param {
                name: "evt".into(),
                type_expr: TypeExpr::Named("Json".into()),
                span: Span::new(0, 0),
            }],
            return_type: None,
        });
        let diags = check_escape_hatches(&sol(vec![TopLevelItem::Construct(port)]), &reg());
        assert!(
            diags.iter().any(|d| d.code == codes::JSON_BOUNDARY),
            "{:?}",
            diags
        );
    }

    #[test]
    fn promote_turns_warnings_into_errors() {
        let mut c = Construct::new(
            "comp",
            "Component",
            Shape::Struct,
            "Card".into(),
            Span::new(0, 0),
        );
        c.raw_blocks.push(("style".into(), ".x{}".into()));
        let mut diags = check_escape_hatches(&sol(vec![TopLevelItem::Construct(c)]), &reg());
        assert!(diags.iter().all(|d| matches!(d.severity, Severity::Warning)));
        promote_escape_hatches(&mut diags);
        assert!(diags.iter().all(|d| matches!(d.severity, Severity::Error)));
    }

    #[test]
    fn summary_counts() {
        let diags = vec![
            debt(codes::RAW_SURFACE, "a".into(), "x", None, None),
            debt(codes::RAW_SURFACE, "b".into(), "x", None, None),
            debt(codes::JSON_BOUNDARY, "c".into(), "x", None, None),
        ];
        let s = EscapeHatchSummary::from_diagnostics(&diags);
        assert_eq!(s.raw_surface, 2);
        assert_eq!(s.json_boundary, 1);
        assert_eq!(s.total(), 3);
        assert!(
            s.format_line().contains("3 total") || s.format_line().contains("total 3"),
            "{}",
            s.format_line()
        );
    }
}
