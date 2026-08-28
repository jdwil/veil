//! VEIL AST → RustExpr lowering.
//!
//! `lower_to_rust` converts a VEIL `Expr` into the typed `RustExpr` IR.
//! Children are always nodes. `emit` is never called from this module.

use veil_ir::ast::{Expr, StringPart};
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::super::context::GenCtx;
use super::super::inference::{
    infer_expr_type, binop_to_rust, unaryop_to_rust, normalize_match_pattern, element_type_of,
    bus_message_name_from_args, bus_return_type_in_scope,
};
use super::super::types::{
    expr_is_numeric, flatten_str_add_chain, peel_option_rust, rust_ty_is_stringish,
    rust_ty_is_copy, rust_ty_is_unit_enum, field_access_is_copy, is_option_type, is_result_type,
    expr_handles_option_wrap, expr_is_stringish,
};
use super::super::calls::{
    resolve_self_field_name, is_json_rooted_expr, is_json_type_name, expr_is_json,
    list_index_get_ir, translate_call, receiver_call_finish, clone_args_ir, rust_method_name,
    arg_to_ir, json_message_ir, json_envelope_ir, param_types_for,
};
use super::super::patterns::{pattern_to_rust, pattern_to_rust_qualified, pattern_binding_names};
use super::super::actions::translate_action;
use super::super::analysis::analyze_mut_locals;
use super::{
    apply_finish, apply_ownership, assign, assign_op, clone_of, compile_error, field,
    fn_call, ident, ident_ty, is_none_node, is_unit_node, is_vec_node, lower_owned, lower_value,
    map_err_to_string, method, none, ok_or_not_found, owned_str, ret, ret_err, ret_ok, some_of,
    strip_try_ir, to_string_of, try_of, unit, wrap_as_option_ir, Arm, CallFinish, RustExpr,
    RustType,
};
use super::ownership::suppress_try_in_closure;

pub fn lower_to_rust(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    if ctx.option_value_wrap && !expr_handles_option_wrap(expr) {
        let mut inner_ctx = ctx.clone_for_inference();
        inner_ctx.option_value_wrap = false;
        let inner = lower_to_rust_inner(expr, &inner_ctx);
        return wrap_as_option_ir(expr, inner, ctx);
    }
    lower_to_rust_inner(expr, ctx)
}

fn lower_to_rust_inner(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::StringLit(s) => RustExpr::StringLit(s.clone()),
        Expr::IntLit(n) => RustExpr::IntLit(*n),
        Expr::FloatLit(f) => RustExpr::FloatLit(*f),
        Expr::BoolLit(b) => RustExpr::BoolLit(*b),
        Expr::Ident(name) => lower_ident(name, expr, ctx),
        Expr::FieldAccess(base, field) => lower_field_access(base, field, expr, ctx),
        Expr::StringInterp(parts) => lower_string_interp(parts, ctx),
        Expr::BinaryOp(op) => lower_binary_op(expr, op, ctx),
        Expr::UnaryOp(op) => RustExpr::UnaryOp {
            op: unaryop_to_rust(&op.op).to_string(),
            expr: Box::new(lower_to_rust(&op.expr, ctx)),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },
        Expr::Call(call) => lower_call(call, ctx),
        Expr::Closure { params, body } => lower_closure(params, body, ctx),
        Expr::Assign(name, rhs, ty_ann) => lower_assign(name, rhs, ty_ann, false, expr, ctx),
        Expr::MutAssign(name, rhs, ty_ann) => lower_assign(name, rhs, ty_ann, true, expr, ctx),
        Expr::LetPattern(pattern, inner_expr, ty_ann) => RustExpr::Let {
            name: pattern_to_rust(pattern),
            mutable: false,
            ty: ty_ann.as_ref().map(crate::rust::type_to_rust),
            value: Box::new(lower_to_rust(inner_expr, ctx)),
        },
        Expr::IfExpr(ie) => lower_if(ie, ctx),
        Expr::Match(scrutinee, arms) => lower_match(scrutinee, arms, expr, ctx),
        Expr::ForLoop {
            binding,
            index,
            iterable,
            body,
        } => lower_for(binding, index.as_deref(), iterable, body, expr, ctx),
        Expr::WhileLoop { condition, body } => {
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
            RustExpr::While {
                condition: Box::new(lower_to_rust(condition, ctx)),
                body: lower_block_with_ctx(body, &body_ctx),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }
        Expr::Loop(body) => RustExpr::Loop {
            body: body.iter().map(|e| lower_to_rust(e, ctx)).collect(),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },
        Expr::Return(inner) => lower_return(inner, expr, ctx),
        Expr::Break => RustExpr::Break,
        Expr::Continue => RustExpr::Continue,
        Expr::ArrayLit(items) => {
            if items.iter().any(|e| matches!(e, Expr::Spread(_))) {
                lower_array_with_spreads(items, expr, ctx)
            } else {
                RustExpr::Array {
                    items: items.iter().map(|e| lower_to_rust(e, ctx)).collect(),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                }
            }
        }
        // Standalone spread outside an array/struct context has no Rust equivalent.
        // (Array-literal spreads are handled above; struct `..base` uses StructUpdate.)
        Expr::Spread(_) => compile_error(
            "spread (`...`) is only supported in array literals and struct updates for the Rust target",
        ),
        Expr::Tuple(items) => RustExpr::Tuple {
            items: items.iter().map(|e| lower_to_rust(e, ctx)).collect(),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },
        Expr::Await(inner) => RustExpr::Await(Box::new(lower_to_rust(inner, ctx))),
        Expr::Try(inner) => RustExpr::Try(Box::new(lower_to_rust(inner, ctx))),
        Expr::Require(inner) => lower_require(inner, expr, ctx),
        Expr::Index(base, idx) => lower_index(base, idx, expr, ctx),
        Expr::IndexAssign { target, value } => RustExpr::Assign {
            target: Box::new(lower_to_rust(target, ctx)),
            op: "=".to_string(),
            value: Box::new(lower_to_rust(value, ctx)),
        },
        // JS `new Class(args)` maps to Rust `Class::new(args)`.
        Expr::New { class, args } => RustExpr::FnCall {
            path: format!("{}::new", class),
            args: args.iter().map(|a| lower_to_rust(a, ctx)).collect(),
            ty: None,
        },
        Expr::Action(a) => translate_action(a, ctx),
        Expr::Range {
            start,
            end,
            inclusive,
        } => RustExpr::Range {
            start: start.as_ref().map(|e| Box::new(lower_to_rust(e, ctx))),
            end: end.as_ref().map(|e| Box::new(lower_to_rust(e, ctx))),
            inclusive: *inclusive,
        },
        Expr::Cast(inner_expr, ty) => RustExpr::Cast {
            expr: Box::new(lower_to_rust(inner_expr, ctx)),
            ty: ty.clone(),
        },
        Expr::StructLit(name, fields) => lower_struct_lit(name, fields, expr, ctx),
        Expr::StructUpdate { name, fields, base } => RustExpr::StructLit {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.clone(), lower_value(v, ctx)))
                .collect(),
            rest: Some(Box::new(lower_to_rust(base, ctx))),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },
        Expr::IfLet {
            pattern,
            expr: inner_expr,
            then_body,
            else_body,
        } => RustExpr::IfLet {
            pattern: pattern.clone(),
            expr: Box::new(lower_to_rust(inner_expr, ctx)),
            then_body: then_body.iter().map(|e| lower_to_rust(e, ctx)).collect(),
            else_body: else_body
                .as_ref()
                .map(|eb| eb.iter().map(|e| lower_to_rust(e, ctx)).collect()),
        },
        Expr::WhileLet {
            pattern,
            expr: inner_expr,
            body,
        } => RustExpr::WhileLet {
            pattern: pattern.clone(),
            expr: Box::new(lower_to_rust(inner_expr, ctx)),
            body: body.iter().map(|e| lower_to_rust(e, ctx)).collect(),
        },
        Expr::DoBlock(body) => lower_do_block(body, expr, ctx),
        Expr::Stock => compile_error("stock not expanded"),
    }
}

/// Lower an array literal that contains one or more spread (`...`) elements to a
/// block that builds a `Vec` by extending from spreads and pushing plain items:
/// `{ let mut __veil_spread = Vec::new(); __veil_spread.extend(base); __veil_spread.push(x); __veil_spread }`.
fn lower_array_with_spreads(items: &[Expr], expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
    let tmp = "__veil_spread";
    let mut stmts: Vec<RustExpr> = Vec::with_capacity(items.len() + 1);
    // let mut __veil_spread = Vec::new();
    stmts.push(RustExpr::Let {
        name: tmp.to_string(),
        mutable: true,
        ty: None,
        value: Box::new(RustExpr::FnCall {
            path: "Vec::new".to_string(),
            args: vec![],
            ty: None,
        }),
    });
    for item in items {
        let recv = Box::new(RustExpr::Ident {
            name: tmp.to_string(),
            ty: None,
        });
        match item {
            Expr::Spread(inner) => {
                // __veil_spread.extend(<inner>);
                stmts.push(RustExpr::MethodCall {
                    receiver: recv,
                    method: "extend".to_string(),
                    args: vec![lower_to_rust(inner, ctx)],
                    ty: None,
                    is_async: false,
                    is_fallible: false,
                });
            }
            other => {
                // __veil_spread.push(<item>);
                stmts.push(RustExpr::MethodCall {
                    receiver: recv,
                    method: "push".to_string(),
                    args: vec![lower_to_rust(other, ctx)],
                    ty: None,
                    is_async: false,
                    is_fallible: false,
                });
            }
        }
    }
    RustExpr::Block {
        stmts,
        value: Some(Box::new(RustExpr::Ident {
            name: tmp.to_string(),
            ty,
        })),
    }
}

fn lower_binary_op(expr: &Expr, op: &veil_ir::ast::BinaryOpExpr, ctx: &GenCtx) -> RustExpr {
    let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
    let l_node = lower_to_rust(&op.left, ctx);
    let r_node = lower_to_rust(&op.right, ctx);
    if is_none_node(&r_node) {
        match op.op {
            veil_ir::ast::BinOp::NotEq => {
                return method(l_node, "is_some", vec![]);
            }
            veil_ir::ast::BinOp::Eq => {
                return method(l_node, "is_none", vec![]);
            }
            _ => {}
        }
    } else if is_none_node(&l_node) {
        match op.op {
            veil_ir::ast::BinOp::NotEq => {
                return method(r_node, "is_some", vec![]);
            }
            veil_ir::ast::BinOp::Eq => {
                return method(r_node, "is_none", vec![]);
            }
            _ => {}
        }
    }
    if matches!(op.op, veil_ir::ast::BinOp::Add) && (is_vec_node(&r_node) || is_vec_node(&l_node)) {
        return RustExpr::Block {
            stmts: vec![
                RustExpr::Let {
                    name: "__v".to_string(),
                    mutable: true,
                    ty: None,
                    value: Box::new(l_node),
                },
                method(ident("__v"), "extend", vec![r_node]),
            ],
            value: Some(Box::new(ident("__v"))),
        };
    }
    if matches!(op.op, veil_ir::ast::BinOp::Add)
        && (expr_is_stringish(&op.left, ctx) || expr_is_stringish(&op.right, ctx))
        && !(expr_is_numeric(&op.left, ctx) && expr_is_numeric(&op.right, ctx))
    {
        let parts = flatten_str_add_chain(expr);
        if parts.len() >= 2 {
            let args: Vec<RustExpr> = parts
                .into_iter()
                .map(|p| apply_ownership(lower_to_rust(p, ctx), ctx))
                .collect();
            let holes = vec!["{}"; args.len()].join("");
            return RustExpr::Format {
                template: holes,
                args,
            };
        }
        return RustExpr::Format {
            template: "{}{}".to_string(),
            args: vec![
                apply_ownership(l_node, ctx),
                apply_ownership(r_node, ctx),
            ],
        };
    }
    RustExpr::BinOp {
        left: Box::new(l_node),
        op: binop_to_rust(&op.op).to_string(),
        right: Box::new(r_node),
        ty,
    }
}

fn lower_assign(
    name: &str,
    rhs: &Expr,
    ty_ann: &Option<veil_ir::ast::TypeExpr>,
    force_mut: bool,
    expr: &Expr,
    ctx: &GenCtx,
) -> RustExpr {
    let _ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
    if let Expr::BinaryOp(bin) = rhs
        && matches!(bin.op, veil_ir::ast::BinOp::Add)
        && let (Expr::Ident(left), Expr::ArrayLit(items)) = (bin.left.as_ref(), bin.right.as_ref())
        && left == name
        && items.len() == 1
    {
        let item = if let Expr::Ident(item_name) = &items[0]
            && let Some(item_ty) = ctx.local_type(item_name)
            && is_option_type(item_ty)
        {
            ok_or_not_found(clone_of(ident(item_name.clone())), ctx)
        } else {
            lower_value(&items[0], ctx)
        };
        return method(ident(name), "push", vec![item]);
    }
    if let Expr::Call(call) = rhs {
        let bare_m = call.method.trim_end_matches('!');
        if bare_m == "concat" && call.target == *name && !call.args.is_empty()
            && let Some(Expr::ArrayLit(items)) = call.args.first()
        {
            if items.len() == 1 {
                return method(ident(name), "push", vec![lower_value(&items[0], ctx)]);
            }
            return method(
                ident(name),
                "extend",
                vec![RustExpr::Array {
                    items: items.iter().map(|i| lower_value(i, ctx)).collect(),
                    ty: None,
                }],
            );
        }
    }
    let rhs_node = match rhs {
        Expr::StringLit(s) => owned_str(s),
        _ => lower_value(rhs, ctx),
    };
    if name.contains('.') {
        let parts: Vec<&str> = name.split('.').collect();
        let base_name = parts[0];
        let mut target = ident(base_name);
        if let Some(local_ty) = ctx.local_type(base_name)
            && is_option_type(local_ty)
        {
            target = try_of(method(
                method(ident(base_name), "as_mut", vec![]),
                "ok_or",
                vec![ident(ctx.error_model.not_found_path())],
            ));
        }
        for (i, seg) in parts.iter().skip(1).enumerate() {
            let field_name = to_snake(seg);
            target = field(target, field_name);
            let _ = i;
        }
        return assign(target, rhs_node);
    }
    if ctx.state_locals.contains(name) {
        return assign(
            RustExpr::Index {
                base: Box::new(ident("state")),
                index: Box::new(RustExpr::StringLit(name.to_string())),
                ty: Some(RustType::Json),
            },
            RustExpr::JsonValue(Box::new(rhs_node)),
        );
    }
    if ctx.in_method && ctx.self_fields.contains(name) {
        return assign(field(ident("self"), to_snake(name)), rhs_node);
    }
    if ctx.is_local(name) {
        if let Expr::BinaryOp(bin) = rhs
            && let Expr::Ident(left) = bin.left.as_ref()
            && left == name
        {
            let op_str = match bin.op {
                veil_ir::ast::BinOp::Add => Some("+="),
                veil_ir::ast::BinOp::Sub => Some("-="),
                veil_ir::ast::BinOp::Mul => Some("*="),
                _ => None,
            };
            if let Some(op) = op_str {
                return assign_op(ident(name), op, lower_value(&bin.right, ctx));
            }
        }
        return assign(ident(name), rhs_node);
    }
    let is_mutable = force_mut || ctx.ownership.mut_locals.contains(name);
    RustExpr::Let {
        name: name.to_string(),
        mutable: is_mutable,
        ty: ty_ann.as_ref().map(crate::rust::type_to_rust),
        value: Box::new(rhs_node),
    }
}

fn lower_if(ie: &veil_ir::ast::IfExprData, ctx: &GenCtx) -> RustExpr {
    let mut cond_ctx = ctx.clone_for_inference();
    cond_ctx.option_value_wrap = false;
    let mut condition = lower_to_rust(&ie.condition, &cond_ctx);
    if let Expr::Ident(name) = ie.condition.as_ref()
        && ctx.local_type(name) == Some("serde_json::Value")
    {
        condition = method(
            method(ident(name.clone()), "as_bool", vec![]),
            "unwrap_or",
            vec![RustExpr::BoolLit(false)],
        );
    }
    let then_is_stmt = matches!(
        ie.then_body.first(),
        Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _))
    );
    let else_is_stmt = ie.else_body.as_ref().is_some_and(|b| {
        matches!(
            b.first(),
            Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _))
        )
    });
    if ie.then_body.len() == 1
        && ie.else_body.as_ref().is_some_and(|b| b.len() == 1)
        && !then_is_stmt
        && !else_is_stmt
    {
        return RustExpr::If {
            condition: Box::new(condition),
            then_body: lower_value_block(&ie.then_body, ctx),
            else_body: Some(lower_value_block(ie.else_body.as_ref().unwrap(), ctx)),
        };
    }
    if ctx.option_value_wrap {
        let then_body = lower_value_block(&ie.then_body, ctx);
        let else_body = if let Some(eb) = &ie.else_body {
            Some(lower_value_block(eb, ctx))
        } else {
            Some(vec![none()])
        };
        RustExpr::If {
            condition: Box::new(condition),
            then_body,
            else_body,
        }
    } else {
        RustExpr::If {
            condition: Box::new(condition),
            then_body: lower_block(&ie.then_body, ctx),
            else_body: ie.else_body.as_ref().map(|eb| lower_block(eb, ctx)),
        }
    }
}

fn lower_match(
    scrutinee: &Expr,
    arms: &[veil_ir::ast::MatchArm],
    expr: &Expr,
    ctx: &GenCtx,
) -> RustExpr {
    let mut scrut_ctx = ctx.clone_for_inference();
    scrut_ctx.option_value_wrap = false;
    let mut scrut = lower_to_rust(scrutinee, &scrut_ctx);
    let has_string_patterns = arms.iter().any(|a| a.pattern.starts_with('"'));
    if !has_string_patterns {
        scrut = strip_try_ir(scrut);
    }
    if let Expr::Ident(name) = scrutinee {
        if ctx.local_type(name) == Some("serde_json::Value") {
            let first_pat = arms.first().map(|a| &a.pattern).cloned().unwrap_or_default();
            let has_enum_pat = first_pat.contains("::")
                || first_pat.contains('.')
                || first_pat
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
            if has_enum_pat && !first_pat.starts_with('"') && first_pat != "_" {
                let enum_type = first_pat
                    .split(['.', ':'])
                    .next()
                    .unwrap_or(&first_pat)
                    .split('{')
                    .next()
                    .unwrap_or(&first_pat)
                    .trim();
                scrut = method(
                    fn_call(
                        format!("serde_json::from_value::<{enum_type}>"),
                        vec![clone_of(ident(name.clone()))],
                    ),
                    "unwrap",
                    vec![],
                );
            }
        }
    }
    if has_string_patterns {
        let already_str = matches!(
            &scrut,
            RustExpr::MethodCall { method, .. } if method == "as_str" || method == "trim"
        );
        if !already_str {
            scrut = method(scrut, "as_str", vec![]);
        }
    }
    let has_enum_patterns = arms
        .iter()
        .any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
    let scrutinee_is_local_ident = if let Expr::Ident(name) = scrutinee {
        ctx.is_local(name) && !has_string_patterns
    } else {
        false
    };
    if scrutinee_is_local_ident && has_enum_patterns {
        scrut = clone_of(scrut);
    }
    let mut ir_arms: Vec<Arm> = Vec::new();
    for arm in arms {
        let pattern = if let Some(rich) = &arm.rich_pattern {
            pattern_to_rust_qualified(rich, Some(&ctx.enum_variants))
        } else {
            normalize_match_pattern(&arm.pattern, ctx)
        };
        let guard = arm.guard.as_ref().map(|g| Box::new(lower_to_rust(g, &scrut_ctx)));
        let mut arm_ctx = ctx.clone_for_inference();
        for name in pattern_binding_names(&arm.pattern) {
            arm_ctx.locals.insert(name);
        }
        arm_ctx.ownership.mut_locals.extend(analyze_mut_locals(&arm.body));
        let body: Vec<RustExpr> = if arm.body.len() == 1 {
            vec![lower_owned_value(&arm.body[0], &arm_ctx)]
        } else {
            lower_value_block(&arm.body, &arm_ctx)
        };
        ir_arms.push(Arm {
            pattern,
            guard,
            body,
        });
    }
    let has_wildcard = arms
        .iter()
        .any(|a| a.pattern == "_" || a.pattern == "else" || a.pattern.starts_with('_'));
    if has_enum_patterns && !has_wildcard {
        ir_arms.push(Arm {
            pattern: "_".to_string(),
            guard: None,
            body: vec![compile_error(
                "non-exhaustive match — add missing arm or wildcard",
            )],
        });
    }
    let _ = expr;
    RustExpr::Match {
        scrutinee: Box::new(scrut),
        arms: ir_arms,
    }
}

fn lower_owned_value(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::StringLit(s) => owned_str(s),
        _ => lower_to_rust(expr, ctx),
    }
}

fn lower_for(
    binding: &str,
    index: Option<&str>,
    iterable: &Expr,
    body: &[Expr],
    expr: &Expr,
    ctx: &GenCtx,
) -> RustExpr {
    let mut iter_node = lower_to_rust(iterable, ctx);
    let elem_copy = element_type_of(iterable, ctx)
        .as_deref()
        .is_some_and(|t| rust_ty_is_copy(t) || rust_ty_is_unit_enum(t, ctx));
    let iterable_is_call = matches!(iterable, Expr::Call(_));
    let already_ref = matches!(iter_node, RustExpr::Borrow { .. });
    let already_iter = matches!(
        &iter_node,
        RustExpr::MethodCall { method, .. } if method == "iter" || method == "into_iter"
    );
    if !elem_copy && !iterable_is_call && !already_ref && !already_iter {
        if let RustExpr::Clone(inner) = iter_node {
            iter_node = super::borrow_of(*inner);
        } else {
            iter_node = super::borrow_of(iter_node);
        }
    } else if matches!(iterable, Expr::FieldAccess(_, _))
        && !matches!(&iter_node, RustExpr::Clone(_))
        && !matches!(&iter_node, RustExpr::MethodCall { method, .. } if method == "iter")
    {
        iter_node = clone_of(iter_node);
    }
    let bind = if let Some(idx) = index {
        format!("({}, {})", idx, binding)
    } else {
        binding.to_string()
    };
    let mut body_ctx = ctx.clone_for_inference();
    body_ctx.locals.insert(binding.to_string());
    if let Some(elem) = element_type_of(iterable, ctx) {
        body_ctx.types.local_types.insert(binding.to_string(), elem);
    }
    if !elem_copy && matches!(iter_node, RustExpr::Borrow { .. }) {
        body_ctx.ownership.ref_elem_locals.insert(binding.to_string());
    }
    if let Some(idx) = index {
        body_ctx.locals.insert(idx.to_string());
    }
    body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
    if let Expr::Ident(name) = iterable
        && ctx.local_type(name).map(is_option_type).unwrap_or(false)
    {
        iter_node = method(iter_node, "unwrap_or_default", vec![]);
    }
    if index.is_some() {
        iter_node = method(iter_node, "enumerate", vec![]);
    }
    RustExpr::For {
        binding: bind,
        iterable: Box::new(iter_node),
        body: lower_block_with_ctx(body, &body_ctx),
        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
    }
}

fn lower_return(inner: &Expr, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let _ = expr;
    match inner {
        Expr::Tuple(items) if items.is_empty() => lower_return(&Expr::Ident("()".to_string()), expr, ctx),
        Expr::Ident(n) if n == "Ok" => ret_ok(unit()),
        Expr::Ident(n) if n == "Err" => ret_err(fn_call(
            ctx.error_model.external_path(),
            vec![owned_str("error")],
        )),
        Expr::Call(c) if c.target == "Err" && c.method.is_empty() => {
            if c.args.is_empty() {
                return ret_err(fn_call(
                    ctx.error_model.validation_path(),
                    vec![owned_str("error")],
                ));
            }
            let a = lower_owned(&c.args[0], ctx);
            let err_type = &ctx.error_model.type_name;
            if matches!(&a, RustExpr::Ident { name, .. } if name.starts_with(&format!("{err_type}::")))
                || (c.args.len() == 1 && matches!(&c.args[0], Expr::Ident(_)))
            {
                ret_err(a)
            } else {
                ret_err(fn_call(ctx.error_model.external_path(), vec![a]))
            }
        }
        Expr::Call(c) if c.target == "Ok" && c.method.is_empty() => {
            if c.args.is_empty() {
                ret_ok(unit())
            } else {
                ret_ok(lower_value(&c.args[0], ctx))
            }
        }
        _ => {
            let val = lower_owned_value(inner, ctx);
            let returns_result = ctx
                .expected_return_rust
                .as_deref()
                .map(is_result_type)
                .unwrap_or(true);
            let returns_option = ctx
                .expected_return_rust
                .as_deref()
                .map(|t| t.contains("Option<"))
                .unwrap_or(false);
            if !returns_result {
                if is_none_node(&val) || is_unit_node(&val) {
                    if returns_option {
                        ret(none())
                    } else if is_unit_node(&val) {
                        ret(val)
                    } else {
                        compile_error("null return on non-Option function")
                    }
                } else if returns_option && !matches!(&val, RustExpr::FnCall { path, .. } if path == "Some")
                {
                    ret(some_of(val))
                } else {
                    ret(val)
                }
            } else if is_none_node(&val) || is_unit_node(&val) {
                if returns_option {
                    ret_ok(none())
                } else if is_unit_node(&val) {
                    ret_ok(unit())
                } else {
                    ret_err(ident(ctx.error_model.not_found_path()))
                }
            } else if returns_option && !matches!(&val, RustExpr::FnCall { path, .. } if path == "Some")
            {
                if let Expr::Ident(name) = inner {
                    let local_ty = ctx.local_type(name);
                    if local_ty.map(is_option_type).unwrap_or(false) {
                        return ret_ok(val);
                    }
                    // If local type is unknown (not inferred), skip wrapping — the local
                    // likely already holds Option from a fetch_optional / similar method.
                    // Double-wrapping (Some(Option<T>)) is always wrong; missing Some is
                    // caught by the compiler as a concrete type error.
                    if local_ty.is_none() {
                        return ret_ok(val);
                    }
                }
                // Fallback: check inferred type of the expression directly.
                if let Some(inferred) = crate::expr::infer_expr_type(inner, ctx) {
                    if is_option_type(&inferred) {
                        return ret_ok(val);
                    }
                }
                ret_ok(some_of(val))
            } else {
                ret_ok(val)
            }
        }
    }
}

fn lower_require(inner: &Expr, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let node = lower_to_rust(inner, ctx);
    let ty = infer_expr_type(inner, ctx);
    let _ = expr;
    if expr_is_json(inner, ctx) || ty.as_deref().is_some_and(is_json_type_name) {
        return ok_or_not_found(as_str_map_owned(node), ctx);
    }
    let still_option = ty.as_deref().is_some_and(|t| peel_option_rust(t).is_some());
    if still_option {
        return ok_or_not_found(node, ctx);
    }
    if matches!(&node, RustExpr::Try(_))
        || ty.as_deref().is_some_and(|t| {
            rust_ty_is_stringish(t) || t == "i64" || t == "bool" || t.starts_with("Vec<")
        })
    {
        node
    } else {
        ok_or_not_found(node, ctx)
    }
}

fn as_str_map_owned(node: RustExpr) -> RustExpr {
    method(
        method(node, "as_str", vec![]),
        "map",
        vec![super::closure(
            vec!["s".to_string()],
            to_string_of(ident("s")),
        )],
    )
}

fn lower_index(base: &Expr, idx: &Expr, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let b = lower_to_rust(base, ctx);
    let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
    match idx {
        Expr::StringLit(s) => ok_or_not_found(
            method(
                method(b, "get", vec![RustExpr::StringLit(s.clone())]),
                "cloned",
                vec![],
            ),
            ctx,
        ),
        Expr::IntLit(n) => list_index_get_ir(b, RustExpr::IntLit(*n), base, ctx),
        other => {
            let i = lower_to_rust(other, ctx);
            let base_ty = match base {
                Expr::Ident(n) => ctx.local_type(n).unwrap_or(""),
                _ => "",
            };
            let idx_is_int = matches!(other, Expr::IntLit(_))
                || matches!(
                    other,
                    Expr::Ident(n) if matches!(
                        ctx.local_type(n),
                        Some("i64") | Some("i32") | Some("u64") | Some("u32") | Some("usize") | Some("isize")
                    )
                );
            if idx_is_int {
                list_index_get_ir(b, i, base, ctx)
            } else if base_ty.contains("Value") || base_ty == "Json" || base_ty.is_empty() {
                method(
                    method(method(b, "get", vec![method(i, "as_str", vec![])]), "cloned", vec![]),
                    "unwrap_or",
                    vec![RustExpr::JsonNull],
                )
            } else {
                RustExpr::Index {
                    base: Box::new(b),
                    index: Box::new(i),
                    ty,
                }
            }
        }
    }
}

fn lower_struct_lit(
    name: &str,
    fields: &[(String, Expr)],
    expr: &Expr,
    ctx: &GenCtx,
) -> RustExpr {
    let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
    if name.is_empty() {
        if fields.is_empty() {
            return RustExpr::JsonMacro { entries: vec![] };
        }
        let entries = fields
            .iter()
            .map(|(k, v)| (k.clone(), super::super::calls::to_json_arg_ir(v, ctx)))
            .collect();
        return RustExpr::JsonMacro { entries };
    }
    let mut ir_fields: Vec<(String, RustExpr)> = fields
        .iter()
        .map(|(k, v)| {
            let mut val = match v {
                Expr::StringLit(s) => owned_str(s),
                _ => lower_value(v, ctx),
            };
            if let Some(field_ty) = ctx.field_type(name, k) {
                let val_ty = match v {
                    Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
                    _ => infer_expr_type(v, ctx),
                };
                if val_ty.as_deref() == Some("serde_json::Value") {
                    let stripped = match val {
                        RustExpr::Clone(inner) => *inner,
                        other => other,
                    };
                    val = match field_ty {
                        "String" => method(
                            as_str_map_owned(stripped),
                            "unwrap_or_default",
                            vec![],
                        ),
                        "bool" => method(
                            method(stripped, "as_bool", vec![]),
                            "unwrap_or",
                            vec![RustExpr::BoolLit(false)],
                        ),
                        "i64" => method(
                            method(stripped, "as_i64", vec![]),
                            "unwrap_or",
                            vec![RustExpr::IntLit(0)],
                        ),
                        "f64" => method(
                            method(stripped, "as_f64", vec![]),
                            "unwrap_or",
                            vec![RustExpr::FloatLit(0.0)],
                        ),
                        t if is_option_type(t) => some_of(match stripped {
                            s => apply_ownership(s, ctx),
                        }),
                        _ => stripped,
                    };
                } else if field_ty == "serde_json::Value" || field_ty == "Option<serde_json::Value>"
                {
                    if is_option_type(field_ty) {
                        if is_none_node(&val) {
                            val = none();
                        } else {
                            val = some_of(RustExpr::JsonValue(Box::new(val)));
                        }
                    } else {
                        val = RustExpr::JsonValue(Box::new(val));
                    }
                } else if is_option_type(field_ty) && !is_none_node(&val) {
                    // Auto-wrap non-Option values in Some() when target field is Option<T>
                    let val_is_already_option = val_ty
                        .as_deref()
                        .map(|t| is_option_type(t))
                        .unwrap_or(false);
                    if !val_is_already_option {
                        val = some_of(val);
                    }
                }
            }
            (to_snake(k), val)
        })
        .collect();
    // Fill missing optional fields with None so the struct literal is complete
    if let Some(all_fields) = ctx.types.struct_fields.get(name) {
        let present: std::collections::HashSet<String> = ir_fields.iter().map(|(k, _)| k.clone()).collect();
        for (field_name, field_ty) in all_fields {
            let snake = to_snake(field_name);
            if !present.contains(&snake) && is_option_type(field_ty) {
                ir_fields.push((snake, none()));
            }
        }
    }
    RustExpr::StructLit {
        name: name.to_string(),
        fields: ir_fields,
        rest: None,
        ty,
    }
}

fn lower_do_block(body: &[Expr], expr: &Expr, ctx: &GenCtx) -> RustExpr {
    let _ = expr;
    if body.is_empty() {
        return RustExpr::Block {
            stmts: vec![],
            value: None,
        };
    }
    let mut block_ctx = ctx.clone_for_inference();
    let mut stmts = Vec::new();
    let mut value = None;
    for (i, e) in body.iter().enumerate() {
        let node = lower_to_rust(e, &block_ctx);
        if let Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) = e
            && !name.contains('.')
        {
            block_ctx.locals.insert(name.clone());
            if let Some(ty) = ty_ann {
                block_ctx
                    .types
                    .local_types
                    .insert(name.clone(), crate::rust::type_to_rust(ty));
            } else if let Some(t) = infer_expr_type(rhs, &block_ctx) {
                block_ctx.types.local_types.insert(name.clone(), t);
            }
        }
        if i + 1 == body.len() {
            value = Some(Box::new(node));
        } else {
            stmts.push(node);
        }
    }
    RustExpr::Block { stmts, value }
}

fn lower_block(body: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    let mut body_ctx = ctx.clone_for_inference();
    body_ctx.option_value_wrap = false;
    body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
    lower_block_with_ctx(body, &body_ctx)
}

fn lower_value_block(body: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    let mut body_ctx = ctx.clone_for_inference();
    body_ctx.option_value_wrap = false;
    body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
    let mut result = Vec::new();
    for (i, e) in body.iter().enumerate() {
        let is_last = i + 1 == body.len();
        if is_last {
            body_ctx.option_value_wrap = ctx.option_value_wrap;
        }
        let node = lower_to_rust(e, &body_ctx);
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e
            && !name.contains('.')
        {
            body_ctx.locals.insert(name.clone());
            if let Some(t) = infer_expr_type(rhs, &body_ctx) {
                body_ctx.types.local_types.insert(name.clone(), t);
            }
        }
        result.push(node);
    }
    result
}

fn lower_block_with_ctx(body: &[Expr], body_ctx: &GenCtx) -> Vec<RustExpr> {
    let mut ctx = body_ctx.clone_for_inference();
    let mut result = Vec::new();
    for e in body {
        let node = lower_to_rust(e, &ctx);
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e
            && !name.contains('.')
        {
            ctx.locals.insert(name.clone());
            if let Some(t) = infer_expr_type(rhs, &ctx) {
                ctx.types.local_types.insert(name.clone(), t);
            }
        }
        result.push(node);
    }
    result
}

fn lower_ident(name: &str, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    if name == "null" {
        return none();
    }
    if name == "noop" {
        return RustExpr::Block {
            stmts: vec![],
            value: None,
        };
    }
    if name.contains(" then ") && (name.contains("f\"") || name.contains("f'")) {
        // Parser leftover: inline ternary jammed into an ident. Lower as a comment
        // plus the original text as a compile_error so we do not silently emit junk.
        return compile_error(format!("unparsed inline ternary ident: {name}"));
    }
    if name.contains(".unwrap_or(\"") && name.ends_with("\")") {
        // Parser leftover: rewrite structurally if it matches `x.unwrap_or("lit")`.
        if let Some((recv, rest)) = name.split_once(".unwrap_or(\"") {
            let lit = rest.trim_end_matches("\")");
            return method(ident(recv), "unwrap_or", vec![owned_str(lit)]);
        }
    }
    if ctx.state_locals.contains(name) {
        return RustExpr::Index {
            base: Box::new(ident("state")),
            index: Box::new(RustExpr::StringLit(name.to_string())),
            ty: Some(RustType::Json),
        };
    }
    if ctx.in_method && !ctx.locals.contains(name) {
        if let Some(rf) = resolve_self_field_name(ctx, name) {
            if ctx.ownership.borrow_fields.contains(rf.as_str()) {
                return super::borrow_of(field(ident("self"), rf));
            }
            return clone_of(field(ident("self"), rf));
        }
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            if let Some(enum_ty) = ctx.enum_variants.get(name) {
                return ident_ty(
                    format!("{enum_ty}::{name}"),
                    infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                );
            }
            return ident_ty(
                name,
                infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            );
        }
        return ident_ty(
            name,
            infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        );
    }
    if !ctx.locals.contains(name)
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        && let Some(enum_ty) = ctx.enum_variants.get(name)
    {
        return ident_ty(
            format!("{enum_ty}::{name}"),
            infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        );
    }
    ident_ty(
        name,
        infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
    )
}

fn lower_field_access(base: &Expr, field_name: &str, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    if field_name == "is_some" || field_name == "is_none" {
        return method(lower_to_rust(base, ctx), field_name, vec![]);
    }
    if let Expr::Ident(name) = base
        && ctx.state_locals.contains(name.as_str())
    {
        return RustExpr::Index {
            base: Box::new(RustExpr::Index {
                base: Box::new(ident("state")),
                index: Box::new(RustExpr::StringLit(name.clone())),
                ty: Some(RustType::Json),
            }),
            index: Box::new(RustExpr::StringLit(field_name.to_string())),
            ty: Some(RustType::Json),
        };
    }
    if let Expr::Ident(name) = base
        && name == "self"
        && ctx.in_method
    {
        let f = resolve_self_field_name(ctx, field_name).unwrap_or_else(|| to_snake(field_name));
        if ctx.ownership.borrow_fields.contains(f.as_str()) {
            return super::borrow_of(field(ident("self"), f));
        }
        if ctx.self_fields.contains(field_name)
            || ctx.self_fields.contains(&f)
            || ctx.self_field_types.contains_key(&f)
        {
            return clone_of(field(ident("self"), f));
        }
        return field(ident("self"), f);
    }
    if let Expr::Ident(name) = base {
        let field_is_variant = field_name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false);
        if matches!(ctx.name_to_shape.get(name.as_str()), Some(Shape::Enum)) {
            let variant = if field_is_variant {
                field_name.to_string()
            } else {
                field_name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default()
                    + &field_name[1..]
            };
            return ident_ty(
                format!("{}::{}", name, variant),
                infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            );
        }
        if field_is_variant
            && let Some((crate_name, path_type)) = ctx.stubs.stub_type_crate.get(name.as_str())
        {
            return ident_ty(
                format!("{}::{}::{}", crate_name, path_type, field_name),
                infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            );
        }
        if !field_is_variant
            && let Some((crate_name, path_type)) = ctx.stubs.stub_type_crate.get(name.as_str())
        {
            let variant: String = field_name
                .split('_')
                .map(|seg| {
                    let mut chars = seg.chars();
                    match chars.next() {
                        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect();
            return ident_ty(
                format!("{}::{}::{}", crate_name, path_type, variant),
                infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            );
        }
    }
    if let Expr::Ident(name) = base
        && ctx.is_local(name)
        && ctx.local_type(name) == Some("serde_json::Value")
    {
        return RustExpr::Index {
            base: Box::new(ident_ty(name.clone(), Some(RustType::Json))),
            index: Box::new(RustExpr::StringLit(field_name.to_string())),
            ty: Some(RustType::Json),
        };
    }
    if is_json_rooted_expr(base, ctx) {
        return RustExpr::Index {
            base: Box::new(lower_to_rust(base, ctx)),
            index: Box::new(RustExpr::StringLit(field_name.to_string())),
            ty: Some(RustType::Json),
        };
    }
    if let Expr::Ident(name) = base
        && let Some(ty) = ctx.local_type(name)
        && is_option_type(ty)
    {
        let enclosing_returns_option = ctx
            .expected_return_rust
            .as_deref()
            .map(is_option_type)
            .unwrap_or(false);
        let base_node = clone_of(lower_to_rust(base, ctx));
        let unwrapped = if enclosing_returns_option {
            try_of(base_node)
        } else {
            ok_or_not_found(base_node, ctx)
        };
        return field(unwrapped, to_snake(field_name));
    }
    let fa = RustExpr::FieldAccess {
        base: Box::new(lower_to_rust(base, ctx)),
        field: to_snake(field_name),
        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
    };
    if infer_expr_type(expr, ctx)
        .as_deref()
        .is_some_and(is_json_type_name)
    {
        return fa;
    }
    if field_access_is_copy(base, field_name, ctx) {
        return fa;
    }
    clone_of(fa)
}

pub(super) fn lower_string_interp(parts: &[StringPart], ctx: &GenCtx) -> RustExpr {
    let mut fmt = String::new();
    let mut args: Vec<RustExpr> = Vec::new();
    for p in parts {
        match p {
            StringPart::Literal(l) => {
                for ch in l.chars() {
                    match ch {
                        '{' => fmt.push_str("{{"),
                        '}' => fmt.push_str("}}"),
                        _ => fmt.push(ch),
                    }
                }
            }
            StringPart::Expr(e) => {
                fmt.push_str("{}");
                args.push(lower_to_rust(e, ctx));
            }
        }
    }
    if args.is_empty() {
        let raw: String = parts
            .iter()
            .filter_map(|p| match p {
                StringPart::Literal(l) => Some(l.as_str()),
                _ => None,
            })
            .collect();
        to_string_of(RustExpr::StringLit(raw))
    } else {
        RustExpr::Format {
            template: fmt,
            args,
        }
    }
}

fn lower_call_method_template(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    if !ctx.is_trait_target(&call.target) {
        return None;
    }
    let method_key = call.method.trim_end_matches(['!', '?']);
    let targets = ctx.method_lowers_to.get(&(call.target.clone(), method_key.to_string()))?;
    let template = targets.get("rust")?;
    let dep_name = ctx.deps_field_for(&call.target);
    let args: Vec<RustExpr> = call.args.iter().map(|a| lower_value(a, ctx)).collect();
    let args_rendered: Vec<String> = args.iter().map(|a| super::emit(a)).collect();
    let args_str = args_rendered.join(", ");
    let mut rendered = template.clone();
    rendered = rendered.replace("{dep}", &dep_name);
    rendered = rendered.replace("{args}", &args_str);
    for (i, arg) in args_rendered.iter().enumerate() {
        rendered = rendered.replace(&format!("{{arg{i}}}"), arg);
    }
    Some(RustExpr::LayerTemplate {
        template: rendered.trim().to_string(),
        bindings: vec![],
    })
}

const SPECIAL_METHODS: &[&str] = &[
    "get", "as_str", "as_s", "as_n", "trim", "unwrap", "unwrap_or", "unwrap_or_else", "body",
    "limit", "parse_int", "parse_json", "first",
];

fn is_special_method(method: &str) -> bool {
    let bare = method.trim_end_matches(['!', '?']);
    SPECIAL_METHODS.contains(&bare)
}

fn lower_call_builder_chain(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    let recv = call.receiver.as_ref()?;
    if !matches!(recv.as_ref(), Expr::Call(_)) {
        return None;
    }
    if is_special_method(&call.method) {
        return None;
    }
    let suffix = receiver_call_finish(recv, &call.method, ctx);
    if suffix.is_bare() {
        return None;
    }
    if matches!(suffix, CallFinish::MapErrOwnStr) {
        return None;
    }
    let receiver_ir = lower_chain_receiver(recv, ctx);
    let method_name = rust_method_name(&call.method);
    let recv_lookup: Option<&str> = match recv.as_ref() {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::FieldAccess(_, field) => Some(field.as_str()),
        _ => None,
    };
    let args_ir = clone_args_ir(recv_lookup, &call.method, &call.args, ctx);
    Some(apply_finish(
        method(receiver_ir, method_name, args_ir),
        suffix,
        &ctx.error_model,
    ))
}

fn lower_chain_receiver(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::Call(inner_call) => {
            if let Some(inner_recv) = &inner_call.receiver {
                if is_special_method(&inner_call.method) {
                    return translate_call(inner_call, ctx);
                }
                let receiver_ir = lower_chain_receiver(inner_recv, ctx);
                let method_name = rust_method_name(&inner_call.method);
                let recv_lookup: Option<&str> = match inner_recv.as_ref() {
                    Expr::Ident(name) => Some(name.as_str()),
                    Expr::FieldAccess(_, field) => Some(field.as_str()),
                    _ => None,
                };
                let args_ir =
                    clone_args_ir(recv_lookup, &inner_call.method, &inner_call.args, ctx);
                let call_node = method(receiver_ir, method_name, args_ir);
                // If this intermediate method has a bang, it's async+fallible —
                // apply .await.map_err(...)? here to split the chain at this point.
                if inner_call.method.ends_with('!') {
                    let finish = receiver_call_finish(
                        inner_recv,
                        &inner_call.method,
                        ctx,
                    );
                    if !finish.is_bare() {
                        return apply_finish(call_node, finish, &ctx.error_model);
                    }
                }
                call_node
            } else {
                translate_call(inner_call, ctx)
            }
        }
        _ => lower_to_rust(expr, ctx),
    }
}

fn lower_call_trait_dep(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    if !ctx.is_trait_target(&call.target) {
        return None;
    }
    if call.sugar.is_some() {
        return None;
    }
    let dep_name = ctx.deps_field_for(&call.target);
    let method_name = if call.method.is_empty() {
        "call"
    } else {
        &call.method
    };
    let method_key = method_name.trim_end_matches(['!', '?']);
    let param_tys = param_types_for(Some(call.target.as_str()), method_key, ctx);
    let args_ir: Vec<RustExpr> = call
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let expected = param_tys.get(i).map(|s| s.as_str());
            match a {
                Expr::Ident(name) if ctx.local_type(name) == Some("serde_json::Value") => {
                    clone_of(ident(name.clone()))
                }
                Expr::Ident(name)
                    if ctx
                        .local_type(name)
                        .map(|t| t.starts_with("Option<"))
                        .unwrap_or(false) =>
                {
                    let expects_opt = expected
                        .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                        .unwrap_or(false);
                    if expects_opt {
                        clone_of(ident(name.clone()))
                    } else {
                        ok_or_not_found(clone_of(ident(name.clone())), ctx)
                    }
                }
                _ => arg_to_ir(a, expected, ctx),
            }
        })
        .collect();
    let has_bang = method_name.ends_with('!');
    let ret_type = ctx.return_type_of(&call.target, method_name).or_else(|| {
        ctx.dep_fields
            .iter()
            .find(|(_, v)| *v == &call.target)
            .and_then(|(trait_name, _)| ctx.return_type_of(trait_name, method_name))
    });
    let is_fallible = if has_bang {
        true
    } else {
        match ret_type {
            Some("bool") | Some("Bool") | Some("i64") | Some("f64") | Some("String")
            | Some("()") | Some("") => false,
            Some(t) if t.starts_with("Option<") || t.starts_with("Opt<") => false,
            _ => true,
        }
    };
    let prefix = if ctx.in_method && ctx.self_fields.contains(&dep_name) {
        field(ident("self"), dep_name)
    } else {
        field(ident("deps"), dep_name)
    };
    Some(apply_finish(
        method(prefix, to_snake(method_key), args_ir),
        if is_fallible {
            CallFinish::AwaitTry
        } else {
            CallFinish::Await
        },
        &ctx.error_model,
    ))
}

pub(super) fn lower_call(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> RustExpr {
    if let Some(expr) = lower_call_method_template(call, ctx) {
        return expr;
    }
    if let Some(expr) = lower_call_builder_chain(call, ctx) {
        return expr;
    }
    if let Some(expr) = lower_call_trait_dep(call, ctx) {
        return expr;
    }
    translate_call(call, ctx)
}

fn lower_closure(params: &[String], body: &[Expr], ctx: &GenCtx) -> RustExpr {
    let mut closure_ctx = ctx.clone_for_inference();
    for param in params {
        closure_ctx.locals.insert(param.clone());
    }
    let body_exprs: Vec<RustExpr> = body
        .iter()
        .map(|e| suppress_try_in_closure(lower_to_rust(e, &closure_ctx)))
        .collect();
    RustExpr::Closure {
        params: params.to_vec(),
        body: body_exprs,
    }
}
