//! Ownership semantics for `RustExpr`.
//!
//! `apply_ownership` wraps expressions in `Clone` when the value is non-Copy,
//! multi-use, and not already owned. `suppress_try_in_closure` converts `?`
//! operators to `.unwrap()` inside closure bodies.

use super::super::context::GenCtx;
use super::super::types::{rust_already_owned, is_unit_enum_variant};
use super::super::calls::{is_copy_local, is_ref_local};
use super::RustExpr;

/// Apply ownership semantics to a `RustExpr`: wrap in `Clone` when the value
/// is non-Copy, multi-use, and not already owned.
///
/// This is the IR-level replacement for the old string-based `clone_for_reuse`.
/// It operates on structure rather than rendered text, making the decision
/// composable with later transforms (borrow insertion, move elision, etc.).
///
/// Call this on expressions in argument positions or assignment RHS where VEIL's
/// "values are reusable" semantics require ownership transfer / cloning.
pub fn apply_ownership(expr: RustExpr, ctx: &GenCtx) -> RustExpr {
    // Already owned / already a clone — no double-clone
    if is_already_owned(&expr) {
        return expr;
    }
    // Copy types don't need cloning
    if is_expr_copy(&expr, ctx) {
        return expr;
    }
    match &expr {
        RustExpr::Ident { name, .. } => {
            if should_clone_ident_ir(name, ctx) {
                RustExpr::Clone(Box::new(expr))
            } else {
                expr
            }
        }
        RustExpr::FieldAccess { .. } => {
            // Field accesses from lower_field_access already have Clone applied
            // where needed. But if someone calls apply_ownership on a bare
            // FieldAccess (e.g. from a different lowering path), clone it.
            RustExpr::Clone(Box::new(expr))
        }
        RustExpr::Statement { text, .. } => {
            // Already-owned strings from the old path
            if rust_already_owned(text) {
                return expr;
            }
            // Call results are owned — block expressions and function/method calls
            let t = text.trim();
            if (t.starts_with('{') && t.ends_with('}'))
                || (t.contains('(') && (t.ends_with(')')
                    || t.ends_with(")?")
                    || t.ends_with(".await?")
                    || t.ends_with(".await")
                    || t.ends_with(".unwrap()")))
            {
                return expr;
            }
            // Qualified paths (e.g. DomainError::NotFound) are values, not borrowable
            if t.contains("::") {
                return expr;
            }
            // Statements don't produce values — never clone them.
            if raw_is_statement(text) {
                return expr;
            }
            RustExpr::Clone(Box::new(expr))
        }
        // Literals, FnCalls, MethodCalls produce owned values — no clone needed
        _ => expr,
    }
}

/// Whether a `RustExpr` is already an owned value (clone, literal, call result).
fn is_already_owned(expr: &RustExpr) -> bool {
    matches!(
        expr,
        RustExpr::Clone(_)
            | RustExpr::StringLit(_)
            | RustExpr::IntLit(_)
            | RustExpr::FloatLit(_)
            | RustExpr::BoolLit(_)
            | RustExpr::FnCall { .. }
            | RustExpr::MethodCall { .. }
            | RustExpr::Format { .. }
            | RustExpr::Block { .. }
            | RustExpr::If { .. }
            | RustExpr::Match { .. }
            | RustExpr::JsonMacro { .. }
            | RustExpr::JsonNull
            | RustExpr::JsonEmptyArray
            | RustExpr::VecMacro(_)
            | RustExpr::Array { .. }
            | RustExpr::Tuple { .. }
            | RustExpr::StructLit { .. }
            | RustExpr::BinOp { .. }
            | RustExpr::UnaryOp { .. }
            | RustExpr::For { .. }
            | RustExpr::While { .. }
            | RustExpr::Loop { .. }
            | RustExpr::Let { .. }
            | RustExpr::Await(_)
            | RustExpr::Try(_)
            | RustExpr::MapErr { .. }
            | RustExpr::Borrow { .. }
            | RustExpr::LayerEmit(_)
            | RustExpr::CompileError(_)
            | RustExpr::Return { .. }
    )
}

/// Whether a raw text expression represents a call result (owned value).
///
/// TRANSITION DEBT: remove when translate_call is fully migrated to structured
/// RustExpr — at that point all calls will be MethodCall/FnCall variants and
/// `is_already_owned` handles those structurally.
///
/// Heuristic: contains `(` and ends with one of:
/// - `)` — plain function/method call
/// - `)?` — fallible call
/// - `.await?` — async+fallible call
/// - `.await` — async call
/// - `.to_string()` / `.clone()` — already owned (redundant with rust_already_owned but safe)
/// - `}` — block expression producing a value (e.g. Process.run)
///
/// Also catches format!(...), serde_json::from_str(...), etc.
/// Whether a raw text expression is a statement (doesn't produce a value).
/// Statements must not be cloned — they're used for side effects.
fn raw_is_statement(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("let ")
        || t.starts_with("for ")
        || t.starts_with("while ")
        || t.starts_with("loop {")
        || t.starts_with("if ")
        || t.starts_with("match ")
        || t.starts_with("return ")
        || t.starts_with("return\n")
        || t == "break"
        || t == "continue"
        || t.starts_with("state[")
        || t.starts_with("self.")
        || t.contains(" = ")
        || t.contains(" += ")
        || t.contains(" -= ")
        || t.contains(" *= ")
        || t.ends_with('}')
        || t.starts_with("compile_error!")
}

/// Whether the expression's type is Copy (primitives, unit enums, refs).
fn is_expr_copy(expr: &RustExpr, ctx: &GenCtx) -> bool {
    match expr {
        RustExpr::IntLit(_) | RustExpr::FloatLit(_) | RustExpr::BoolLit(_) => true,
        RustExpr::Ident { name, ty } => {
            // Check type annotation first
            if let Some(t) = ty
                && t.is_copy() {
                    return true;
                }
            // Check context: local type or unit enum variant
            if is_copy_local(name, ctx) {
                return true;
            }
            is_unit_enum_variant(name, ctx)
        }
        RustExpr::FieldAccess { ty, .. } => {
            ty.as_ref().is_some_and(|t| t.is_copy())
        }
        RustExpr::Statement { ty, text, .. } => {
            if let Some(t) = ty
                && t.is_copy() {
                    return true;
                }
            // Fallback: check if the raw text is a literal
            text.parse::<i64>().is_ok() || text == "true" || text == "false"
        }
        RustExpr::Borrow { .. } => true, // refs are Copy
        _ => false,
    }
}

/// IR-level equivalent of `should_clone_ident`: decides whether an ident
/// needs cloning based on usage count, ref status, and copy type.
fn should_clone_ident_ir(name: &str, ctx: &GenCtx) -> bool {
    if is_copy_local(name, ctx) || is_unit_enum_variant(name, ctx) {
        return false;
    }
    // Qualified enum paths (e.g. "Kind::Event") are values, not borrowable names.
    // Either they're Copy unit variants or constructors — neither needs cloning.
    if name.contains("::") {
        return false;
    }
    // Uppercase non-local ident: likely an enum variant or type constant — don't clone.
    if !ctx.is_local(name)
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    {
        return false;
    }
    // Shared-ref loop element (`for x in &xs`) is `&T`. Owned slots need `.clone()`.
    if ctx.ownership.ref_elem_locals.contains(name) {
        return true;
    }
    if is_ref_local(name, ctx) {
        return false;
    }
    // Only clone if usage count is definitively > 1.
    // Default to 1 (no clone) for variables without tracking — avoids cloning
    // error variables, pattern bindings, and closure params that don't impl Clone.
    // Short names (1-2 chars) are typically lambda/match params — move-only semantics.
    if name.len() <= 2 {
        return ctx.ownership.ident_uses.get(name).copied().unwrap_or(1) > 1
            && ctx.types.local_types.get(name).is_some_and(|t| !t.contains("Error"));
    }
    ctx.ownership.ident_uses.get(name).copied().unwrap_or(1) > 1
}

// ─── Closure try-suppression ─────────────────────────────────────────────────

/// Structurally transform a `RustExpr` tree to suppress `?` / `map_err(...)?`
/// inside closure bodies. Closures don't return `Result`, so the try operator
/// is invalid — replace with `.unwrap()`.
///
/// Transforms:
/// - `Try(inner)` → `MethodCall { receiver: inner, method: "unwrap", ... }`
/// - `MapErr { inner, .. }` → `MethodCall { receiver: inner, method: "unwrap", ... }`
/// - `MethodCall { is_fallible: true, .. }` → same with `is_fallible: false` + `.unwrap()`
/// - `Raw { text }` → text-level fixup (fallback for unmigrated paths)
///
/// This is a recursive traversal — it walks the entire expression tree.
pub fn suppress_try_in_closure(expr: RustExpr) -> RustExpr {
    match expr {
        // Direct ? operator → .unwrap()
        RustExpr::Try(inner) => {
            let inner = suppress_try_in_closure(*inner);
            RustExpr::MethodCall {
                receiver: Box::new(inner),
                method: "unwrap".to_string(),
                args: vec![],
                ty: None,
                is_async: false,
                is_fallible: false,
            }
        }
        // .map_err(...)? → .unwrap()  (drop the error mapping entirely)
        RustExpr::MapErr { inner, .. } => {
            let inner = suppress_try_in_closure(*inner);
            RustExpr::MethodCall {
                receiver: Box::new(inner),
                method: "unwrap".to_string(),
                args: vec![],
                ty: None,
                is_async: false,
                is_fallible: false,
            }
        }
        // Fallible method call → call .unwrap() on the result
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible: true,
        } => {
            let inner = RustExpr::MethodCall {
                receiver: Box::new(suppress_try_in_closure(*receiver)),
                method,
                args: args.into_iter().map(suppress_try_in_closure).collect(),
                ty: ty.clone(),
                is_async,
                is_fallible: false,
            };
            RustExpr::MethodCall {
                receiver: Box::new(inner),
                method: "unwrap".to_string(),
                args: vec![],
                ty,
                is_async: false,
                is_fallible: false,
            }
        }
        // Raw text: apply string-level fixup as fallback for unmigrated paths
        RustExpr::Statement { text, ty } => {
            let text = fixup_closure_raw(&text);
            RustExpr::Statement { text, ty }
        }
        // Recurse into compound expressions
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible,
        } => RustExpr::MethodCall {
            receiver: Box::new(suppress_try_in_closure(*receiver)),
            method,
            args: args.into_iter().map(suppress_try_in_closure).collect(),
            ty,
            is_async,
            is_fallible,
        },
        RustExpr::FnCall { path, args, ty } => RustExpr::FnCall {
            path,
            args: args.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Clone(inner) => {
            RustExpr::Clone(Box::new(suppress_try_in_closure(*inner)))
        }
        RustExpr::Borrow { inner, mutable } => RustExpr::Borrow {
            inner: Box::new(suppress_try_in_closure(*inner)),
            mutable,
        },
        RustExpr::Await(inner) => {
            RustExpr::Await(Box::new(suppress_try_in_closure(*inner)))
        }
        RustExpr::Block { stmts, value } => RustExpr::Block {
            stmts: stmts.into_iter().map(suppress_try_in_closure).collect(),
            value: value.map(|v| Box::new(suppress_try_in_closure(*v))),
        },
        RustExpr::If {
            condition,
            then_body,
            else_body,
        } => RustExpr::If {
            condition: Box::new(suppress_try_in_closure(*condition)),
            then_body: then_body.into_iter().map(suppress_try_in_closure).collect(),
            else_body: else_body.map(|b| b.into_iter().map(suppress_try_in_closure).collect()),
        },
        RustExpr::Format { template, args } => RustExpr::Format {
            template,
            args: args.into_iter().map(suppress_try_in_closure).collect(),
        },
        RustExpr::FieldAccess { base, field, ty } => RustExpr::FieldAccess {
            base: Box::new(suppress_try_in_closure(*base)),
            field,
            ty,
        },
        RustExpr::Let {
            name,
            mutable,
            ty,
            value,
        } => RustExpr::Let {
            name,
            mutable,
            ty,
            value: Box::new(suppress_try_in_closure(*value)),
        },
        RustExpr::Match { scrutinee, arms } => RustExpr::Match {
            scrutinee: Box::new(suppress_try_in_closure(*scrutinee)),
            arms: arms
                .into_iter()
                .map(|(pat, body)| (pat, suppress_try_in_closure(body)))
                .collect(),
        },
        // Leaves: no recursion needed
        RustExpr::Ident { .. }
        | RustExpr::StringLit(_)
        | RustExpr::IntLit(_)
        | RustExpr::FloatLit(_)
        | RustExpr::BoolLit(_)
        | RustExpr::JsonNull
        | RustExpr::JsonEmptyArray
        | RustExpr::LayerEmit(_)
        | RustExpr::CompileError(_) => expr,
        RustExpr::Return { value, wraps_ok } => RustExpr::Return {
            value: Box::new(suppress_try_in_closure(*value)),
            wraps_ok,
        },
        RustExpr::JsonMacro { entries } => RustExpr::JsonMacro {
            entries: entries
                .into_iter()
                .map(|(k, v)| (k, suppress_try_in_closure(v)))
                .collect(),
        },
        RustExpr::VecMacro(items) => {
            RustExpr::VecMacro(items.into_iter().map(suppress_try_in_closure).collect())
        }
        // Structural nodes: recurse into children
        RustExpr::BinOp { left, op, right, ty } => RustExpr::BinOp {
            left: Box::new(suppress_try_in_closure(*left)),
            op,
            right: Box::new(suppress_try_in_closure(*right)),
            ty,
        },
        RustExpr::UnaryOp { op, expr, ty } => RustExpr::UnaryOp {
            op,
            expr: Box::new(suppress_try_in_closure(*expr)),
            ty,
        },
        RustExpr::Array { items, ty } => RustExpr::Array {
            items: items.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Tuple { items, ty } => RustExpr::Tuple {
            items: items.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Index { base, index, ty } => RustExpr::Index {
            base: Box::new(suppress_try_in_closure(*base)),
            index: Box::new(suppress_try_in_closure(*index)),
            ty,
        },
        RustExpr::StructLit { name, fields, ty } => RustExpr::StructLit {
            name,
            fields: fields.into_iter().map(|(k, v)| (k, suppress_try_in_closure(v))).collect(),
            ty,
        },
        RustExpr::For { binding, iterable, body, ty } => RustExpr::For {
            binding,
            iterable: Box::new(suppress_try_in_closure(*iterable)),
            body: body.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::While { condition, body, ty } => RustExpr::While {
            condition: Box::new(suppress_try_in_closure(*condition)),
            body: body.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
        RustExpr::Loop { body, ty } => RustExpr::Loop {
            body: body.into_iter().map(suppress_try_in_closure).collect(),
            ty,
        },
    }
}
fn fixup_closure_raw(s: &str) -> String {
    let mut s = s
        .replace(
            ".map_err(|e| DomainError::External(format!(\"{:?}\", e)))?",
            ".unwrap()",
        )
        .replace(
            ".map_err(|e| DomainError::External(format!(\"{e:?}\")))?",
            ".unwrap()",
        )
        .replace(
            ".map_err(|e| DomainError::External(e.to_string()))?",
            ".unwrap()",
        );
    // Replace trailing `)?` with `).unwrap()`
    while let Some(pos) = s.find(")?") {
        let after = if pos + 2 < s.len() {
            &s[pos + 2..pos + 3]
        } else {
            ""
        };
        if after.is_empty()
            || after == ")"
            || after == "."
            || after == ","
            || after == ";"
            || after == " "
        {
            s = format!("{}).unwrap(){}", &s[..pos], &s[pos + 2..]);
        } else {
            break;
        }
    }
    s
}
