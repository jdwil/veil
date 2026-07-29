//! Macro expansion pass — substitutes macro invocations with expanded macro bodies.
//!
//! Macros in VEIL are invoked using the existing Call syntax with a `!` suffix:
//! `secure!(["relay.read"])` parses as a Call with target="" and method="secure!".
//!
//! This pass runs after parsing and before codegen. It walks the AST, finds Call
//! expressions where the method name (minus `!`) matches a defined macro, and
//! replaces the entire expression (or statement) with the macro's expanded body.
//!
//! Expansion is recursive — macros can invoke other macros (up to MAX_DEPTH).

use crate::ast::{Expr, CallExpr, MacroDef, Package};
use std::collections::HashMap;

/// Maximum expansion depth to prevent infinite recursion.
const MAX_DEPTH: usize = 16;

/// Expand all macro invocations in a package.
/// Modifies the package in place — macro calls are replaced with their expanded bodies.
pub fn expand_macros(pkg: &mut Package) {
    let macros: HashMap<String, MacroDef> = pkg
        .macros
        .iter()
        .map(|m| (m.name.clone(), m.clone()))
        .collect();

    if macros.is_empty() {
        return;
    }

    for item in &mut pkg.items {
        expand_in_top_level_item(item, &macros, 0);
    }
}

/// Expand macros that are imported from another package into this one.
pub fn expand_macros_with(pkg: &mut Package, external_macros: &HashMap<String, MacroDef>) {
    let mut all_macros: HashMap<String, MacroDef> = external_macros.clone();
    for m in &pkg.macros {
        all_macros.insert(m.name.clone(), m.clone());
    }

    if all_macros.is_empty() {
        return;
    }

    for item in &mut pkg.items {
        expand_in_top_level_item(item, &all_macros, 0);
    }
}

/// Check if a Call expression is a macro invocation.
/// Returns the macro name (without `!`) if it matches.
/// Handles both forms:
/// - `name!(args)` → target="name!", method="" (bare function call syntax)
/// - `Target.name!(args)` → target="Target", method="name!" (method call syntax)
fn is_macro_call(call: &CallExpr, macros: &HashMap<String, MacroDef>) -> Option<String> {
    // Check method first (Target.macro!() form)
    if !call.method.is_empty() && call.method.ends_with('!') {
        let name = call.method.trim_end_matches('!').to_string();
        if macros.contains_key(&name) {
            return Some(name);
        }
    }
    // Check target (macro!() bare call form)
    if call.method.is_empty() && call.target.ends_with('!') {
        let name = call.target.trim_end_matches('!').to_string();
        if macros.contains_key(&name) {
            return Some(name);
        }
    }
    None
}

fn expand_in_top_level_item(
    item: &mut crate::ast::TopLevelItem,
    macros: &HashMap<String, MacroDef>,
    depth: usize,
) {
    use crate::ast::TopLevelItem;
    if let TopLevelItem::Construct(c) = item {
        expand_in_construct(c, macros, depth);
    }
}

fn expand_in_construct(
    c: &mut crate::ast::Construct,
    macros: &HashMap<String, MacroDef>,
    depth: usize,
) {
    for f in &mut c.fns {
        expand_in_body(&mut f.body, macros, depth);
    }
    for m in &mut c.impls {
        expand_in_body(&mut m.body, macros, depth);
    }
    for step in &mut c.steps {
        expand_in_flow_step(step, macros, depth);
    }
    for effect in &mut c.effects {
        expand_in_body(&mut effect.body, macros, depth);
    }
    for child in &mut c.children {
        expand_in_construct(child, macros, depth);
    }
}

fn expand_in_flow_step(
    step: &mut crate::ast::FlowStep,
    macros: &HashMap<String, MacroDef>,
    depth: usize,
) {
    use crate::ast::FlowStep;
    match step {
        FlowStep::Step(s) => {
            expand_in_body(&mut s.body, macros, depth);
        }
        FlowStep::Parallel(p) => {
            for step in &mut p.steps {
                expand_in_body(&mut step.body, macros, depth);
            }
        }
        FlowStep::Match(m) => {
            for arm in &mut m.arms {
                expand_in_body(&mut arm.body, macros, depth);
            }
        }
    }
}

/// Expand macro invocations in a body (Vec<Expr>).
/// A macro call like `secure!(args)` is replaced with the macro's body expressions.
fn expand_in_body(body: &mut Vec<Expr>, macros: &HashMap<String, MacroDef>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }

    let mut i = 0;
    while i < body.len() {
        let should_expand = match &body[i] {
            Expr::Call(call) => is_macro_call(call, macros),
            _ => None,
        };

        if let Some(macro_name) = should_expand {
            if let Expr::Call(call) = &body[i] {
                let macro_def = &macros[&macro_name];
                let expanded = expand_one(macro_def, &call.args, macros, depth + 1);
                body.splice(i..i + 1, expanded.into_iter());
                continue; // Re-check this position for nested expansions
            }
        }

        i += 1;
    }
}

/// Expand a single macro invocation into its body with parameter substitution.
fn expand_one(
    macro_def: &MacroDef,
    args: &[Expr],
    macros: &HashMap<String, MacroDef>,
    depth: usize,
) -> Vec<Expr> {
    // Build parameter → argument substitution map
    let mut subst: HashMap<String, Expr> = HashMap::new();
    for (i, param) in macro_def.params.iter().enumerate() {
        let value = if i < args.len() {
            args[i].clone()
        } else if let Some(ref default) = param.default {
            default.clone()
        } else {
            Expr::Ident(param.name.clone())
        };
        subst.insert(param.name.clone(), value);
    }

    // Clone the macro body and substitute parameters
    let mut expanded: Vec<Expr> = macro_def
        .body
        .iter()
        .map(|expr| substitute_expr(expr, &subst))
        .collect();

    // Recursively expand any nested macro invocations
    expand_in_body(&mut expanded, macros, depth);

    expanded
}

/// Substitute parameter references in an expression with their argument values.
fn substitute_expr(expr: &Expr, subst: &HashMap<String, Expr>) -> Expr {
    match expr {
        Expr::Ident(name) => {
            if let Some(replacement) = subst.get(name) {
                replacement.clone()
            } else {
                expr.clone()
            }
        }
        Expr::Assign(name, rhs, ty) => {
            Expr::Assign(name.clone(), Box::new(substitute_expr(rhs, subst)), ty.clone())
        }
        Expr::MutAssign(name, rhs, ty) => {
            Expr::MutAssign(name.clone(), Box::new(substitute_expr(rhs, subst)), ty.clone())
        }
        Expr::Call(call) => {
            let mut new_call = call.clone();
            new_call.args = call.args.iter().map(|a| substitute_expr(a, subst)).collect();
            Expr::Call(new_call)
        }
        Expr::IfExpr(if_data) => {
            let mut new_if = if_data.clone();
            new_if.condition = Box::new(substitute_expr(&if_data.condition, subst));
            new_if.then_body = if_data.then_body.iter().map(|e| substitute_expr(e, subst)).collect();
            if let Some(ref else_body) = if_data.else_body {
                new_if.else_body = Some(else_body.iter().map(|e| substitute_expr(e, subst)).collect());
            }
            Expr::IfExpr(new_if)
        }
        Expr::Return(inner) => Expr::Return(Box::new(substitute_expr(inner, subst))),
        Expr::ForLoop { binding, index, iterable, body } => Expr::ForLoop {
            binding: binding.clone(),
            index: index.clone(),
            iterable: Box::new(substitute_expr(iterable, subst)),
            body: body.iter().map(|e| substitute_expr(e, subst)).collect(),
        },
        Expr::StringInterp(parts) => {
            let new_parts = parts.iter().map(|p| match p {
                crate::ast::StringPart::Literal(s) => crate::ast::StringPart::Literal(s.clone()),
                crate::ast::StringPart::Expr(e) => crate::ast::StringPart::Expr(substitute_expr(e, subst)),
            }).collect();
            Expr::StringInterp(new_parts)
        },
        // All other expressions: clone as-is
        _ => expr.clone(),
    }
}
