//! Target capability matrix (CHK-005).
//!
//! Each codegen backend declares which language features it supports honestly.
//! Features that currently lower to placeholders (`todo!`, `/* range */`, empty
//! service bodies) are **unsupported** for that target and fail check when
//! `-t <target>` is selected (or warn as multi-target debt on the default Rust check).
//!
//! The engine has no domain knowledge — capabilities are about **expression and
//! construct shape features**, not DDD vocabulary.

use std::collections::HashSet;

use veil_ir::ast::*;
use veil_ir::diagnostics::{Diagnostic, Severity};
use veil_ir::layer::{LayerRegistry, Shape};
use veil_ir::span::Span;

use crate::CodegenTarget;

/// A language / IR feature that backends may or may not support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// `start..end` / `..=` range expressions
    RangeExpr,
    /// Infinite `loop { ... }`
    InfiniteLoop,
    /// `break` / `continue`
    BreakContinue,
    /// Closures `|params| body`
    Closures,
    /// `while let` / `if let`
    PatternWhileIfLet,
    /// Struct update `Name { field, ..base }`
    StructUpdate,
    /// `static` items
    StaticItems,
    /// Trait object types `dyn Trait` / `List<Trait>`
    TraitObjects,
    /// Impl-shaped constructs (adapters)
    ImplBlocks,
    /// Fn-shaped service/flow with no step body (TS emits empty TODO)
    EmptyServiceBody,
    /// Component/page with empty `template` raw block (Svelte placeholder)
    EmptyUiTemplate,
    /// Adapter/impl method with empty body (Rust may emit `todo!`)
    EmptyAdapterBody,
    /// Try operator `expr?`
    TryOperator,
    /// Await expressions
    AwaitExpr,
    /// Match expressions
    MatchExpr,
    /// Raw string blocks (`template`, `style`)
    RawBlocks,
}

impl Feature {
    pub fn id(self) -> &'static str {
        match self {
            Feature::RangeExpr => "range_expr",
            Feature::InfiniteLoop => "infinite_loop",
            Feature::BreakContinue => "break_continue",
            Feature::Closures => "closures",
            Feature::PatternWhileIfLet => "pattern_while_if_let",
            Feature::StructUpdate => "struct_update",
            Feature::StaticItems => "static_items",
            Feature::TraitObjects => "trait_objects",
            Feature::ImplBlocks => "impl_blocks",
            Feature::EmptyServiceBody => "empty_service_body",
            Feature::EmptyUiTemplate => "empty_ui_template",
            Feature::EmptyAdapterBody => "empty_adapter_body",
            Feature::TryOperator => "try_operator",
            Feature::AwaitExpr => "await_expr",
            Feature::MatchExpr => "match_expr",
            Feature::RawBlocks => "raw_blocks",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Feature::RangeExpr => "range expressions (start..end)",
            Feature::InfiniteLoop => "infinite loop expressions",
            Feature::BreakContinue => "break/continue",
            Feature::Closures => "closures",
            Feature::PatternWhileIfLet => "if-let / while-let",
            Feature::StructUpdate => "struct update syntax (..base)",
            Feature::StaticItems => "static items",
            Feature::TraitObjects => "trait objects (dyn Trait / List<Trait>)",
            Feature::ImplBlocks => "impl / adapter blocks",
            Feature::EmptyServiceBody => "fn-shaped construct with empty body (TS emits TODO)",
            Feature::EmptyUiTemplate => "UI construct with empty template (placeholder markup)",
            Feature::EmptyAdapterBody => "adapter method with empty body (may emit todo!)",
            Feature::TryOperator => "try operator (?)",
            Feature::AwaitExpr => "await expressions",
            Feature::MatchExpr => "match expressions",
            Feature::RawBlocks => "raw template/style blocks",
        }
    }
}

/// Features currently supported by each target.
/// Anything not listed is treated as unsupported when used.
fn supported_features(target: CodegenTarget) -> HashSet<Feature> {
    use Feature::*;
    match target {
        CodegenTarget::Rust => HashSet::from([
            RangeExpr,
            InfiniteLoop,
            BreakContinue,
            Closures,
            PatternWhileIfLet,
            StructUpdate,
            StaticItems,
            TraitObjects,
            ImplBlocks,
            // EmptyServiceBody / EmptyUiTemplate / EmptyAdapterBody intentionally
            // NOT fully supported — gated as incomplete lowering
            TryOperator,
            AwaitExpr,
            MatchExpr,
            RawBlocks, // ignored / passed through in adapters
            // Empty bodies: Rust supports non-empty; empty adapter body is debt
        ]),
        CodegenTarget::TypeScript => HashSet::from([
            // RangeExpr: NOT supported (placeholder)
            InfiniteLoop, // while(true) ok
            BreakContinue,
            Closures,
            PatternWhileIfLet, // partial but not placeholder
            StructUpdate,      // spread
            // StaticItems: weak
            // TraitObjects: interfaces only
            // ImplBlocks: partial interfaces
            TryOperator, // mapped awkwardly
            AwaitExpr,
            MatchExpr,
            RawBlocks, // Svelte templates
            // EmptyServiceBody NOT supported
            // EmptyUiTemplate NOT supported as complete
            // EmptyAdapterBody N/A-ish
        ]),
        // PAR-005/006 spikes: intentionally sparse — most features fail closed.
        CodegenTarget::Swift => crate::swift::swift_supported_features(),
        CodegenTarget::Kotlin => crate::kotlin::kotlin_supported_features(),
    }
}

fn target_name(target: CodegenTarget) -> &'static str {
    match target {
        CodegenTarget::Rust => "rust",
        CodegenTarget::TypeScript => "typescript",
        CodegenTarget::Swift => "swift",
        CodegenTarget::Kotlin => "kotlin",
    }
}

/// A feature occurrence found in the solution.
#[derive(Debug, Clone)]
struct FeatureUse {
    feature: Feature,
    location: String,
    span: Option<Span>,
    detail: Option<String>,
}

/// Check solution against a single target's capability matrix.
///
/// Unsupported features used in the AST become **errors** for that target.
pub fn check_target_capabilities(
    sol: &Solution,
    _registry: &LayerRegistry,
    target: CodegenTarget,
) -> Vec<Diagnostic> {
    let used = collect_feature_uses(sol);
    let supported = supported_features(target);
    let tname = target_name(target);

    let mut diagnostics = Vec::new();
    for u in used {
        if supported.contains(&u.feature) {
            // Special cases still incomplete even when "feature" is known
            continue;
        }
        // Incomplete-lowering features
        if matches!(
            u.feature,
            Feature::EmptyServiceBody | Feature::EmptyUiTemplate | Feature::EmptyAdapterBody
        ) {
            // Always gate these for the target that would emit placeholders
            if target == CodegenTarget::TypeScript
                && matches!(
                    u.feature,
                    Feature::EmptyServiceBody | Feature::EmptyUiTemplate
                )
            {
                diagnostics.push(capability_diag(
                    Severity::Error,
                    &u,
                    tname,
                    "codegen would emit a TODO/placeholder",
                ));
            } else if target == CodegenTarget::Rust
                && matches!(u.feature, Feature::EmptyAdapterBody)
            {
                diagnostics.push(capability_diag(
                    Severity::Warning,
                    &u,
                    tname,
                    "codegen may emit todo!() for empty adapter bodies",
                ));
            }
            continue;
        }

        diagnostics.push(capability_diag(
            Severity::Error,
            &u,
            tname,
            "not supported by this target",
        ));
    }
    diagnostics
}

/// Features that TypeScript cannot lower honestly — used for multi-target debt
/// warnings when checking the primary (Rust) target.
fn typescript_hard_gaps() -> HashSet<Feature> {
    use Feature::*;
    HashSet::from([
        RangeExpr,
        EmptyServiceBody,
        EmptyUiTemplate,
        StaticItems,
        TraitObjects,
        ImplBlocks, // full adapter impl fidelity
    ])
}

/// When checking Rust (default), warn about uses that would fail TS check.
pub fn check_multi_target_debt(sol: &Solution, _registry: &LayerRegistry) -> Vec<Diagnostic> {
    let used = collect_feature_uses(sol);
    let ts_gaps = typescript_hard_gaps();
    let mut diagnostics = Vec::new();
    for u in used {
        if ts_gaps.contains(&u.feature) {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!(
                    "feature '{}' ({}) is not honestly supported by target typescript",
                    u.feature.id(),
                    u.feature.description()
                ),
                node_id: None,
                node_name: Some(u.location.clone()),
                code: "capability_debt".to_string(),
                constraint: "capability_debt".to_string(),
                parent: None,
                hint: Some(format!(
                    "run `veil check -t ts` for errors; detail: {}",
                    u.detail.as_deref().unwrap_or("—")
                )),
                span_start: u.span.map(|s| s.start),
                span_end: u.span.map(|s| s.end),
            });
        }
    }
    diagnostics
}

fn capability_diag(severity: Severity, u: &FeatureUse, tname: &str, reason: &str) -> Diagnostic {
    Diagnostic {
        severity,
        message: format!(
            "target '{}': {} — {} ({})",
            tname,
            u.feature.description(),
            reason,
            u.feature.id()
        ),
        node_id: None,
        node_name: Some(u.location.clone()),
        code: format!("unsupported_{}", u.feature.id()),
        constraint: format!("unsupported_{}", u.feature.id()),
        parent: None,
        hint: u.detail.clone().or_else(|| {
            Some(format!(
                "remove or rewrite this construct for {}, or use a different -t target",
                tname
            ))
        }),
        span_start: u.span.map(|s| s.start),
        span_end: u.span.map(|s| s.end),
    }
}

// ─── Collect uses ────────────────────────────────────────────────────────────

fn collect_feature_uses(sol: &Solution) -> Vec<FeatureUse> {
    let mut uses = Vec::new();
    for item in &sol.items {
        match item {
            TopLevelItem::Construct(c) => collect_construct(c, &mut uses),
            TopLevelItem::Function(f) => {
                for e in &f.body {
                    collect_expr(e, &f.name, &mut uses);
                }
            }
            TopLevelItem::Flow(flow) => {
                for step in &flow.steps {
                    collect_flow_step(step, &flow.name, &mut uses);
                }
            }
            TopLevelItem::Static { .. } => {
                uses.push(FeatureUse {
                    feature: Feature::StaticItems,
                    location: sol.name.clone(),
                    span: None,
                    detail: Some("static item at package level".into()),
                });
            }
            _ => {}
        }
    }
    uses
}

fn collect_construct(c: &Construct, uses: &mut Vec<FeatureUse>) {
    if c.shape == Shape::Impl {
        uses.push(FeatureUse {
            feature: Feature::ImplBlocks,
            location: c.name.clone(),
            span: Some(c.span),
            detail: c.target.clone().map(|t| format!("impl for {}", t)),
        });
        for imp in &c.impls {
            if imp.body.is_empty() {
                uses.push(FeatureUse {
                    feature: Feature::EmptyAdapterBody,
                    location: c.name.clone(),
                    span: Some(imp.span),
                    detail: Some(format!("empty impl {}", imp.method_name)),
                });
            }
            for e in &imp.body {
                collect_expr(e, &c.name, uses);
            }
        }
    }

    // Fn-shaped services: empty steps + no nested fns with body content
    if c.shape == Shape::Fn && !c.layer_provided {
        let has_step_body = c.steps.iter().any(|s| match s {
            FlowStep::Step(sd) => !sd.body.is_empty(),
            FlowStep::Parallel(p) => p.steps.iter().any(|sd| !sd.body.is_empty()),
            FlowStep::Match(m) => m.arms.iter().any(|a| !a.body.is_empty()),
        });
        let has_fn_body = c.fns.iter().any(|f| !f.body.is_empty());
        if !has_step_body && !has_fn_body && c.return_expr.is_none() {
            // Only flag if it looks like a service (has name, maybe inputs)
            uses.push(FeatureUse {
                feature: Feature::EmptyServiceBody,
                location: c.name.clone(),
                span: Some(c.span),
                detail: Some("no steps or expression body".into()),
            });
        }
        for step in &c.steps {
            collect_flow_step(step, &c.name, uses);
        }
        if let Some(ret) = &c.return_expr {
            collect_expr(ret, &c.name, uses);
        }
    }

    // UI raw blocks
    if !c.raw_blocks.is_empty() {
        uses.push(FeatureUse {
            feature: Feature::RawBlocks,
            location: c.name.clone(),
            span: Some(c.span),
            detail: Some(
                c.raw_blocks
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        });
        let has_template = c
            .raw_blocks
            .iter()
            .any(|(k, v)| k == "template" && !v.trim().is_empty());
        let is_ui = c
            .raw_blocks
            .iter()
            .any(|(k, _)| k == "template" || k == "style")
            || matches!(c.subkind.as_str(), "Component" | "Page" | "Layout");
        if is_ui && !has_template {
            uses.push(FeatureUse {
                feature: Feature::EmptyUiTemplate,
                location: c.name.clone(),
                span: Some(c.span),
                detail: Some("empty or missing template block".into()),
            });
        }
    }

    for f in &c.fields {
        collect_type(&f.type_expr, &c.name, uses);
    }
    for m in &c.methods {
        for p in &m.params {
            collect_type(&p.type_expr, &c.name, uses);
        }
        if let Some(rt) = &m.return_type {
            collect_type(rt, &c.name, uses);
        }
    }
    for fndef in &c.fns {
        for e in &fndef.body {
            collect_expr(e, &c.name, uses);
        }
    }
    for child in &c.children {
        collect_construct(child, uses);
    }
}

fn collect_flow_step(step: &FlowStep, location: &str, uses: &mut Vec<FeatureUse>) {
    match step {
        FlowStep::Step(sd) => {
            for e in &sd.body {
                collect_expr(e, location, uses);
            }
            for sb in &sd.sub_blocks {
                for e in &sb.body {
                    collect_expr(e, location, uses);
                }
            }
        }
        FlowStep::Parallel(p) => {
            for s in &p.steps {
                collect_flow_step(&FlowStep::Step(s.clone()), location, uses);
            }
        }
        FlowStep::Match(m) => {
            uses.push(FeatureUse {
                feature: Feature::MatchExpr,
                location: location.into(),
                span: Some(m.span),
                detail: None,
            });
            collect_expr(&m.expr, location, uses);
            for arm in &m.arms {
                for e in &arm.body {
                    collect_expr(e, location, uses);
                }
            }
        }
    }
}

fn collect_expr(expr: &Expr, location: &str, uses: &mut Vec<FeatureUse>) {
    match expr {
        Expr::Range { start, end, .. } => {
            uses.push(FeatureUse {
                feature: Feature::RangeExpr,
                location: location.into(),
                span: None,
                detail: Some("range expression lowers to placeholder in TypeScript".into()),
            });
            if let Some(s) = start {
                collect_expr(s, location, uses);
            }
            if let Some(e) = end {
                collect_expr(e, location, uses);
            }
        }
        Expr::Loop(body) => {
            uses.push(FeatureUse {
                feature: Feature::InfiniteLoop,
                location: location.into(),
                span: None,
                detail: None,
            });
            for e in body {
                collect_expr(e, location, uses);
            }
        }
        Expr::Break | Expr::Continue => {
            uses.push(FeatureUse {
                feature: Feature::BreakContinue,
                location: location.into(),
                span: None,
                detail: None,
            });
        }
        Expr::Closure { body, .. } => {
            uses.push(FeatureUse {
                feature: Feature::Closures,
                location: location.into(),
                span: None,
                detail: None,
            });
            for e in body {
                collect_expr(e, location, uses);
            }
        }
        Expr::IfLet {
            expr: e,
            then_body,
            else_body,
            ..
        } => {
            uses.push(FeatureUse {
                feature: Feature::PatternWhileIfLet,
                location: location.into(),
                span: None,
                detail: Some("if let".into()),
            });
            collect_expr(e, location, uses);
            for x in then_body {
                collect_expr(x, location, uses);
            }
            if let Some(eb) = else_body {
                for x in eb {
                    collect_expr(x, location, uses);
                }
            }
        }
        Expr::WhileLet {
            expr: e, body, ..
        } => {
            uses.push(FeatureUse {
                feature: Feature::PatternWhileIfLet,
                location: location.into(),
                span: None,
                detail: Some("while let".into()),
            });
            collect_expr(e, location, uses);
            for x in body {
                collect_expr(x, location, uses);
            }
        }
        Expr::StructUpdate { fields, base, .. } => {
            uses.push(FeatureUse {
                feature: Feature::StructUpdate,
                location: location.into(),
                span: None,
                detail: None,
            });
            collect_expr(base, location, uses);
            for (_, e) in fields {
                collect_expr(e, location, uses);
            }
        }
        Expr::Try(e) => {
            uses.push(FeatureUse {
                feature: Feature::TryOperator,
                location: location.into(),
                span: None,
                detail: None,
            });
            collect_expr(e, location, uses);
        }
        Expr::Await(e) => {
            uses.push(FeatureUse {
                feature: Feature::AwaitExpr,
                location: location.into(),
                span: None,
                detail: None,
            });
            collect_expr(e, location, uses);
        }
        Expr::Match(scrut, arms) => {
            uses.push(FeatureUse {
                feature: Feature::MatchExpr,
                location: location.into(),
                span: None,
                detail: None,
            });
            collect_expr(scrut, location, uses);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_expr(g, location, uses);
                }
                for e in &arm.body {
                    collect_expr(e, location, uses);
                }
            }
        }
        Expr::Call(call) => {
            for a in &call.args {
                collect_expr(a, location, uses);
            }
            if let Some(r) = &call.receiver {
                collect_expr(r, location, uses);
            }
        }
        Expr::Action(a) => {
            for a in &a.args {
                collect_expr(a, location, uses);
            }
            for (_, e) in &a.named_args {
                collect_expr(e, location, uses);
            }
            if let Some(c) = &a.condition {
                collect_expr(c, location, uses);
            }
        }
        Expr::Assign(_, e, _) | Expr::MutAssign(_, e, _) | Expr::Return(e) | Expr::FieldAccess(e, _) => {
            collect_expr(e, location, uses);
        }
        Expr::LetPattern(_, e, _) => collect_expr(e, location, uses),
        Expr::BinaryOp(op) => {
            collect_expr(&op.left, location, uses);
            collect_expr(&op.right, location, uses);
        }
        Expr::UnaryOp(op) => collect_expr(&op.expr, location, uses),
        Expr::IfExpr(ie) => {
            collect_expr(&ie.condition, location, uses);
            for e in &ie.then_body {
                collect_expr(e, location, uses);
            }
            if let Some(eb) = &ie.else_body {
                for e in eb {
                    collect_expr(e, location, uses);
                }
            }
        }
        Expr::ForLoop { iterable, body, .. } => {
            collect_expr(iterable, location, uses);
            for e in body {
                collect_expr(e, location, uses);
            }
        }
        Expr::WhileLoop { condition, body } => {
            collect_expr(condition, location, uses);
            for e in body {
                collect_expr(e, location, uses);
            }
        }
        Expr::Tuple(items) | Expr::ArrayLit(items) => {
            for e in items {
                collect_expr(e, location, uses);
            }
        }
        Expr::Index(a, b) => {
            collect_expr(a, location, uses);
            collect_expr(b, location, uses);
        }
        Expr::StructLit(_, fields) => {
            for (_, e) in fields {
                collect_expr(e, location, uses);
            }
        }
        Expr::StringInterp(parts) => {
            for p in parts {
                if let StringPart::Expr(e) = p {
                    collect_expr(e, location, uses);
                }
            }
        }
        Expr::Cast(e, _) => collect_expr(e, location, uses),
        _ => {}
    }
}

fn collect_type(ty: &TypeExpr, location: &str, uses: &mut Vec<FeatureUse>) {
    match ty {
        TypeExpr::Dyn(_) => {
            uses.push(FeatureUse {
                feature: Feature::TraitObjects,
                location: location.into(),
                span: None,
                detail: Some("dyn Trait".into()),
            });
        }
        TypeExpr::List(inner) | TypeExpr::Optional(inner) | TypeExpr::Set(inner)
        | TypeExpr::Ref(inner, _) | TypeExpr::ImplTrait(inner) | TypeExpr::Array(inner, _) => {
            // List<Trait> heuristic: Named capital trait-like inside List — hard without registry
            collect_type(inner, location, uses);
        }
        TypeExpr::Map(k, v) => {
            collect_type(k, location, uses);
            collect_type(v, location, uses);
        }
        TypeExpr::Generic(name, args) if name == "List" || name == "Vec" => {
            for a in args {
                if let TypeExpr::Named(n) = a {
                    // Heuristic: trait object list often List<Bus> etc. — skip false positives
                    let _ = n;
                }
                collect_type(a, location, uses);
            }
        }
        TypeExpr::Generic(_, args) => {
            for a in args {
                collect_type(a, location, uses);
            }
        }
        TypeExpr::Tuple(items) => {
            for t in items {
                collect_type(t, location, uses);
            }
        }
        TypeExpr::Result(Some(t)) | TypeExpr::FnPtr(_, Some(t)) => collect_type(t, location, uses),
        TypeExpr::FnPtr(args, None) => {
            for a in args {
                collect_type(a, location, uses);
            }
        }
        _ => {}
    }
}

/// Human-readable summary of a target's gaps (for docs / CLI help).
pub fn target_capability_summary(target: CodegenTarget) -> String {
    let supported = supported_features(target);
    let all = [
        Feature::RangeExpr,
        Feature::InfiniteLoop,
        Feature::BreakContinue,
        Feature::Closures,
        Feature::PatternWhileIfLet,
        Feature::StructUpdate,
        Feature::StaticItems,
        Feature::TraitObjects,
        Feature::ImplBlocks,
        Feature::EmptyServiceBody,
        Feature::EmptyUiTemplate,
        Feature::EmptyAdapterBody,
        Feature::TryOperator,
        Feature::AwaitExpr,
        Feature::MatchExpr,
        Feature::RawBlocks,
    ];
    let mut lines = vec![format!("target: {}", target_name(target))];
    for f in all {
        let status = if supported.contains(&f) { "ok" } else { "GAP" };
        // Empty* special
        let status = match (target, f) {
            (CodegenTarget::TypeScript, Feature::EmptyServiceBody) => "GAP",
            (CodegenTarget::TypeScript, Feature::EmptyUiTemplate) => "GAP",
            (CodegenTarget::Rust, Feature::EmptyAdapterBody) => "warn",
            _ => status,
        };
        lines.push(format!("  [{}] {} — {}", status, f.id(), f.description()));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use veil_ir::layer::LayerRegistry;
    use veil_ir::span::Span;

    fn sol_with(c: Construct) -> Solution {
        Solution {
            name: "T".into(),
            span: Span::new(0, 0),
            uses: Vec::new(),
            items: vec![TopLevelItem::Construct(c)],
            expose: None,
        }
    }

    #[test]
    fn ts_rejects_range_expr() {
        let mut svc = Construct::new("fn", "Fn", Shape::Fn, "Walk".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(StepDef {
            name: "s".into(),
            span: Span::new(0, 0),
            body: vec![Expr::ForLoop {
                binding: "i".into(),
                index: None,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::IntLit(0))),
                    end: Some(Box::new(Expr::IntLit(10))),
                    inclusive: false,
                }),
                body: vec![],
            }],
            refs: Vec::new(),
            sub_blocks: Vec::new(),
        }));
        let reg = LayerRegistry::builtin();
        let diags = check_target_capabilities(&sol_with(svc), &reg, CodegenTarget::TypeScript);
        assert!(
            diags.iter().any(|d| d.code.contains("range_expr")),
            "{:?}",
            diags
        );
    }

    #[test]
    fn rust_allows_range_expr() {
        let mut svc = Construct::new("fn", "Fn", Shape::Fn, "Walk".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(StepDef {
            name: "s".into(),
            span: Span::new(0, 0),
            body: vec![Expr::Range {
                start: Some(Box::new(Expr::IntLit(0))),
                end: Some(Box::new(Expr::IntLit(3))),
                inclusive: true,
            }],
            refs: Vec::new(),
            sub_blocks: Vec::new(),
        }));
        let reg = LayerRegistry::builtin();
        let diags = check_target_capabilities(&sol_with(svc), &reg, CodegenTarget::Rust);
        assert!(
            !diags.iter().any(|d| d.code.contains("range_expr")),
            "{:?}",
            diags
        );
    }

    #[test]
    fn ts_rejects_empty_service() {
        let svc = Construct::new("fn", "Fn", Shape::Fn, "Empty".into(), Span::new(0, 0));
        let reg = LayerRegistry::builtin();
        let diags = check_target_capabilities(&sol_with(svc), &reg, CodegenTarget::TypeScript);
        assert!(
            diags.iter().any(|d| d.code.contains("empty_service_body")),
            "{:?}",
            diags
        );
    }

    #[test]
    fn multi_target_debt_warns_on_range_for_default_rust() {
        let mut svc = Construct::new("fn", "Fn", Shape::Fn, "Walk".into(), Span::new(0, 0));
        svc.steps.push(FlowStep::Step(StepDef {
            name: "s".into(),
            span: Span::new(0, 0),
            body: vec![Expr::Range {
                start: None,
                end: Some(Box::new(Expr::IntLit(5))),
                inclusive: false,
            }],
            refs: Vec::new(),
            sub_blocks: Vec::new(),
        }));
        let reg = LayerRegistry::builtin();
        let diags = check_multi_target_debt(&sol_with(svc), &reg);
        assert!(
            diags
                .iter()
                .any(|d| d.code == "capability_debt" && d.message.contains("range")),
            "{:?}",
            diags
        );
    }

    #[test]
    fn empty_ui_template_ts_error() {
        let mut comp = Construct::new(
            "comp",
            "Component",
            Shape::Struct,
            "Card".into(),
            Span::new(0, 0),
        );
        comp.raw_blocks
            .push(("template".into(), "   ".into()));
        let reg = LayerRegistry::builtin();
        let diags = check_target_capabilities(&sol_with(comp), &reg, CodegenTarget::TypeScript);
        assert!(
            diags.iter().any(|d| d.code.contains("empty_ui_template")),
            "{:?}",
            diags
        );
    }
}
