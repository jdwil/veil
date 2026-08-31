//! Typed-step config → Rust lowering (workflow / reaction node kinds).
//!
//! Layer step-nodes (`mt step`) carry their configuration in
//! [`veil_ir::ast::StepField`] as raw strings (e.g. `condition: "x > 0"`,
//! `expression: "a + b"`, `binding: "total"`). The core codegen paths emit only
//! the step *body* expressions and ignore this config. This module lowers the
//! config of the built-in control/data node kinds into real Rust:
//!
//! | kind        | config fields                     | lowers to                     |
//! |-------------|-----------------------------------|-------------------------------|
//! | `decision`  | `condition`                       | `if <cond> { .. } else { .. }` |
//! | `branch`    | `scrutinee`, `cases`, `has_default` | `match <scrutinee> { .. }`   |
//! | `transform` | `binding`, `expression`           | `let <binding> = <expr>;`      |
//!
//! Config field values are parsed on demand with
//! [`veil_parser::parse_expr_str`] against the active [`LayerRegistry`]; the IR
//! keeps the raw string as the source of truth (byte-stable round-trip), so no
//! AST/serialize/edit/builder change is required.
//!
//! NOTE (edge assembly): the branch/arm *bodies* here are emitted as
//! placeholders. Assembling which downstream node's code belongs in the
//! true/false/case arms is the graph→structured-control-flow lowering
//! (walk edges from entry, nest blocks), specified separately. This module
//! provides the per-node construct; the graph walker fills the bodies.

use veil_ir::ast::StepDef;
use veil_ir::layer::LayerRegistry;

use crate::expr::{expr_to_rust, GenCtx};

/// Look up a typed-step config field value by name.
fn field<'a>(step: &'a StepDef, name: &str) -> Option<&'a str> {
    step.fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.value.as_str())
}

/// Render a config field value as a Rust expression string, parsing it against
/// the layer registry. On parse failure, emits a compile-visible marker so the
/// error surfaces at build time rather than silently producing wrong code.
///
/// Expression-typed config fields are declared `Str` in the layer `has` schema,
/// so authors quote them (`condition: "x > 0"`). The quotes delimit the field
/// value in source; the *inner* text is the VEIL expression. Unwrap a single
/// surrounding pair of quotes before parsing so `"x > 0"` parses as the
/// comparison `x > 0`, not as a string literal.
fn config_expr(value: &str, registry: &LayerRegistry, ctx: &GenCtx) -> Result<String, String> {
    let inner = unwrap_quotes(value);
    match veil_parser::parse_expr_str(inner, registry) {
        Ok(expr) => Ok(expr_to_rust(&expr, ctx)),
        Err(e) => Err(format!("invalid expression `{inner}`: {}", e.message)),
    }
}

/// Strip a single matching pair of surrounding double quotes, if present.
fn unwrap_quotes(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

/// If `step` is a built-in typed node kind whose config lowers to Rust, return
/// the emitted Rust. Returns `None` for plain steps and unknown kinds so the
/// caller falls back to body-expression emission.
///
/// `body_placeholders` supplies the rendered Rust for edge-target bodies keyed
/// by edge label (e.g. `"true"`, `"false"`, a case label, `"default"`). When
/// empty, empty blocks are emitted (per-node lowering without graph assembly).
pub fn lower_typed_step(
    step: &StepDef,
    registry: &LayerRegistry,
    ctx: &GenCtx,
    body_for: &dyn Fn(&str) -> String,
) -> Option<String> {
    let kind = step.kind.as_deref()?;
    match kind {
        "decision" => Some(lower_decision(step, registry, ctx, body_for)),
        "branch" => Some(lower_branch(step, registry, ctx, body_for)),
        "transform" => Some(lower_transform(step, registry, ctx)),
        _ => None,
    }
}

fn lower_decision(
    step: &StepDef,
    registry: &LayerRegistry,
    ctx: &GenCtx,
    body_for: &dyn Fn(&str) -> String,
) -> String {
    let cond = match field(step, "condition") {
        Some(v) => match config_expr(v, registry, ctx) {
            Ok(rust) => rust,
            Err(msg) => return format!("compile_error!(\"decision {}: {}\");\n", step.name, msg),
        },
        None => return format!("compile_error!(\"decision {}: missing condition\");\n", step.name),
    };
    let then_body = body_for("true");
    let else_body = body_for("false");
    let mut out = String::new();
    out.push_str(&format!("// decision: {}\n", step.name));
    out.push_str(&format!("if {cond} {{\n"));
    out.push_str(&indent(&then_body));
    out.push_str("} else {\n");
    out.push_str(&indent(&else_body));
    out.push_str("}\n");
    out
}

fn lower_branch(
    step: &StepDef,
    registry: &LayerRegistry,
    ctx: &GenCtx,
    body_for: &dyn Fn(&str) -> String,
) -> String {
    let scrutinee = match field(step, "scrutinee") {
        Some(v) => match config_expr(v, registry, ctx) {
            Ok(rust) => rust,
            Err(msg) => return format!("compile_error!(\"branch {}: {}\");\n", step.name, msg),
        },
        None => return format!("compile_error!(\"branch {}: missing scrutinee\");\n", step.name),
    };
    // `cases` is captured as a raw list string (e.g. `["approve", "reject"]`);
    // the edge labels are the authoritative case set. Emit one arm per edge
    // label that is not the default sentinel.
    let mut out = String::new();
    out.push_str(&format!("// branch: {}\n", step.name));
    out.push_str(&format!("match {scrutinee} {{\n"));
    let mut saw_default = false;
    for edge in &step.edges {
        if edge.label == "default" || edge.label == "_" {
            saw_default = true;
            continue;
        }
        let arm_body = body_for(&edge.label);
        out.push_str(&format!("    {:?} => {{\n", edge.label));
        out.push_str(&indent(&indent(&arm_body)));
        out.push_str("    }\n");
    }
    // Rust match must be exhaustive: always emit a wildcard arm.
    let default_body = if saw_default { body_for("default") } else { String::new() };
    out.push_str("    _ => {\n");
    out.push_str(&indent(&indent(&default_body)));
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

fn lower_transform(step: &StepDef, registry: &LayerRegistry, ctx: &GenCtx) -> String {
    let binding = match field(step, "binding") {
        Some(v) if !v.is_empty() => v,
        _ => return format!("compile_error!(\"transform {}: missing binding\");\n", step.name),
    };
    let expr = match field(step, "expression") {
        Some(v) => match config_expr(v, registry, ctx) {
            Ok(rust) => rust,
            Err(msg) => return format!("compile_error!(\"transform {}: {}\");\n", step.name, msg),
        },
        None => return format!("compile_error!(\"transform {}: missing expression\");\n", step.name),
    };
    // Reassignment vs new binding is resolved by the caller's mut-local
    // analysis; default to a `let` (shadowing is valid Rust and matches
    // "create a new variable"). A reassignment (`x = expr;`) is emitted when
    // the binding is already a known local — handled by the caller when it
    // threads scope; here we emit the create form.
    format!("// transform: {}\nlet {binding} = {expr};\n", step.name)
}

/// Indent every non-empty line by 4 spaces.
fn indent(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use veil_ir::ast::{StepEdge, StepField};
    use veil_ir::span::Span;

    fn f(name: &str, value: &str) -> StepField {
        StepField { name: name.into(), value: value.into(), span: Span::new(0, 0) }
    }
    fn edge(label: &str, target: &str) -> StepEdge {
        StepEdge { label: label.into(), target: target.into(), span: Span::new(0, 0) }
    }
    fn step(kind: &str, name: &str, fields: Vec<StepField>, edges: Vec<StepEdge>) -> StepDef {
        StepDef {
            name: name.into(),
            span: Span::new(0, 0),
            body: vec![],
            refs: vec![],
            sub_blocks: vec![],
            kind: Some(kind.into()),
            fields,
            edges,
        }
    }
    fn ctx() -> GenCtx {
        GenCtx::new(HashMap::new())
    }
    fn reg() -> LayerRegistry {
        LayerRegistry::builtin()
    }
    fn empty(_l: &str) -> String {
        String::new()
    }

    #[test]
    fn transform_lowers_to_let() {
        let s = step("transform", "Setup", vec![f("binding", "total"), f("expression", "\"1 + 2\"")], vec![]);
        let out = lower_typed_step(&s, &reg(), &ctx(), &empty).unwrap();
        assert!(out.contains("let total = 1 + 2;"), "got: {out}");
    }

    #[test]
    fn decision_lowers_to_if_else() {
        let s = step("decision", "Check", vec![f("condition", "\"total > 0\"")], vec![]);
        let out = lower_typed_step(&s, &reg(), &ctx(), &empty).unwrap();
        assert!(out.contains("if total > 0 {"), "got: {out}");
        assert!(out.contains("} else {"), "got: {out}");
    }

    #[test]
    fn branch_lowers_to_match_with_wildcard() {
        let s = step(
            "branch",
            "Route",
            vec![f("scrutinee", "\"label\"")],
            vec![edge("approve", "a"), edge("reject", "b"), edge("default", "c")],
        );
        let out = lower_typed_step(&s, &reg(), &ctx(), &empty).unwrap();
        assert!(out.contains("match label {"), "got: {out}");
        assert!(out.contains("\"approve\" =>"), "got: {out}");
        assert!(out.contains("\"reject\" =>"), "got: {out}");
        assert!(out.contains("_ =>"), "got: {out}");
    }

    #[test]
    fn missing_condition_emits_compile_error() {
        let s = step("decision", "Bad", vec![], vec![]);
        let out = lower_typed_step(&s, &reg(), &ctx(), &empty).unwrap();
        assert!(out.contains("compile_error!"), "got: {out}");
    }

    #[test]
    fn unknown_kind_returns_none() {
        let s = step("mystery", "X", vec![], vec![]);
        assert!(lower_typed_step(&s, &reg(), &ctx(), &empty).is_none());
    }
}
