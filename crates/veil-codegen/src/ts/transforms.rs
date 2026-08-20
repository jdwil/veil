//! TypeScript IR transforms — post-lowering passes on TsExpr trees.
//!
//! These transforms run after `lower_to_ts` and before `emit_ts`:
//!
//! - `track_imports` — collect type names that need import statements
//! - `detect_async` — determine if a function body requires the `async` keyword

use std::collections::BTreeSet;
use super::expr::{TsExpr, TsTemplatePart, TsType};

// ─── Import Tracking ─────────────────────────────────────────────────────────

/// Walk a slice of TsExpr nodes and collect type names that require imports.
///
/// Collects:
/// - Type names from `TypeAssertion` nodes (e.g. `expr as Customer`)
/// - Class names from `NewCall` nodes (e.g. `new Order(...)`)
/// - Named types from `TsType::Named` annotations on nodes
///
/// Returns a sorted, deduplicated list of type names.
pub fn track_imports(exprs: &[TsExpr]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for expr in exprs {
        collect_imports_from_expr(expr, &mut names);
    }
    // Filter out built-in types that don't need imports
    names
        .into_iter()
        .filter(|n| !is_builtin_type(n))
        .collect()
}

/// Generate import statement from collected type names.
///
/// Produces: `import type { Customer, Order } from './types';`
/// Returns `None` if no imports are needed.
pub fn import_statement(type_names: &[String]) -> Option<String> {
    if type_names.is_empty() {
        return None;
    }
    Some(format!(
        "import type {{ {} }} from './types';",
        type_names.join(", ")
    ))
}

fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "number"
            | "boolean"
            | "void"
            | "null"
            | "undefined"
            | "Date"
            | "Uint8Array"
            | "Error"
            | "Map"
            | "Set"
            | "Array"
            | "Promise"
            | "Record"
            | "Partial"
            | "Required"
            | "Readonly"
            | "Pick"
            | "Omit"
    )
}

fn collect_imports_from_expr(expr: &TsExpr, names: &mut BTreeSet<String>) {
    match expr {
        // Type assertions: `expr as Customer`
        TsExpr::TypeAssertion { expr, ty } => {
            if !is_builtin_type(ty) && ty.chars().next().is_some_and(|c| c.is_uppercase()) {
                names.insert(ty.clone());
            }
            collect_imports_from_expr(expr, names);
        }

        // Constructor calls: `new Order(...)`
        TsExpr::NewCall { class, args, ty } => {
            if !is_builtin_type(class) && class.chars().next().is_some_and(|c| c.is_uppercase()) {
                names.insert(class.clone());
            }
            collect_imports_from_type(ty, names);
            for arg in args {
                collect_imports_from_expr(arg, names);
            }
        }

        // Type annotations on various nodes
        TsExpr::Ident { ty, .. } => collect_imports_from_type(ty, names),
        TsExpr::ArrayLit { items, ty } => {
            collect_imports_from_type(ty, names);
            for item in items {
                collect_imports_from_expr(item, names);
            }
        }
        TsExpr::ObjectLit { fields, ty } => {
            collect_imports_from_type(ty, names);
            for (_, val) in fields {
                collect_imports_from_expr(val, names);
            }
        }
        TsExpr::BinOp { left, right, ty, .. } => {
            collect_imports_from_type(ty, names);
            collect_imports_from_expr(left, names);
            collect_imports_from_expr(right, names);
        }
        TsExpr::FieldAccess { base, ty, .. } => {
            collect_imports_from_type(ty, names);
            collect_imports_from_expr(base, names);
        }
        TsExpr::MethodCall { receiver, args, ty, .. } => {
            collect_imports_from_type(ty, names);
            collect_imports_from_expr(receiver, names);
            for arg in args {
                collect_imports_from_expr(arg, names);
            }
        }
        TsExpr::FnCall { args, ty, .. } => {
            collect_imports_from_type(ty, names);
            for arg in args {
                collect_imports_from_expr(arg, names);
            }
        }

        // Bindings — check the type annotation string for Named types
        TsExpr::Const { value, ty, .. } => {
            if let Some(ty_str) = ty {
                if ty_str.chars().next().is_some_and(|c| c.is_uppercase())
                    && !is_builtin_type(ty_str)
                {
                    names.insert(ty_str.clone());
                }
            }
            collect_imports_from_expr(value, names);
        }
        TsExpr::Let { value, ty, .. } => {
            if let Some(ty_str) = ty {
                if ty_str.chars().next().is_some_and(|c| c.is_uppercase())
                    && !is_builtin_type(ty_str)
                {
                    names.insert(ty_str.clone());
                }
            }
            collect_imports_from_expr(value, names);
        }
        TsExpr::Destructure { value, .. } => {
            collect_imports_from_expr(value, names);
        }
        TsExpr::Assign { target, value } => {
            collect_imports_from_expr(target, names);
            collect_imports_from_expr(value, names);
        }

        // Control flow — recurse into bodies
        TsExpr::If { condition, then_body, else_body } => {
            collect_imports_from_expr(condition, names);
            for e in then_body {
                collect_imports_from_expr(e, names);
            }
            if let Some(els) = else_body {
                for e in els {
                    collect_imports_from_expr(e, names);
                }
            }
        }
        TsExpr::Switch { scrutinee, cases, default } => {
            collect_imports_from_expr(scrutinee, names);
            for (_, body) in cases {
                for e in body {
                    collect_imports_from_expr(e, names);
                }
            }
            if let Some(def) = default {
                for e in def {
                    collect_imports_from_expr(e, names);
                }
            }
        }
        TsExpr::For { iterable, body, .. } => {
            collect_imports_from_expr(iterable, names);
            for e in body {
                collect_imports_from_expr(e, names);
            }
        }
        TsExpr::ForIndex { iterable, body, .. } => {
            collect_imports_from_expr(iterable, names);
            for e in body {
                collect_imports_from_expr(e, names);
            }
        }
        TsExpr::While { condition, body } => {
            collect_imports_from_expr(condition, names);
            for e in body {
                collect_imports_from_expr(e, names);
            }
        }
        TsExpr::Loop { body } => {
            for e in body {
                collect_imports_from_expr(e, names);
            }
        }
        TsExpr::ArrowFn { body, .. } => {
            for e in body {
                collect_imports_from_expr(e, names);
            }
        }

        // Wrappers
        TsExpr::Return(inner) => collect_imports_from_expr(inner, names),
        TsExpr::Await(inner) => collect_imports_from_expr(inner, names),
        TsExpr::Throw { message } => collect_imports_from_expr(message, names),
        TsExpr::UnaryOp { expr, .. } => collect_imports_from_expr(expr, names),
        TsExpr::OptionalChain { base, .. } => collect_imports_from_expr(base, names),
        TsExpr::NullishCoalesce { left, right } => {
            collect_imports_from_expr(left, names);
            collect_imports_from_expr(right, names);
        }
        TsExpr::Index { base, index } => {
            collect_imports_from_expr(base, names);
            collect_imports_from_expr(index, names);
        }
        TsExpr::NonNullAssertion(inner) => collect_imports_from_expr(inner, names),
        TsExpr::Spread(inner) => collect_imports_from_expr(inner, names),
        TsExpr::TemplateLit { parts } => {
            for part in parts {
                if let TsTemplatePart::Expr(e) = part {
                    collect_imports_from_expr(e, names);
                }
            }
        }

        // Leaves — no children to recurse into
        TsExpr::StringLit(_)
        | TsExpr::IntLit(_)
        | TsExpr::FloatLit(_)
        | TsExpr::BoolLit(_)
        | TsExpr::NullLit
        | TsExpr::UndefinedLit
        | TsExpr::Break
        | TsExpr::Continue
        | TsExpr::Raw(_)
        | TsExpr::LayerEmit(_) => {}
    }
}

fn collect_imports_from_type(ty: &Option<TsType>, names: &mut BTreeSet<String>) {
    if let Some(t) = ty {
        collect_type_names(t, names);
    }
}

fn collect_type_names(ty: &TsType, names: &mut BTreeSet<String>) {
    match ty {
        TsType::Named(name) => {
            if !is_builtin_type(name) && name.chars().next().is_some_and(|c| c.is_uppercase()) {
                names.insert(name.clone());
            }
        }
        TsType::Array(inner) => collect_type_names(inner, names),
        TsType::Promise(inner) => collect_type_names(inner, names),
        TsType::Union(types) => {
            for t in types {
                collect_type_names(t, names);
            }
        }
        TsType::Record(k, v) => {
            collect_type_names(k, names);
            collect_type_names(v, names);
        }
        TsType::Fn { params, ret } => {
            for p in params {
                collect_type_names(p, names);
            }
            collect_type_names(ret, names);
        }
        TsType::String | TsType::Number | TsType::Boolean | TsType::Null | TsType::Void => {}
    }
}

// ─── Async Detection ─────────────────────────────────────────────────────────

/// Determine if a function body requires the `async` keyword.
///
/// Returns `true` if ANY node in the tree is `TsExpr::Await(...)` or
/// a `MethodCall` with `is_async: true`.
pub fn detect_async(body: &[TsExpr]) -> bool {
    body.iter().any(|e| has_await(e))
}

fn has_await(expr: &TsExpr) -> bool {
    match expr {
        // Direct await
        TsExpr::Await(_) => true,

        // Method call marked async (emits `await` in emit_ts)
        TsExpr::MethodCall { is_async: true, .. } => true,

        // Recurse into children
        TsExpr::MethodCall { receiver, args, .. } => {
            has_await(receiver) || args.iter().any(|a| has_await(a))
        }
        TsExpr::FnCall { args, .. } => args.iter().any(|a| has_await(a)),
        TsExpr::NewCall { args, .. } => args.iter().any(|a| has_await(a)),
        TsExpr::BinOp { left, right, .. } => has_await(left) || has_await(right),
        TsExpr::UnaryOp { expr, .. } => has_await(expr),
        TsExpr::FieldAccess { base, .. } => has_await(base),
        TsExpr::OptionalChain { base, .. } => has_await(base),
        TsExpr::NullishCoalesce { left, right } => has_await(left) || has_await(right),
        TsExpr::Index { base, index } => has_await(base) || has_await(index),
        TsExpr::TypeAssertion { expr, .. } => has_await(expr),
        TsExpr::NonNullAssertion(inner) => has_await(inner),
        TsExpr::Spread(inner) => has_await(inner),
        TsExpr::Return(inner) => has_await(inner),
        TsExpr::Throw { message } => has_await(message),

        TsExpr::Const { value, .. } => has_await(value),
        TsExpr::Let { value, .. } => has_await(value),
        TsExpr::Destructure { value, .. } => has_await(value),
        TsExpr::Assign { target, value } => has_await(target) || has_await(value),

        TsExpr::If { condition, then_body, else_body } => {
            has_await(condition)
                || then_body.iter().any(|e| has_await(e))
                || else_body.as_ref().is_some_and(|els| els.iter().any(|e| has_await(e)))
        }
        TsExpr::Switch { scrutinee, cases, default } => {
            has_await(scrutinee)
                || cases.iter().any(|(_, body)| body.iter().any(|e| has_await(e)))
                || default.as_ref().is_some_and(|d| d.iter().any(|e| has_await(e)))
        }
        TsExpr::For { iterable, body, .. } => {
            has_await(iterable) || body.iter().any(|e| has_await(e))
        }
        TsExpr::ForIndex { iterable, body, .. } => {
            has_await(iterable) || body.iter().any(|e| has_await(e))
        }
        TsExpr::While { condition, body } => {
            has_await(condition) || body.iter().any(|e| has_await(e))
        }
        TsExpr::Loop { body } => body.iter().any(|e| has_await(e)),

        // Arrow functions create a new scope — do NOT look inside them
        // (they get their own `async` keyword independently)
        TsExpr::ArrowFn { .. } => false,

        TsExpr::ArrayLit { items, .. } => items.iter().any(|e| has_await(e)),
        TsExpr::ObjectLit { fields, .. } => fields.iter().any(|(_, v)| has_await(v)),
        TsExpr::TemplateLit { parts } => parts.iter().any(|p| match p {
            TsTemplatePart::Expr(e) => has_await(e),
            TsTemplatePart::Literal(_) => false,
        }),

        // Leaves
        TsExpr::Ident { .. }
        | TsExpr::StringLit(_)
        | TsExpr::IntLit(_)
        | TsExpr::FloatLit(_)
        | TsExpr::BoolLit(_)
        | TsExpr::NullLit
        | TsExpr::UndefinedLit
        | TsExpr::Break
        | TsExpr::Continue
        | TsExpr::Raw(_)
        | TsExpr::LayerEmit(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::expr::{TsBinOp, TsType};

    // ─── track_imports tests ─────────────────────────────────────────────

    #[test]
    fn test_track_imports_type_assertion() {
        let exprs = vec![TsExpr::TypeAssertion {
            expr: Box::new(TsExpr::Ident { name: "data".into(), ty: None }),
            ty: "Customer".into(),
        }];
        assert_eq!(track_imports(&exprs), vec!["Customer".to_string()]);
    }

    #[test]
    fn test_track_imports_new_call() {
        let exprs = vec![TsExpr::NewCall {
            class: "Order".into(),
            args: vec![],
            ty: None,
        }];
        assert_eq!(track_imports(&exprs), vec!["Order".to_string()]);
    }

    #[test]
    fn test_track_imports_named_type_annotation() {
        let exprs = vec![TsExpr::Ident {
            name: "item".into(),
            ty: Some(TsType::Named("Product".into())),
        }];
        assert_eq!(track_imports(&exprs), vec!["Product".to_string()]);
    }

    #[test]
    fn test_track_imports_builtin_filtered() {
        let exprs = vec![
            TsExpr::NewCall { class: "Error".into(), args: vec![], ty: None },
            TsExpr::NewCall { class: "Date".into(), args: vec![], ty: None },
        ];
        assert!(track_imports(&exprs).is_empty());
    }

    #[test]
    fn test_track_imports_deduplicates() {
        let exprs = vec![
            TsExpr::TypeAssertion {
                expr: Box::new(TsExpr::NullLit),
                ty: "Customer".into(),
            },
            TsExpr::NewCall { class: "Customer".into(), args: vec![], ty: None },
        ];
        assert_eq!(track_imports(&exprs), vec!["Customer".to_string()]);
    }

    #[test]
    fn test_track_imports_nested_in_body() {
        let exprs = vec![TsExpr::If {
            condition: Box::new(TsExpr::BoolLit(true)),
            then_body: vec![TsExpr::NewCall {
                class: "Invoice".into(),
                args: vec![],
                ty: None,
            }],
            else_body: None,
        }];
        assert_eq!(track_imports(&exprs), vec!["Invoice".to_string()]);
    }

    #[test]
    fn test_track_imports_array_type() {
        let exprs = vec![TsExpr::Ident {
            name: "items".into(),
            ty: Some(TsType::Array(Box::new(TsType::Named("LineItem".into())))),
        }];
        assert_eq!(track_imports(&exprs), vec!["LineItem".to_string()]);
    }

    #[test]
    fn test_import_statement_generation() {
        let names = vec!["Customer".to_string(), "Order".to_string()];
        assert_eq!(
            import_statement(&names),
            Some("import type { Customer, Order } from './types';".to_string())
        );
    }

    #[test]
    fn test_import_statement_empty() {
        let names: Vec<String> = vec![];
        assert_eq!(import_statement(&names), None);
    }

    // ─── detect_async tests ──────────────────────────────────────────────

    #[test]
    fn test_detect_async_with_await() {
        let body = vec![
            TsExpr::Const {
                name: "result".into(),
                ty: None,
                value: Box::new(TsExpr::Await(Box::new(TsExpr::FnCall {
                    name: "fetch".into(),
                    args: vec![],
                    ty: None,
                }))),
            },
        ];
        assert!(detect_async(&body));
    }

    #[test]
    fn test_detect_async_with_async_method() {
        let body = vec![TsExpr::MethodCall {
            receiver: Box::new(TsExpr::Ident { name: "deps".into(), ty: None }),
            method: "save".into(),
            args: vec![],
            ty: None,
            is_async: true,
        }];
        assert!(detect_async(&body));
    }

    #[test]
    fn test_detect_async_without_await() {
        let body = vec![
            TsExpr::Const {
                name: "x".into(),
                ty: None,
                value: Box::new(TsExpr::IntLit(42)),
            },
            TsExpr::Return(Box::new(TsExpr::Ident { name: "x".into(), ty: None })),
        ];
        assert!(!detect_async(&body));
    }

    #[test]
    fn test_detect_async_nested_in_if() {
        let body = vec![TsExpr::If {
            condition: Box::new(TsExpr::BoolLit(true)),
            then_body: vec![TsExpr::Await(Box::new(TsExpr::FnCall {
                name: "doWork".into(),
                args: vec![],
                ty: None,
            }))],
            else_body: None,
        }];
        assert!(detect_async(&body));
    }

    #[test]
    fn test_detect_async_arrow_fn_boundary() {
        // Await inside an arrow function does NOT make the outer function async
        let body = vec![TsExpr::ArrowFn {
            params: vec![],
            body: vec![TsExpr::Await(Box::new(TsExpr::FnCall {
                name: "fetch".into(),
                args: vec![],
                ty: None,
            }))],
            is_async: true,
        }];
        assert!(!detect_async(&body));
    }
}
