//! Ownership semantics for `RustExpr`.
//!
//! `apply_ownership` wraps expressions in `Clone` when the value is non-Copy,
//! multi-use, and not already owned. `suppress_try_in_closure` converts `?`
//! operators to `.unwrap()` inside closure bodies. Both operate only on
//! structure — never on rendered text.

use super::super::context::GenCtx;
use super::super::types::is_unit_enum_variant;
use super::super::calls::{is_copy_local, is_ref_local};
use super::{Arm, RustExpr};

/// Apply ownership semantics to a `RustExpr`: wrap in `Clone` when the value
/// is non-Copy, multi-use, and not already owned.
pub fn apply_ownership(expr: RustExpr, ctx: &GenCtx) -> RustExpr {
    match expr {
        RustExpr::FnCall { path, args, ty } => RustExpr::FnCall {
            path,
            args: args.into_iter().map(|a| apply_ownership(a, ctx)).collect(),
            ty,
        },
        RustExpr::MethodCall {
            receiver,
            method,
            args,
            ty,
            is_async,
            is_fallible,
        } => RustExpr::MethodCall {
            receiver: Box::new(apply_ownership_receiver(*receiver, ctx)),
            method,
            args: args.into_iter().map(|a| apply_ownership(a, ctx)).collect(),
            ty,
            is_async,
            is_fallible,
        },
        other => apply_ownership_leaf(other, ctx),
    }
}

/// Apply ownership to a method-call *receiver*. A receiver is passed to the
/// method by reference (or moved into `self`) — it must NOT be defensively
/// cloned the way a value/move position is. We still recurse into nested
/// calls/args so inner move positions get their clones, but a bare receiver
/// (ident or field access) is left untouched (`deps.repo.save(...)`, not
/// `deps.repo.clone().save(...)`).
fn apply_ownership_receiver(expr: RustExpr, ctx: &GenCtx) -> RustExpr {
    match expr {
        RustExpr::FnCall { .. } | RustExpr::MethodCall { .. } => apply_ownership(expr, ctx),
        // Leaf receivers (idents, field accesses, etc.): no defensive clone.
        other => other,
    }
}

fn apply_ownership_leaf(expr: RustExpr, ctx: &GenCtx) -> RustExpr {
    if is_already_owned(&expr) {
        return expr;
    }
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
        RustExpr::FieldAccess { .. } => RustExpr::Clone(Box::new(expr)),
        _ => expr,
    }
}

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
            | RustExpr::JsonValue(_)
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
            | RustExpr::Assign { .. }
            | RustExpr::Await(_)
            | RustExpr::Try(_)
            | RustExpr::MapErr { .. }
            | RustExpr::Borrow { .. }
            | RustExpr::LayerTemplate { .. }
            | RustExpr::CompileError(_)
            | RustExpr::Return { .. }
            | RustExpr::Closure { .. }
            | RustExpr::Cast { .. }
            | RustExpr::Range { .. }
            | RustExpr::IfLet { .. }
            | RustExpr::WhileLet { .. }
            | RustExpr::Break
            | RustExpr::Continue
            | RustExpr::Comment(_)
            | RustExpr::Join { .. }
            | RustExpr::Index { .. }
    )
}

fn is_expr_copy(expr: &RustExpr, ctx: &GenCtx) -> bool {
    match expr {
        RustExpr::IntLit(_) | RustExpr::FloatLit(_) | RustExpr::BoolLit(_) => true,
        RustExpr::Ident { name, ty } => {
            if let Some(t) = ty
                && t.is_copy()
            {
                return true;
            }
            if is_copy_local(name, ctx) {
                return true;
            }
            is_unit_enum_variant(name, ctx)
        }
        RustExpr::FieldAccess { ty, .. } => ty.as_ref().is_some_and(|t| t.is_copy()),
        RustExpr::Borrow { .. } => true,
        _ => false,
    }
}

fn should_clone_ident_ir(name: &str, ctx: &GenCtx) -> bool {
    if is_copy_local(name, ctx) || is_unit_enum_variant(name, ctx) {
        return false;
    }
    if name.contains("::") {
        return false;
    }
    if !ctx.is_local(name)
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    {
        return false;
    }
    if ctx.ownership.ref_elem_locals.contains(name) {
        return true;
    }
    if is_ref_local(name, ctx) {
        return false;
    }
    if name.len() <= 2 {
        return ctx.ownership.ident_uses.get(name).copied().unwrap_or(1) > 1
            && ctx
                .types
                .local_types
                .get(name)
                .is_some_and(|t| !t.contains("Error"));
    }
    ctx.ownership.ident_uses.get(name).copied().unwrap_or(1) > 1
}

/// Structurally transform a `RustExpr` tree to suppress `?` / `map_err(...)?`
/// inside closure bodies. Closures don't return `Result`, so the try operator
/// is invalid — replace with `.unwrap()`.
pub fn suppress_try_in_closure(expr: RustExpr) -> RustExpr {
    match expr {
        RustExpr::Try(inner) => unwrap_of(suppress_try_in_closure(*inner)),
        RustExpr::MapErr { inner, .. } => unwrap_of(suppress_try_in_closure(*inner)),
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
            unwrap_of(inner)
        }
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
        RustExpr::Clone(inner) => RustExpr::Clone(Box::new(suppress_try_in_closure(*inner))),
        RustExpr::Borrow { inner, mutable } => RustExpr::Borrow {
            inner: Box::new(suppress_try_in_closure(*inner)),
            mutable,
        },
        RustExpr::Await(inner) => RustExpr::Await(Box::new(suppress_try_in_closure(*inner))),
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
            then_body: map_vec(then_body),
            else_body: else_body.map(map_vec),
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
                .map(|Arm { pattern, guard, body }| Arm {
                    pattern,
                    guard: guard.map(|g| Box::new(suppress_try_in_closure(*g))),
                    body: map_vec(body),
                })
                .collect(),
        },
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
        RustExpr::JsonValue(inner) => {
            RustExpr::JsonValue(Box::new(suppress_try_in_closure(*inner)))
        }
        RustExpr::VecMacro(items) => {
            RustExpr::VecMacro(items.into_iter().map(suppress_try_in_closure).collect())
        }
        RustExpr::BinOp {
            left,
            op,
            right,
            ty,
        } => RustExpr::BinOp {
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
        RustExpr::StructLit {
            name,
            fields,
            rest,
            ty,
        } => RustExpr::StructLit {
            name,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, suppress_try_in_closure(v)))
                .collect(),
            rest: rest.map(|r| Box::new(suppress_try_in_closure(*r))),
            ty,
        },
        RustExpr::For {
            binding,
            iterable,
            body,
            ty,
        } => RustExpr::For {
            binding,
            iterable: Box::new(suppress_try_in_closure(*iterable)),
            body: map_vec(body),
            ty,
        },
        RustExpr::While {
            condition,
            body,
            ty,
        } => RustExpr::While {
            condition: Box::new(suppress_try_in_closure(*condition)),
            body: map_vec(body),
            ty,
        },
        RustExpr::Loop { body, ty } => RustExpr::Loop {
            body: map_vec(body),
            ty,
        },
        RustExpr::Assign { target, op, value } => RustExpr::Assign {
            target: Box::new(suppress_try_in_closure(*target)),
            op,
            value: Box::new(suppress_try_in_closure(*value)),
        },
        RustExpr::Closure { params, body } => RustExpr::Closure {
            params,
            body: map_vec(body),
        },
        RustExpr::Cast { expr, ty } => RustExpr::Cast {
            expr: Box::new(suppress_try_in_closure(*expr)),
            ty,
        },
        RustExpr::Range {
            start,
            end,
            inclusive,
        } => RustExpr::Range {
            start: start.map(|s| Box::new(suppress_try_in_closure(*s))),
            end: end.map(|e| Box::new(suppress_try_in_closure(*e))),
            inclusive,
        },
        RustExpr::IfLet {
            pattern,
            expr,
            then_body,
            else_body,
        } => RustExpr::IfLet {
            pattern,
            expr: Box::new(suppress_try_in_closure(*expr)),
            then_body: map_vec(then_body),
            else_body: else_body.map(map_vec),
        },
        RustExpr::WhileLet {
            pattern,
            expr,
            body,
        } => RustExpr::WhileLet {
            pattern,
            expr: Box::new(suppress_try_in_closure(*expr)),
            body: map_vec(body),
        },
        RustExpr::LayerTemplate {
            template,
            bindings,
        } => RustExpr::LayerTemplate {
            template,
            bindings: bindings
                .into_iter()
                .map(|(k, v)| (k, suppress_try_in_closure(v)))
                .collect(),
        },
        RustExpr::Join { items, sep } => RustExpr::Join {
            items: items.into_iter().map(suppress_try_in_closure).collect(),
            sep,
        },
        RustExpr::Ident { .. }
        | RustExpr::StringLit(_)
        | RustExpr::IntLit(_)
        | RustExpr::FloatLit(_)
        | RustExpr::BoolLit(_)
        | RustExpr::JsonNull
        | RustExpr::JsonEmptyArray
        | RustExpr::CompileError(_)
        | RustExpr::Break
        | RustExpr::Continue
        | RustExpr::Comment(_) => expr,
    }
}

fn map_vec(v: Vec<RustExpr>) -> Vec<RustExpr> {
    v.into_iter().map(suppress_try_in_closure).collect()
}

fn unwrap_of(inner: RustExpr) -> RustExpr {
    RustExpr::MethodCall {
        receiver: Box::new(inner),
        method: "unwrap".to_string(),
        args: vec![],
        ty: None,
        is_async: false,
        is_fallible: false,
    }
}
