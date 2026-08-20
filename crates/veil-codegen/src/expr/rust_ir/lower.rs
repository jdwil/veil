//! VEIL AST → RustExpr lowering.
//!
//! `lower_to_rust` is the entry point: it converts a VEIL `Expr` into the
//! typed `RustExpr` IR. Per-expression helpers handle specific shapes
//! (idents, field access, calls, blocks, etc.).

use veil_ir::ast::{Expr, StringPart};
use veil_ir::layer::Shape;
use crate::rust::to_snake;
use super::super::context::GenCtx;
use super::super::translate::{expr_to_rust, to_json_arg};
use super::super::inference::{infer_expr_type, binop_to_rust, unaryop_to_rust, normalize_match_pattern, element_type_of};
use super::super::types::{rust_string_lit_owned, expr_is_stringish, expr_is_numeric,
    flatten_str_add_chain, clone_if_named_value, strip_try_suffix,
    peel_option_rust, rust_ty_is_stringish, rust_ty_is_copy, rust_ty_is_unit_enum,
    expr_to_rust_value, field_access_is_copy, rust_already_owned, rust_is_copy_value,
    should_clone_ident, is_option_type, is_result_type};
use super::super::calls::{resolve_self_field_name, is_json_rooted_expr, is_json_type_name,
    expr_is_json, list_index_get_rust};
use super::super::patterns::{pattern_to_rust, pattern_to_rust_qualified, pattern_binding_names,
    emit_value_block};
use super::super::actions::translate_action;
use super::super::analysis::analyze_mut_locals;
use super::{RustExpr, RustType, emit};
use super::ownership::suppress_try_in_closure;


pub fn lower_to_rust(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        // ── Migrated: literals ───────────────────────────────────────────
        Expr::StringLit(s) => RustExpr::StringLit(s.clone()),
        Expr::IntLit(n) => RustExpr::IntLit(*n),
        Expr::FloatLit(f) => RustExpr::FloatLit(*f),
        Expr::BoolLit(b) => RustExpr::BoolLit(*b),

        // ── Migrated: identifiers ────────────────────────────────────────
        Expr::Ident(name) => lower_ident(name, expr, ctx),

        // ── Migrated: field access ───────────────────────────────────────
        Expr::FieldAccess(base, field) => lower_field_access(base, field, expr, ctx),

        // ── Migrated: string interpolation ───────────────────────────────
        Expr::StringInterp(parts) => lower_string_interp(parts, ctx),

        // ── Migrated: binary and unary ops ───────────────────────────────
        Expr::BinaryOp(op) => {
            let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));
            let l_node = lower_to_rust(&op.left, ctx);
            let r_node = lower_to_rust(&op.right, ctx);
            let l = emit(&l_node);
            let r = emit(&r_node);
            // Special case: x != None → x.is_some(), x == None → x.is_none()
            if r == "None" {
                match op.op {
                    veil_ir::ast::BinOp::NotEq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(l_node),
                            method: "is_some".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    veil_ir::ast::BinOp::Eq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(l_node),
                            method: "is_none".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    _ => {}
                }
            } else if l == "None" {
                match op.op {
                    veil_ir::ast::BinOp::NotEq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(r_node),
                            method: "is_some".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    veil_ir::ast::BinOp::Eq => {
                        return RustExpr::MethodCall {
                            receiver: Box::new(r_node),
                            method: "is_none".to_string(),
                            args: vec![],
                            ty,
                            is_async: false,
                            is_fallible: false,
                        };
                    }
                    _ => {}
                }
            }
            // Vec concat: x + [items] → { let mut __v = x; __v.extend(items); __v }
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (r.starts_with("vec![") || l.starts_with("vec!["))
            {
                return RustExpr::Statement {
                    text: format!("{{ let mut __v = {l}; __v.extend({r}); __v }}"),
                    ty,
                };
            }
            // String concat: x + y → format!("{}{}", x, y)
            if matches!(op.op, veil_ir::ast::BinOp::Add)
                && (expr_is_stringish(&op.left, &l, ctx) || expr_is_stringish(&op.right, &r, ctx))
                && !(expr_is_numeric(&op.left, ctx) && expr_is_numeric(&op.right, ctx))
            {
                let parts = flatten_str_add_chain(expr);
                if parts.len() >= 2 {
                    let rendered: Vec<String> = parts
                        .into_iter()
                        .map(|p| {
                            let s = expr_to_rust(p, ctx);
                            clone_if_named_value(p, s)
                        })
                        .collect();
                    let holes = vec!["{}"; rendered.len()].join("");
                    let args = rendered.iter().map(|a| RustExpr::Statement { text: a.clone(), ty: None }).collect();
                    return RustExpr::Format { template: holes, args };
                } else {
                    let l = clone_if_named_value(&op.left, l);
                    let r = clone_if_named_value(&op.right, r);
                    return RustExpr::Format {
                        template: "{}{}".to_string(),
                        args: vec![
                            RustExpr::Statement { text: l, ty: None },
                            RustExpr::Statement { text: r, ty: None },
                        ],
                    };
                }
            }
            // Simple binary op
            RustExpr::BinOp {
                left: Box::new(l_node),
                op: binop_to_rust(&op.op).to_string(),
                right: Box::new(r_node),
                ty,
            }
        }
        Expr::UnaryOp(op) => {
            let inner_node = lower_to_rust(&op.expr, ctx);
            RustExpr::UnaryOp {
                op: unaryop_to_rust(&op.op).to_string(),
                expr: Box::new(inner_node),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: calls ──────────────────────────────────────────────
        Expr::Call(call) => lower_call(call, ctx),

        // ── Migrated: closures ───────────────────────────────────────────
        Expr::Closure { params, body } => lower_closure(params, body, ctx),

        // ── Migrated: assign ─────────────────────────────────────────────
        Expr::Assign(name, rhs, ty_ann) => {
            let text = {
                // List append sugar: `out = out + [x]` → `out.push(x)`
                if let Expr::BinaryOp(bin) = rhs.as_ref()
                    && matches!(bin.op, veil_ir::ast::BinOp::Add)
                        && let (Expr::Ident(left), Expr::ArrayLit(items)) =
                            (bin.left.as_ref(), bin.right.as_ref())
                            && left == name && items.len() == 1 {
                                let item = expr_to_rust(&items[0], ctx);
                                if let Expr::Ident(item_name) = &items[0]
                                    && let Some(ty) = ctx.local_type(item_name)
                                    && is_option_type(ty) {
                                            return RustExpr::MethodCall {
                                                receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                                method: "push".to_string(),
                                                args: vec![RustExpr::Statement {
                                                    text: format!("{}.clone().ok_or({})?", item, ctx.error_model.not_found_path()),
                                                    ty: None,
                                                }],
                                                ty: None,
                                                is_async: false,
                                                is_fallible: false,
                                            };
                                        }
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "push".to_string(),
                                    args: vec![RustExpr::Statement { text: item, ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            }
                // List concat sugar: `x = x.concat([items])` → `x.extend(vec![items])`
                if let Expr::Call(call) = rhs.as_ref() {
                    let bare_m = call.method.trim_end_matches('!');
                    if bare_m == "concat" && call.target == *name && !call.args.is_empty()
                        && let Some(Expr::ArrayLit(items)) = call.args.first() {
                            let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                            if items.len() == 1 {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "push".to_string(),
                                    args: vec![RustExpr::Statement { text: item_strs[0].clone(), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            } else {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "extend".to_string(),
                                    args: vec![RustExpr::Statement { text: format!("vec![{}]", item_strs.join(", ")), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            }
                        }
                }
                let rhs_str = match rhs.as_ref() {
                    Expr::StringLit(s) => rust_string_lit_owned(s),
                    _ => expr_to_rust(rhs, ctx),
                };
                // Field assignment: `wt.name = x`
                if name.contains('.') {
                    let parts: Vec<&str> = name.splitn(2, '.').collect();
                    let base_name = parts[0];
                    let field_path = parts[1];
                    if let Some(ty) = ctx.local_type(base_name)
                        && is_option_type(ty) {
                            let field_snake = field_path
                                .split('.')
                                .map(to_snake)
                                .collect::<Vec<_>>()
                                .join(".");
                            return RustExpr::Statement {
                                text: format!(
                                    "{}.as_mut().ok_or({})?.{} = {}",
                                    base_name, ctx.error_model.not_found_path(), field_snake, rhs_str
                                ),
                                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                            };
                        }
                    let path = name
                        .split('.')
                        .enumerate()
                        .map(|(i, seg)| {
                            if i == 0 {
                                seg.to_string()
                            } else {
                                to_snake(seg)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(".");
                    format!("{} = {}", path, rhs_str)
                } else if ctx.state_locals.contains(name.as_str()) {
                    format!("state[\"{}\"] = serde_json::json!({})", name, rhs_str)
                } else if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                    format!("self.{} = {}", to_snake(name), rhs_str)
                } else if ctx.is_local(name) {
                    // Already-declared local → reassignment
                    if let Expr::BinaryOp(bin) = rhs.as_ref()
                        && let Expr::Ident(left) = bin.left.as_ref()
                            && left == name {
                                let op_str = match bin.op {
                                    veil_ir::ast::BinOp::Add => Some("+="),
                                    veil_ir::ast::BinOp::Sub => Some("-="),
                                    veil_ir::ast::BinOp::Mul => Some("*="),
                                    _ => None,
                                };
                                if let Some(op) = op_str {
                                    let right_str = expr_to_rust(&bin.right, ctx);
                                    return RustExpr::BinOp {
                                        left: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                        op: op.to_string(),
                                        right: Box::new(RustExpr::Statement { text: right_str, ty: None }),
                                        ty: None,
                                    };
                                }
                            }
                    format!("{} = {}", name, rhs_str)
                } else {
                    let is_mutable = ctx.ownership.mut_locals.contains(name.as_str());
                    let ty_str = ty_ann.as_ref().map(crate::rust::type_to_rust);
                    return RustExpr::Let {
                        name: name.clone(),
                        mutable: is_mutable,
                        ty: ty_str,
                        value: Box::new(RustExpr::Statement { text: rhs_str, ty: None }),
                    };
                }
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: mut assign ─────────────────────────────────────────
        Expr::MutAssign(name, rhs, ty_ann) => {
            let text = {
                // List concat sugar
                if let Expr::Call(call) = rhs.as_ref() {
                    let bare_m = call.method.trim_end_matches('!');
                    if bare_m == "concat" && call.target == *name && !call.args.is_empty()
                        && let Some(Expr::ArrayLit(items)) = call.args.first() {
                            let item_strs: Vec<String> = items.iter().map(|i| expr_to_rust(i, ctx)).collect();
                            if items.len() == 1 {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "push".to_string(),
                                    args: vec![RustExpr::Statement { text: item_strs[0].clone(), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            } else {
                                return RustExpr::MethodCall {
                                    receiver: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                                    method: "extend".to_string(),
                                    args: vec![RustExpr::Statement { text: format!("vec![{}]", item_strs.join(", ")), ty: None }],
                                    ty: None,
                                    is_async: false,
                                    is_fallible: false,
                                };
                            }
                        }
                }
                let rhs_str = match rhs.as_ref() {
                    Expr::StringLit(s) => rust_string_lit_owned(s),
                    _ => expr_to_rust(rhs, ctx),
                };
                if ctx.is_local(name) {
                    format!("{} = {}", name, rhs_str)
                } else {
                    let ty_str = ty_ann.as_ref().map(crate::rust::type_to_rust);
                    return RustExpr::Let {
                        name: name.clone(),
                        mutable: true,
                        ty: ty_str,
                        value: Box::new(RustExpr::Statement { text: rhs_str, ty: None }),
                    };
                }
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: let pattern ────────────────────────────────────────
        Expr::LetPattern(pattern, inner_expr, ty_ann) => {
            let pat_str = pattern_to_rust(pattern);
            let e = lower_to_rust(inner_expr, ctx);
            RustExpr::Let {
                name: pat_str,
                mutable: false,
                ty: ty_ann.as_ref().map(crate::rust::type_to_rust),
                value: Box::new(e),
            }
        }

        // ── Migrated: if expression ──────────────────────────────────────
        Expr::IfExpr(ie) => {
            let mut cond_ctx = ctx.clone_for_inference();
            cond_ctx.option_value_wrap = false;
            let cond_str = expr_to_rust(&ie.condition, &cond_ctx);
            let cond_str = if let Expr::Ident(name) = ie.condition.as_ref() {
                if ctx.local_type(name) == Some("serde_json::Value") {
                    format!("{}.as_bool().unwrap_or(false)", name)
                } else { cond_str }
            } else { cond_str };

            let condition = Box::new(RustExpr::Statement { text: cond_str, ty: Some(RustType::Named("bool".to_string())) });

            let then_is_stmt = matches!(
                ie.then_body.first(),
                Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _))
            );
            let else_is_stmt = ie.else_body.as_ref().is_some_and(|b| {
                matches!(b.first(), Some(Expr::Assign(_, _, _) | Expr::MutAssign(_, _, _)))
            });

            // Simple ternary: single value expressions in then/else
            if ie.then_body.len() == 1
                && ie.else_body.as_ref().is_some_and(|b| b.len() == 1)
                && !then_is_stmt
                && !else_is_stmt
            {
                let then_body = lower_value_block(&ie.then_body, ctx);
                let else_body = lower_value_block(ie.else_body.as_ref().unwrap(), ctx);
                return RustExpr::If { condition, then_body, else_body: Some(else_body) };
            }

            // Multi-line: option-value-wrap or tracked block
            if ctx.option_value_wrap {
                let then_body = lower_value_block(&ie.then_body, ctx);
                let else_body = if let Some(eb) = &ie.else_body {
                    Some(lower_value_block(eb, ctx))
                } else {
                    // No else → add explicit None
                    Some(vec![RustExpr::Ident { name: "None".to_string(), ty: None }])
                };
                RustExpr::If { condition, then_body, else_body }
            } else {
                let then_body = lower_block(&ie.then_body, ctx);
                let else_body = ie.else_body.as_ref().map(|eb| lower_block(eb, ctx));
                RustExpr::If { condition, then_body, else_body }
            }
        }

        // ── Migrated: match ──────────────────────────────────────────────
        Expr::Match(scrutinee, arms) => {
            let text = {
                let mut scrut_ctx = ctx.clone_for_inference();
                scrut_ctx.option_value_wrap = false;
                let raw = expr_to_rust(scrutinee, &scrut_ctx);
                let has_string_patterns = arms.iter().any(|a| a.pattern.starts_with('"'));
                let scrutinee_str = if has_string_patterns {
                    raw.clone()
                } else {
                    strip_try_suffix(raw)
                };
                let scrutinee_str = if let Expr::Ident(name) = scrutinee.as_ref() {
                    if ctx.local_type(name) == Some("serde_json::Value") {
                        let first_pat = arms.first().map(|a| &a.pattern).cloned().unwrap_or_default();
                        let has_enum_pat = first_pat.contains("::")
                            || first_pat.contains('.')
                            || first_pat.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
                        if has_enum_pat && !first_pat.starts_with('"') && first_pat != "_" {
                            let enum_type = first_pat.split(['.', ':'])
                                .next().unwrap_or(&first_pat)
                                .split('{').next().unwrap_or(&first_pat).trim();
                            format!("serde_json::from_value::<{}>({}.clone()).unwrap()", enum_type, name)
                        } else {
                            scrutinee_str
                        }
                    } else {
                        scrutinee_str
                    }
                } else {
                    scrutinee_str
                };
                let scrutinee_final = if has_string_patterns {
                    let t = scrutinee_str.trim();
                    if t.ends_with(".as_str()") || t.ends_with(".as_str().trim()") {
                        scrutinee_str
                    } else {
                        format!("{scrutinee_str}.as_str()")
                    }
                } else {
                    scrutinee_str
                };
                let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
                let scrutinee_is_local_ident = if let Expr::Ident(name) = scrutinee.as_ref() {
                    ctx.is_local(name) && !has_string_patterns
                } else {
                    false
                };
                let scrutinee_final = if scrutinee_is_local_ident && has_enum_patterns {
                    format!("{}.clone()", scrutinee_final)
                } else {
                    scrutinee_final
                };
                let mut out = format!("match {} {{\n", scrutinee_final);
                for arm in arms {
                    let pattern = if let Some(rich) = &arm.rich_pattern {
                        pattern_to_rust_qualified(rich, Some(&ctx.enum_variants))
                    } else {
                        normalize_match_pattern(&arm.pattern, ctx)
                    };
                    let guard_str = match &arm.guard {
                        Some(g) => format!(" if {}", expr_to_rust(g, &scrut_ctx)),
                        None => String::new(),
                    };
                    let mut arm_ctx = ctx.clone_for_inference();
                    for name in pattern_binding_names(&arm.pattern) {
                        arm_ctx.locals.insert(name);
                    }
                    arm_ctx.ownership.mut_locals.extend(analyze_mut_locals(&arm.body));
                    let body_str = if arm.body.len() == 1 {
                        expr_to_rust_value(&arm.body[0], &arm_ctx)
                    } else {
                        format!(
                            "{{\n{}\n    }}",
                            emit_value_block(&arm.body, &arm_ctx, "        ")
                        )
                    };
                    out.push_str(&format!("        {}{} => {},\n", pattern, guard_str, body_str));
                }
                let has_enum_patterns = arms.iter().any(|a| a.pattern.contains('.') || a.pattern.contains("::"));
                let has_wildcard = arms.iter().any(|a| a.pattern == "_" || a.pattern == "else" || a.pattern.starts_with('_'));
                if has_enum_patterns && !has_wildcard {
                    out.push_str("        _ => compile_error!(\"non-exhaustive match — add missing arm or wildcard\")\n");
                }
                out.push_str("    }");
                out
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: for loop ───────────────────────────────────────────
        Expr::ForLoop { binding, index, iterable, body } => {
            let mut iter_str = expr_to_rust(iterable, ctx);
            let elem_copy = element_type_of(iterable, ctx)
                .as_deref()
                .is_some_and(|t| rust_ty_is_copy(t) || rust_ty_is_unit_enum(t, ctx));
            let iterable_is_call = matches!(iterable.as_ref(), Expr::Call(_));
            if !elem_copy
                && !iterable_is_call
                && !iter_str.starts_with('&')
                && !iter_str.ends_with(".iter()")
                && !iter_str.ends_with(".into_iter()")
            {
                let base = iter_str
                    .strip_suffix(".clone()")
                    .unwrap_or(iter_str.as_str());
                iter_str = format!("&{base}");
            } else if matches!(iterable.as_ref(), Expr::FieldAccess(_, _))
                && !iter_str.ends_with(".clone()")
                && !iter_str.ends_with(".iter()")
            {
                iter_str = format!("{iter_str}.clone()");
            }
            let bind = if let Some(idx) = index {
                format!("({}, {})", idx, binding)
            } else {
                binding.clone()
            };
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.locals.insert(binding.clone());
            if let Some(elem) = element_type_of(iterable, ctx) {
                body_ctx.types.local_types.insert(binding.clone(), elem);
            }
            if !elem_copy && iter_str.starts_with('&') {
                body_ctx.ownership.ref_elem_locals.insert(binding.clone());
            }
            if let Some(idx) = index {
                body_ctx.locals.insert(idx.clone());
            }
            body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
            let enumerate = if index.is_some() { ".enumerate()" } else { "" };
            let iter_expr = if let Expr::Ident(name) = iterable.as_ref() {
                if ctx
                    .local_type(name)
                    .map(is_option_type)
                    .unwrap_or(false)
                {
                    format!("{iter_str}.unwrap_or_default()")
                } else {
                    iter_str
                }
            } else {
                iter_str
            };
            let iterable_final = format!("{iter_expr}{enumerate}");
            // Lower body with the custom body_ctx
            let body_nodes = lower_block_with_ctx(body, &body_ctx);
            RustExpr::For {
                binding: bind,
                iterable: Box::new(RustExpr::Statement { text: iterable_final, ty: None }),
                body: body_nodes,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: while loop ─────────────────────────────────────────
        Expr::WhileLoop { condition, body } => {
            let cond_str = expr_to_rust(condition, ctx);
            let mut body_ctx = ctx.clone_for_inference();
            body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
            let body_nodes = lower_block_with_ctx(body, &body_ctx);
            RustExpr::While {
                condition: Box::new(RustExpr::Statement { text: cond_str, ty: Some(RustType::Named("bool".to_string())) }),
                body: body_nodes,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: loop ───────────────────────────────────────────────
        Expr::Loop(body) => {
            let body_exprs: Vec<RustExpr> = body.iter().map(|e| {
                lower_to_rust(e, ctx)
            }).collect();
            RustExpr::Loop {
                body: body_exprs,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: return ─────────────────────────────────────────────
        Expr::Return(inner) => {
            let text = match inner.as_ref() {
                Expr::Ident(n) if n == "Ok" => "return Ok(())".to_string(),
                Expr::Ident(n) if n == "Err" => {
                    format!("return Err({}(\"error\".to_string()))", ctx.error_model.external_path())
                }
                Expr::Call(c) if c.target == "Err" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust_value(e, ctx)).collect::<Vec<_>>().join(", ");
                    let err_type = &ctx.error_model.type_name;
                    if a.is_empty() {
                        format!("return Err({}(\"error\".to_string()))", ctx.error_model.validation_path())
                    } else if a.starts_with(&format!("{err_type}::")) {
                        format!("return Err({})", a)
                    } else {
                        let is_simple_ident = c.args.len() == 1 && matches!(&c.args[0], Expr::Ident(_));
                        if is_simple_ident {
                            format!("return Err({})", a)
                        } else {
                            // String literals, format!, computed messages → External
                            format!("return Err({}({}))", ctx.error_model.external_path(), a)
                        }
                    }
                }
                Expr::Call(c) if c.target == "Ok" && c.method.is_empty() => {
                    let a = c.args.iter().map(|e| expr_to_rust(e, ctx)).collect::<Vec<_>>().join(", ");
                    format!("return Ok({})", if a.is_empty() { "()".to_string() } else { a })
                }
                _ => {
                    let val = expr_to_rust_value(inner, ctx);
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
                        if val == "None" {
                            if returns_option {
                                "return None".to_string()
                            } else {
                                // Non-Option API with null: should be a check-time diagnostic
                                "compile_error!(\"null return on non-Option function\")"
                                    .to_string()
                            }
                        } else if returns_option && !val.starts_with("Some(") {
                            format!("return Some({})", val)
                        } else {
                            format!("return {}", val)
                        }
                    } else if val == "None" || val == "()" {
                        if returns_option {
                            "return Ok(None)".to_string()
                        } else if val == "()" {
                            "return Ok(())".to_string()
                        } else {
                            format!("return Err({})", ctx.error_model.not_found_path())
                        }
                    } else if returns_option && !val.starts_with("Some(") {
                        if let Expr::Ident(name) = inner.as_ref()
                            && ctx.local_type(name).map(is_option_type).unwrap_or(false) {
                                return RustExpr::Statement {
                                    text: format!("return Ok({})", val),
                                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                                };
                            }
                        format!("return Ok(Some({}))", val)
                    } else {
                        format!("return Ok({})", val)
                    }
                }
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: break / continue ───────────────────────────────────
        Expr::Break => RustExpr::Ident {
            name: "break".to_string(),
            ty: Some(RustType::Unit),
        },
        Expr::Continue => RustExpr::Ident {
            name: "continue".to_string(),
            ty: Some(RustType::Unit),
        },

        // ── Migrated: array literal ──────────────────────────────────────
        Expr::ArrayLit(items) => {
            let item_exprs: Vec<RustExpr> = items.iter().map(|e| {
                lower_to_rust(e, ctx)
            }).collect();
            RustExpr::Array {
                items: item_exprs,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: tuple ──────────────────────────────────────────────
        Expr::Tuple(items) => {
            let item_exprs: Vec<RustExpr> = items.iter().map(|e| {
                lower_to_rust(e, ctx)
            }).collect();
            RustExpr::Tuple {
                items: item_exprs,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: await ──────────────────────────────────────────────
        Expr::Await(inner) => {
            RustExpr::Await(Box::new(lower_to_rust(inner, ctx)))
        }

        // ── Migrated: try ────────────────────────────────────────────────
        Expr::Try(inner) => {
            RustExpr::Try(Box::new(lower_to_rust(inner, ctx)))
        }

        // ── Migrated: require ────────────────────────────────────────────
        Expr::Require(inner) => {
            let s = expr_to_rust(inner, ctx);
            let ty = infer_expr_type(inner, ctx);
            let text = if expr_is_json(inner, ctx)
                || ty.as_deref().is_some_and(is_json_type_name)
            {
                format!(
                    "{s}.as_str().map(|s| s.to_string()).ok_or({})?", ctx.error_model.not_found_path()
                )
            } else {
                let still_option = ty.as_deref().is_some_and(|t| peel_option_rust(t).is_some());
                if still_option {
                    format!("{s}.ok_or({})?", ctx.error_model.not_found_path())
                } else if s.trim_end().ends_with('?')
                    || ty.as_deref().is_some_and(|t| {
                        rust_ty_is_stringish(t) || t == "i64" || t == "bool" || t.starts_with("Vec<")
                    })
                {
                    // Already present (trailing ?) or a known-present scalar type
                    s
                } else {
                    format!("{s}.ok_or({})?", ctx.error_model.not_found_path())
                }
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: index ──────────────────────────────────────────────
        Expr::Index(base, idx) => {
            let b = expr_to_rust(base, ctx);
            let text = match idx.as_ref() {
                Expr::StringLit(s) => format!(
                    "{b}.get(\"{s}\").cloned().ok_or({})?", ctx.error_model.not_found_path()
                ),
                Expr::IntLit(n) => list_index_get_rust(&b, &n.to_string(), base, ctx),
                other => {
                    let i = expr_to_rust(other, ctx);
                    let base_ty = match base.as_ref() {
                        Expr::Ident(n) => ctx.local_type(n).unwrap_or(""),
                        _ => "",
                    };
                    let idx_is_int = matches!(other, Expr::IntLit(_))
                        || matches!(
                            other,
                            Expr::Ident(n) if matches!(
                                ctx.local_type(n),
                                Some("i64")
                                    | Some("i32")
                                    | Some("u64")
                                    | Some("u32")
                                    | Some("usize")
                                    | Some("isize")
                            )
                        );
                    if idx_is_int {
                        list_index_get_rust(&b, &format!("({i})"), base, ctx)
                    } else if base_ty.contains("Value") || base_ty == "Json" || base_ty.is_empty()
                    {
                        format!(
                            "{b}.get({i}.as_str()).cloned().unwrap_or(serde_json::Value::Null)"
                        )
                    } else {
                        format!("{b}[{i}]")
                    }
                }
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: action ─────────────────────────────────────────────
        Expr::Action(a) => RustExpr::Statement {
            text: translate_action(a, ctx),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },

        // ── Migrated: range ──────────────────────────────────────────────
        Expr::Range { start, end, inclusive } => {
            let s = start.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default();
            let e = end.as_ref().map(|e| expr_to_rust(e, ctx)).unwrap_or_default();
            let op = if *inclusive { "..=" } else { ".." };
            RustExpr::Statement {
                text: format!("{}{}{}", s, op, e),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: cast ───────────────────────────────────────────────
        Expr::Cast(inner_expr, ty) => {
            RustExpr::Statement {
                text: format!("{} as {}", expr_to_rust(inner_expr, ctx), ty),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: struct literal ─────────────────────────────────────
        Expr::StructLit(name, fields) => {
            let text = if name.is_empty() {
                // Anonymous record/map literal → JSON object value
                if fields.is_empty() {
                    "serde_json::json!({})".to_string()
                } else {
                    let pairs = fields.iter().map(|(k, v)| {
                        format!("\"{}\": {}", k, to_json_arg(v, ctx))
                    }).collect::<Vec<_>>().join(", ");
                    format!("serde_json::json!({{ {} }})", pairs)
                }
            } else {
                let fs = fields.iter().map(|(k, v)| {
                    let v_str = expr_to_rust(v, ctx);
                    let cloned = match v {
                        Expr::StringLit(s) => rust_string_lit_owned(s),
                        _ => {
                            if rust_already_owned(&v_str) || rust_is_copy_value(v, &v_str, ctx) {
                                v_str.clone()
                            } else {
                                match v {
                                    Expr::Ident(n) if should_clone_ident(n, ctx) => format!("{v_str}.clone()"),
                                    Expr::FieldAccess(_, _) => format!("{v_str}.clone()"),
                                    _ => v_str.clone(),
                                }
                            }
                        }
                    };
                    let coerced = if let Some(field_ty) = ctx.field_type(name, k) {
                        let val_ty = match v {
                            Expr::Ident(n) => ctx.local_type(n).map(|s| s.to_string()),
                            _ => infer_expr_type(v, ctx),
                        };
                        if val_ty.as_deref() == Some("serde_json::Value") {
                            match field_ty {
                                "String" => format!(
                                    "{}.as_str().map(|s| s.to_string()).unwrap_or_default()",
                                    cloned.trim_end_matches(".clone()")
                                ),
                                "bool" => format!("{}.as_bool().unwrap_or(false)", cloned.trim_end_matches(".clone()")),
                                "i64" => format!("{}.as_i64().unwrap_or(0)", cloned.trim_end_matches(".clone()")),
                                "f64" => format!("{}.as_f64().unwrap_or(0.0)", cloned.trim_end_matches(".clone()")),
                                t if is_option_type(t) => format!("Some({})", cloned),
                                _ => cloned,
                            }
                        } else if field_ty == "serde_json::Value" || field_ty == "Option<serde_json::Value>" {
                            if is_option_type(field_ty) {
                                if cloned == "None" {
                                    "None".to_string()
                                } else {
                                    format!("Some(serde_json::json!({}))", cloned)
                                }
                            } else {
                                format!("serde_json::json!({})", cloned)
                            }
                        } else {
                            cloned
                        }
                    } else {
                        cloned
                    };
                    let field = to_snake(k);
                    if coerced == field || coerced == *k {
                        coerced
                    } else {
                        format!("{field}: {coerced}")
                    }
                }).collect::<Vec<_>>().join(", ");
                format!("{} {{ {} }}", name, fs)
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: struct update ──────────────────────────────────────
        Expr::StructUpdate { name, fields, base } => {
            let fs = fields.iter().map(|(k, v)| format!("{}: {}", k, expr_to_rust(v, ctx))).collect::<Vec<_>>().join(", ");
            RustExpr::Statement {
                text: format!("{} {{ {}, ..{} }}", name, fs, expr_to_rust(base, ctx)),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: if let ─────────────────────────────────────────────
        Expr::IfLet { pattern, expr: inner_expr, then_body, else_body } => {
            let e = expr_to_rust(inner_expr, ctx);
            let then_str = then_body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            let else_str = else_body.as_ref().map(|eb| { let s = eb.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n"); format!(" else {{\n{}\n}}", s) }).unwrap_or_default();
            RustExpr::Statement {
                text: format!("if let {} = {} {{\n{}\n}}{}", pattern, e, then_str, else_str),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: while let ──────────────────────────────────────────
        Expr::WhileLet { pattern, expr: inner_expr, body } => {
            let e = expr_to_rust(inner_expr, ctx);
            let body_str = body.iter().map(|e2| format!("    {};", expr_to_rust(e2, ctx))).collect::<Vec<_>>().join("\n");
            RustExpr::Statement {
                text: format!("while let {} = {} {{\n{}\n}}", pattern, e, body_str),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: do block ───────────────────────────────────────────
        Expr::DoBlock(body) => {
            let text = if body.is_empty() {
                "{}".to_string()
            } else {
                let mut block_ctx = ctx.clone_for_inference();
                let mut lines = Vec::new();
                for (i, e) in body.iter().enumerate() {
                    let rust = expr_to_rust(e, &block_ctx);
                    if let Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) = e
                        && !name.contains('.') {
                            block_ctx.locals.insert(name.clone());
                            if let Some(ty) = ty_ann {
                                block_ctx.types.local_types.insert(name.clone(), crate::rust::type_to_rust(ty));
                            } else if let Some(t) = infer_expr_type(rhs, &block_ctx) {
                                block_ctx.types.local_types.insert(name.clone(), t);
                            }
                        }
                    if i == body.len() - 1 {
                        lines.push(format!("    {}", rust));
                    } else {
                        lines.push(format!("    {};", rust));
                    }
                }
                format!("{{\n{}\n}}", lines.join("\n"))
            };
            RustExpr::Statement {
                text,
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            }
        }

        // ── Migrated: stock ──────────────────────────────────────────────
        Expr::Stock => RustExpr::Statement {
            text: "/* error: stock not expanded */ ()".to_string(),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        },
    }
}

/// Lower `Expr::Ident` to structured `RustExpr`.
///
/// Lower a body (Vec<Expr>) to a Vec<RustExpr> with context tracking.
/// Each expression is lowered with a context that includes locals declared
/// by previous expressions in the body. This replicates the tracking that
/// `emit_block_lines` did at the string level.
fn lower_block(body: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    use super::super::analysis::analyze_mut_locals;
    use super::super::inference::infer_expr_type;

    let mut body_ctx = ctx.clone_for_inference();
    body_ctx.option_value_wrap = false;
    body_ctx.ownership.mut_locals.extend(analyze_mut_locals(body));
    let mut result = Vec::new();
    for e in body {
        let node = lower_to_rust(e, &body_ctx);
        // Track new local declarations for subsequent expressions
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e {
            if !name.contains('.') {
                body_ctx.locals.insert(name.clone());
                if let Some(t) = infer_expr_type(rhs, &body_ctx) {
                    body_ctx.types.local_types.insert(name.clone(), t);
                }
            }
        }
        result.push(node);
    }
    result
}

/// Like lower_block but the last expression is rendered as a value
/// (for option_value_wrap contexts and match arm bodies).
fn lower_value_block(body: &[Expr], ctx: &GenCtx) -> Vec<RustExpr> {
    use super::super::analysis::analyze_mut_locals;
    use super::super::inference::infer_expr_type;

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
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e {
            if !name.contains('.') {
                body_ctx.locals.insert(name.clone());
                if let Some(t) = infer_expr_type(rhs, &body_ctx) {
                    body_ctx.types.local_types.insert(name.clone(), t);
                }
            }
        }
        result.push(node);
    }
    result
}

/// Like lower_block but takes a pre-configured body context.
/// Used by ForLoop/WhileLoop which set up custom contexts (element types,
/// ref_elem_locals, etc.) before lowering the body.
fn lower_block_with_ctx(body: &[Expr], body_ctx: &GenCtx) -> Vec<RustExpr> {
    use super::super::inference::infer_expr_type;

    let mut ctx = body_ctx.clone_for_inference();
    let mut result = Vec::new();
    for e in body {
        let node = lower_to_rust(e, &ctx);
        if let Expr::Assign(name, rhs, _) | Expr::MutAssign(name, rhs, _) = e {
            if !name.contains('.') {
                ctx.locals.insert(name.clone());
                if let Some(t) = infer_expr_type(rhs, &ctx) {
                    ctx.types.local_types.insert(name.clone(), t);
                }
            }
        }
        result.push(node);
    }
    result
}

/// Handles: null→None, noop→{}, state locals, self field resolution,
/// enum variant qualification. Edge cases (inline ternary fstrings,
/// unwrap_or rewrites) fall through to Raw since they're not proper idents.
fn lower_ident(name: &str, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    // VEIL null → Rust None
    if name == "null" {
        return RustExpr::Ident {
            name: "None".to_string(),
            ty: None,
        };
    }
    // VEIL noop → Rust empty block (no-op)
    if name == "noop" {
        return RustExpr::Statement {
            text: "{}".to_string(),
            ty: Some(RustType::Unit),
        };
    }
    // Edge case: inline ternary with nested f-strings from parse_fstring_parts.
    // Not a proper ident — handled directly here.
    if name.contains(" then ") && (name.contains("f\"") || name.contains("f'")) {
        return RustExpr::Statement {
            text: super::super::translate::translate_inline_ternary_fstring(name),
            ty: None,
        };
    }
    // Edge case: unwrap_or rewrite from fstring parsing.
    if name.contains(".unwrap_or(\"") && name.ends_with("\")") {
        // Transform: x.unwrap_or("text") → x.unwrap_or("text".to_string())
        let converted = name.replacen("\")", "\".to_string())", 1);
        return RustExpr::Statement {
            text: converted,
            ty: None,
        };
    }
    // Threaded step state: read from the shared JSON bag.
    if ctx.state_locals.contains(name) {
        return RustExpr::Index {
            base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
            index: Box::new(RustExpr::StringLit(name.to_string())),
            ty: Some(RustType::Json),
        };
    }
    // Inside a method body: resolve self fields and enum variants.
    if ctx.in_method && !ctx.locals.contains(name) {
        if let Some(rf) = resolve_self_field_name(ctx, name) {
            if ctx.ownership.borrow_fields.contains(rf.as_str()) {
                return RustExpr::Borrow {
                    inner: Box::new(RustExpr::FieldAccess {
                        base: Box::new(RustExpr::Ident { name: "self".to_string(), ty: None }),
                        field: rf,
                        ty: None,
                    }),
                    mutable: false,
                };
            }
            // self.field.clone() — produces a clone of the field access
            return RustExpr::Clone(Box::new(RustExpr::FieldAccess {
                base: Box::new(RustExpr::Ident {
                    name: "self".to_string(),
                    ty: None,
                }),
                field: rf,
                ty: None,
            }));
        }
        // Uppercase non-local: try enum variant qualification
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            if let Some(enum_ty) = ctx.enum_variants.get(name) {
                return RustExpr::Ident {
                    name: format!("{enum_ty}::{name}"),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
            return RustExpr::Ident {
                name: name.to_string(),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
        }
        return RustExpr::Ident {
            name: name.to_string(),
            ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
        };
    }
    // Not in a method, or name is a local:
    // Uppercase non-local: try enum variant qualification
    if !ctx.locals.contains(name)
        && name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        && let Some(enum_ty) = ctx.enum_variants.get(name) {
            return RustExpr::Ident {
                name: format!("{enum_ty}::{name}"),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
        }
    // Plain identifier
    RustExpr::Ident {
        name: name.to_string(),
        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
    }
}

/// Lower `Expr::FieldAccess` to structured `RustExpr`.
///
/// Handles: is_some/is_none predicates, state locals, self.field, enum variants,
/// stub enum variants, JSON indexing, Option auto-unwrap, and copy/clone analysis.
fn lower_field_access(base: &Expr, field: &str, expr: &Expr, ctx: &GenCtx) -> RustExpr {
    // `opt.is_some` / `opt.is_none` (no call) → method call syntax
    if field == "is_some" || field == "is_none" {
        let base_lowered = lower_to_rust(base, ctx);
        return RustExpr::MethodCall {
            receiver: Box::new(base_lowered),
            method: field.to_string(),
            args: vec![],
            ty: Some(RustType::Named("bool".to_string())),
            is_async: false,
            is_fallible: false,
        };
    }

    // State local: index into the threaded JSON state.
    if let Expr::Ident(name) = base
        && ctx.state_locals.contains(name.as_str()) {
            return RustExpr::Index {
                base: Box::new(RustExpr::Index {
                    base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
                    index: Box::new(RustExpr::StringLit(name.clone())),
                    ty: Some(RustType::Json),
                }),
                index: Box::new(RustExpr::StringLit(field.to_string())),
                ty: Some(RustType::Json),
            };
        }

    // Method body: self.field handling
    if let Expr::Ident(name) = base
        && name == "self" && ctx.in_method {
            let f = resolve_self_field_name(ctx, field).unwrap_or_else(|| to_snake(field));
            if ctx.ownership.borrow_fields.contains(f.as_str()) {
                return RustExpr::Borrow {
                    inner: Box::new(RustExpr::FieldAccess {
                        base: Box::new(RustExpr::Ident { name: "self".to_string(), ty: None }),
                        field: f,
                        ty: None,
                    }),
                    mutable: false,
                };
            }
            if ctx.self_fields.contains(field)
                || ctx.self_fields.contains(&f)
                || ctx.self_field_types.contains_key(&f)
            {
                return RustExpr::Clone(Box::new(RustExpr::FieldAccess {
                    base: Box::new(RustExpr::Ident {
                        name: "self".to_string(),
                        ty: None,
                    }),
                    field: f,
                    ty: None,
                }));
            }
            return RustExpr::FieldAccess {
                base: Box::new(RustExpr::Ident {
                    name: "self".to_string(),
                    ty: None,
                }),
                field: f,
                ty: None,
            };
        }

    // Enum variant access: EnumName.Variant → EnumName::Variant
    if let Expr::Ident(name) = base {
        let field_is_variant = field.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);

        if matches!(ctx.name_to_shape.get(name.as_str()), Some(Shape::Enum)) {
            let variant = if field_is_variant {
                field.to_string()
            } else {
                field.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
                    + &field[1..]
            };
            return RustExpr::Ident {
                name: format!("{}::{}", name, variant),
                ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
            };
        }

        // Stub enums: PascalCase field access means a unit variant
        if field_is_variant
            && let Some((crate_name, path_type)) = ctx.stubs.stub_type_crate.get(name.as_str()) {
                return RustExpr::Ident {
                    name: format!("{}::{}::{}", crate_name, path_type, field),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
        // Lowercase variant on a stub-known type (snake_case → PascalCase)
        if !field_is_variant
            && let Some((crate_name, path_type)) = ctx.stubs.stub_type_crate.get(name.as_str()) {
                let variant: String = field
                    .split('_')
                    .map(|seg| {
                        let mut chars = seg.chars();
                        match chars.next() {
                            Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                            None => String::new(),
                        }
                    })
                    .collect();
                return RustExpr::Ident {
                    name: format!("{}::{}::{}", crate_name, path_type, variant),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }
    }

    // JSON local: field access on a JSON-typed local → index syntax
    if let Expr::Ident(name) = base
        && ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
            return RustExpr::Index {
                base: Box::new(RustExpr::Ident { name: name.clone(), ty: Some(RustType::Json) }),
                index: Box::new(RustExpr::StringLit(field.to_string())),
                ty: Some(RustType::Json),
            };
        }

    // Nested field access on a JSON value at any depth
    if is_json_rooted_expr(base, ctx) {
        let base_node = lower_to_rust(base, ctx);
        return RustExpr::Index {
            base: Box::new(base_node),
            index: Box::new(RustExpr::StringLit(field.to_string())),
            ty: Some(RustType::Json),
        };
    }

    // Option auto-unwrap: local has type Option<X> → unwrap on field access
    if let Expr::Ident(name) = base
        && let Some(ty) = ctx.local_type(name)
            && is_option_type(ty) {
                let base_str = expr_to_rust(base, ctx);
                let enclosing_returns_option = ctx.expected_return_rust.as_deref()
                    .map(is_option_type)
                    .unwrap_or(false);
                if enclosing_returns_option {
                    return RustExpr::Statement {
                        text: format!("{}.clone()?.{}", base_str, to_snake(field)),
                        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                    };
                }
                return RustExpr::Statement {
                    text: format!(
                        "{}.clone().ok_or({})?.{}",
                        base_str,
                        ctx.error_model.not_found_path(),
                        to_snake(field)
                    ),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                };
            }

    // Standard field access with copy/clone analysis
    let base_lowered = lower_to_rust(base, ctx);
    let field_snake = to_snake(field);
    let ty = infer_expr_type(expr, ctx).map(|s| RustType::parse(&s));

    let fa = RustExpr::FieldAccess {
        base: Box::new(base_lowered),
        field: field_snake,
        ty: ty.clone(),
    };

    // Check if the field access result needs cloning
    // Json / Value is indexed in place — do not clone
    if infer_expr_type(expr, ctx).as_deref().is_some_and(is_json_type_name) {
        return fa;
    }
    // Copy types don't need clone
    if field_access_is_copy(base, field, ctx) {
        return fa;
    }
    // Already-cloned base (emitted as self.field.clone() above) doesn't need double-clone
    // For other field accesses, VEIL values are reusable: clone non-Copy fields
    RustExpr::Clone(Box::new(fa))
}

// ─── lower_string_interp ──────────────────────────────────────────────────────

/// Lower `Expr::StringInterp` (f-strings) to `RustExpr::Format`.
///
/// Mirrors the logic in translate.rs: literal chars get `{`/`}` escaped for
/// `format!`, expression parts become `{}` holes with args.
/// When there are no expression parts, the result is just a string literal.
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
                // Args in format!() are by-reference (Display trait) — do NOT
                // apply ownership analysis here; cloning would be wasteful.
                args.push(lower_to_rust(e, ctx));
            }
        }
    }
    if args.is_empty() {
        // No interpolations — just a string literal that needs .to_string()
        let raw: String = parts
            .iter()
            .filter_map(|p| match p {
                StringPart::Literal(l) => Some(l.as_str()),
                _ => None,
            })
            .collect();
        // Emit as MethodCall on StringLit to produce the `"...".to_string()` form
        RustExpr::MethodCall {
            receiver: Box::new(RustExpr::StringLit(raw)),
            method: "to_string".to_string(),
            args: vec![],
            ty: Some(RustType::Named("String".to_string())),
            is_async: false,
            is_fallible: false,
        }
    } else {
        RustExpr::Format { template: fmt, args }
    }
}

// ─── Bus routing IR (structured envelope/message builders) ───────────────────

/// Convert a VEIL expression to a `RustExpr` for use in JSON argument positions
/// (inside `serde_json::json!({...})` envelopes and messages).
///
/// Mirrors the logic of `to_json_arg()` in translate.rs but produces structured
/// nodes instead of formatted strings.
fn to_json_arg_ir(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::Ident(name) => {
            // VEIL null → JSON null
            if name == "null" {
                return RustExpr::JsonNull;
            }
            // Shared step-state value → state["name"].clone()
            if ctx.state_locals.contains(name.as_str()) {
                return RustExpr::Clone(Box::new(RustExpr::Index {
                    base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
                    index: Box::new(RustExpr::StringLit(name.clone())),
                    ty: Some(RustType::Json),
                }));
            }
            // Struct-captured input (step impl) → self.<field>.clone()
            if ctx.in_method && ctx.self_fields.contains(name.as_str()) {
                return RustExpr::Clone(Box::new(RustExpr::FieldAccess {
                    base: Box::new(RustExpr::Ident { name: "self".to_string(), ty: None }),
                    field: to_snake(name),
                    ty: None,
                }));
            }
            // Local variable → name.clone()
            if ctx.is_local(name) {
                return RustExpr::Clone(Box::new(RustExpr::Ident {
                    name: name.clone(),
                    ty: None,
                }));
            }
            // Non-local bare ident → symbolic string (enum variant, marker)
            RustExpr::StringLit(name.clone())
        }
        Expr::FieldAccess(base, field) => {
            // Field of a state-local → state["name"]["field"].clone()
            if let Expr::Ident(name) = base.as_ref() {
                if ctx.state_locals.contains(name.as_str()) {
                    return RustExpr::Clone(Box::new(RustExpr::Index {
                        base: Box::new(RustExpr::Index {
                            base: Box::new(RustExpr::Ident { name: "state".to_string(), ty: None }),
                            index: Box::new(RustExpr::StringLit(name.clone())),
                            ty: Some(RustType::Json),
                        }),
                        index: Box::new(RustExpr::StringLit(field.clone())),
                        ty: Some(RustType::Json),
                    }));
                }
                // serde_json::Value local → name["field"].clone()
                if ctx.is_local(name) && ctx.local_type(name) == Some("serde_json::Value") {
                    return RustExpr::Clone(Box::new(RustExpr::Index {
                        base: Box::new(RustExpr::Ident { name: name.clone(), ty: None }),
                        index: Box::new(RustExpr::StringLit(field.clone())),
                        ty: Some(RustType::Json),
                    }));
                }
            }
            // Otherwise serialize base then index
            let base_ir = to_json_arg_ir(base, ctx);
            RustExpr::Statement {
                text: format!("serde_json::json!({})[\"{}\"].clone()", emit(&base_ir), field),
                ty: Some(RustType::Json),
            }
        }
        Expr::ArrayLit(items) if items.is_empty() => RustExpr::JsonEmptyArray,
        Expr::ArrayLit(items) => {
            let vals: Vec<RustExpr> = items.iter().map(|e| to_json_arg_ir(e, ctx)).collect();
            RustExpr::VecMacro(vals)
        }
        _ => {
            // Fall back to recursive lowering
            lower_to_rust(expr, ctx)
        }
    }
}

/// Build a structured `RustExpr::JsonMacro` for a named message payload
/// (desugared `invoke MessageType { field: val, ... }`).
///
/// Emits: `serde_json::json!({ "type": "Name", "field1": val1, ... })`
fn json_message_ir(name: &str, fields: &[(String, Expr)], ctx: &GenCtx) -> RustExpr {
    let mut entries: Vec<(String, RustExpr)> = Vec::with_capacity(fields.len() + 1);
    entries.push(("type".to_string(), RustExpr::StringLit(name.to_string())));
    for (k, v) in fields {
        entries.push((k.clone(), to_json_arg_ir(v, ctx)));
    }
    RustExpr::JsonMacro { entries }
}

/// Build a structured `RustExpr::JsonMacro` for a cross-boundary JSON envelope.
///
/// Emits: `serde_json::json!({ "target": "T", "method": "m", "args": [...] })`
fn json_envelope_ir(target: &str, method: &str, args: &[Expr], ctx: &GenCtx) -> RustExpr {
    let arg_vals: Vec<RustExpr> = args.iter().map(|a| to_json_arg_ir(a, ctx)).collect();
    let entries = vec![
        ("target".to_string(), RustExpr::StringLit(target.to_string())),
        ("method".to_string(), RustExpr::StringLit(method.to_string())),
        ("args".to_string(), RustExpr::VecMacro(arg_vals)),
    ];
    RustExpr::JsonMacro { entries }
}

/// Attempt to lower a bus routing call to structured `RustExpr`.
///
/// Handles three paths:
/// 1. Routing trait calls (`ctx.routing.routing_traits`) with json_message/envelope args
/// 2. Typed bus decode (invoke/request with known return type → from_value)
/// 3. Envelope routing (cross-boundary calls via `routing_ref.invoke(envelope)`)
///
/// Returns `Some(RustExpr)` if the call was handled, `None` to fall through.
fn lower_call_bus_routing(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    use super::super::inference::{bus_message_name_from_args, bus_return_type_in_scope};

    // ── Path 1 & 2: Trait-shaped target with routing_traits ──────────────────
    if ctx.is_trait_target(&call.target) && ctx.routing.routing_traits.contains(&call.target) {
        let dep_name = ctx.deps_field_for(&call.target);
        let method = if call.method.is_empty() { "call" } else { &call.method };

        // Build the JSON payload (sugar path or direct call)
        let payload_ir = if call.sugar.is_some() {
            match call.args.first() {
                Some(Expr::StructLit(name, fields)) => json_message_ir(name, fields, ctx),
                Some(Expr::Ident(evt)) => {
                    // Simple event identifier → json!({ "type": "EventName" })
                    let entries = vec![
                        ("type".to_string(), RustExpr::StringLit(evt.clone())),
                    ];
                    RustExpr::JsonMacro { entries }
                }
                _ => json_envelope_ir(&call.target, method, &call.args, ctx),
            }
        } else {
            // Direct routing-trait call: these use clone_args style, not JSON envelope.
            // This sub-path does NOT produce a JSON envelope — it passes args directly.
            // Fall through to the old path for now; only sugar-based routing uses envelopes.
            return None;
        };

        // Build the receiver reference
        let rref = if ctx.routing.routing_ref.is_empty() {
            format!("deps.{}", dep_name)
        } else {
            ctx.routing.routing_ref.clone()
        };
        let bare = to_snake(method);

        // The base call: routing_ref.method(payload).await?
        let base_call = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident { name: rref, ty: None }),
            method: bare.clone(),
            args: vec![payload_ir],
            ty: Some(RustType::Json),
            is_async: true,
            is_fallible: true,
        };

        // Path 2: Typed bus decode for invoke/request with known return type
        if matches!(bare.as_str(), "invoke" | "request") {
            let decode = bus_message_name_from_args(&call.args)
                .and_then(|msg| ctx.routing.bus_returns.get(&msg).map(|ret| (msg, ret.clone())));
            if let Some((_msg, ref ret)) = decode.filter(|(_, r)| bus_return_type_in_scope(ctx, r)) {
                // serde_json::from_value::<RetType>(call_expr).map_err(|e| Error::External(...))?
                let from_value = RustExpr::FnCall {
                    path: format!("serde_json::from_value::<{}>", ret),
                    args: vec![base_call],
                    ty: Some(RustType::parse(ret)),
                };
                return Some(RustExpr::MapErr {
                    inner: Box::new(from_value),
                    variant: ctx.error_model.external_path(),
                });
            }
        }

        return Some(base_call);
    }

    // ── Path 3: Envelope routing (cross-boundary calls) ──────────────────────
    let is_lang_target = matches!(
        call.target.as_str(),
        "Dt" | "DateTime" | "Uuid" | "Map" | "List" | "Opt" | "Json" | "Env" | "Str" | "Id" | "Int" | "UUID"
    );
    let is_typed_local = ctx.is_local(&call.target) && ctx.local_type(&call.target).is_some();
    if ctx.routing.envelope_routing
        && !is_lang_target
        && !is_typed_local
        && !ctx.stubs.stub_pkg_crate.contains_key(&call.target)
        && (ctx.is_struct_target(&call.target) || ctx.is_local(&call.target) || !call.method.is_empty())
    {
        let method = if call.method.is_empty() { "new" } else { &call.method };
        let rref = if ctx.routing.routing_ref.is_empty() {
            "deps".to_string()
        } else {
            ctx.routing.routing_ref.clone()
        };
        let envelope = json_envelope_ir(&call.target, method, &call.args, ctx);
        let invoke_call = RustExpr::MethodCall {
            receiver: Box::new(RustExpr::Ident { name: rref, ty: None }),
            method: "invoke".to_string(),
            args: vec![envelope],
            ty: Some(RustType::Json),
            is_async: true,
            is_fallible: true,
        };
        return Some(invoke_call);
    }

    None
}

// ─── Builder chain lowering ──────────────────────────────────────────────────

/// Methods that require custom pre-processing in translate_call and cannot be
/// structurally lowered as simple MethodCall nodes in a builder chain.
const SPECIAL_METHODS: &[&str] = &[
    "get", "as_str", "as_s", "as_n", "trim", "unwrap", "unwrap_or",
    "unwrap_or_else", "body", "limit", "parse_int", "parse_json", "first",
];

/// Check if a method (after stripping `!`/`?`) is in the special-case list.
fn is_special_method(method: &str) -> bool {
    let bare = method.trim_end_matches(['!', '?']);
    SPECIAL_METHODS.contains(&bare)
}

/// Attempt to lower a receiver-based method call (builder chain) to structured
/// `RustExpr::MethodCall` nodes.
///
/// Intercepts calls where:
/// - The call has a receiver (it's `receiver.method(args)`)
/// - The receiver is itself a Call (chained builder pattern)
/// - The terminal method is not in the special-case list
///
/// Each method in the chain becomes a nested `MethodCall`. The terminal method
/// gets its async/fallible flags from `receiver_call_suffix`. If the suffix
/// includes a `.map_err(...)`, the result is wrapped in `RustExpr::MapErr`.
///
/// Returns `Some(RustExpr)` if handled, `None` to fall through.
fn lower_call_builder_chain(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    use super::super::calls::{receiver_call_suffix, clone_args_for_typed_method, rust_method_name};

    // Only handle calls with a receiver that is itself a Call (chained)
    let recv = call.receiver.as_ref()?;
    if !matches!(recv.as_ref(), Expr::Call(_)) {
        return None;
    }

    // Skip special methods that need custom handling in translate_call
    if is_special_method(&call.method) {
        return None;
    }

    // Get the suffix to determine async/fallible for the terminal method
    let suffix = receiver_call_suffix(recv, &call.method, ctx);

    // Only intercept when the terminal method has a meaningful suffix
    // (async, fallible, or both). Plain builder intermediates with no suffix
    // could still be part of a chain that ends with .send() — but those are
    // handled when the send() call is the outer Call and this intermediate
    // is the receiver.
    if suffix.is_empty() {
        return None;
    }

    // Parse suffix into structural flags
    let (is_async, is_fallible, needs_map_err, owns_str) = if suffix.contains(".await") && suffix.contains("map_err") {
        (true, true, true, false)
    } else if suffix == ".await?" {
        (true, true, false, false)
    } else if suffix == ".await" {
        (true, false, false, false)
    } else if suffix.contains("map(|s| s.to_string())") {
        (false, true, true, true)
    } else if suffix.contains("map_err") {
        (false, true, true, false)
    } else if suffix.ends_with('?') {
        (false, true, false, false)
    } else {
        (false, false, false, false)
    };

    // Build the receiver chain recursively
    let receiver_ir = lower_chain_receiver(recv, ctx);

    // Build args (rendered as Raw strings matching clone_args_for_typed_method)
    let method_name = rust_method_name(&call.method);
    let recv_lookup: Option<&str> = match recv.as_ref() {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::FieldAccess(_, field) => Some(field.as_str()),
        _ => None,
    };
    let args_str = clone_args_for_typed_method(recv_lookup, &call.method, &call.args, ctx);
    let args_ir = if args_str.is_empty() {
        vec![]
    } else {
        vec![RustExpr::Statement { text: args_str, ty: None }]
    };

    // Build the terminal method call
    let method_call = RustExpr::MethodCall {
        receiver: Box::new(receiver_ir),
        method: method_name,
        args: args_ir,
        ty: infer_call_type_from_ctx(call, ctx),
        is_async,
        is_fallible: is_fallible && !needs_map_err,
    };

    // Wrap in MapErr if the suffix demands it
    if needs_map_err {
        let variant = if owns_str {
            // map(|s| s.to_string()).map_err(...) — too complex for structured.
            // Fall through to Raw for this edge case.
            return None;
        } else {
            ctx.error_model.external_path()
        };
        return Some(RustExpr::MapErr {
            inner: Box::new(method_call),
            variant,
        });
    }

    Some(method_call)
}

/// Recursively lower a chain receiver (Call → nested MethodCalls).
/// Non-Call receivers (Ident, FieldAccess) are lowered via lower_to_rust.
fn lower_chain_receiver(expr: &Expr, ctx: &GenCtx) -> RustExpr {
    match expr {
        Expr::Call(inner_call) => {
            // Check if this inner call also has a receiver (deeper chain)
            if let Some(inner_recv) = &inner_call.receiver {
                // Skip special methods — fall back to Raw for the whole sub-chain
                if is_special_method(&inner_call.method) {
                    return RustExpr::Statement {
                        text: expr_to_rust(expr, ctx),
                        ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                    };
                }
                let receiver_ir = lower_chain_receiver(inner_recv, ctx);
                let method_name = super::super::calls::rust_method_name(&inner_call.method);
                let recv_lookup: Option<&str> = match inner_recv.as_ref() {
                    Expr::Ident(name) => Some(name.as_str()),
                    Expr::FieldAccess(_, field) => Some(field.as_str()),
                    _ => None,
                };
                let args_str = super::super::calls::clone_args_for_typed_method(
                    recv_lookup, &inner_call.method, &inner_call.args, ctx,
                );
                let args_ir = if args_str.is_empty() {
                    vec![]
                } else {
                    vec![RustExpr::Statement { text: args_str, ty: None }]
                };
                // Intermediate chain methods have no async/fallible suffix
                RustExpr::MethodCall {
                    receiver: Box::new(receiver_ir),
                    method: method_name,
                    args: args_ir,
                    ty: None,
                    is_async: false,
                    is_fallible: false,
                }
            } else {
                // Call without a receiver (e.g. `client.query()` where client is the target)
                // This is the chain root — render it as Raw since it may need
                // target-based resolution (struct constructor, free fn, etc.)
                RustExpr::Statement {
                    text: expr_to_rust(expr, ctx),
                    ty: infer_expr_type(expr, ctx).map(|s| RustType::parse(&s)),
                }
            }
        }
        // Non-Call receivers: use lower_to_rust for idents/field access
        _ => lower_to_rust(expr, ctx),
    }
}

/// Infer call type from context without rendering (avoids double translate_call).
fn infer_call_type_from_ctx(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustType> {
    let method_key = call.method.trim_end_matches(['!', '?']);
    if let Some(ret) = (!call.target.is_empty())
        .then(|| ctx.types.method_returns.get(&(call.target.clone(), method_key.to_string())))
        .flatten()
    {
        return Some(RustType::parse(ret));
    }
    if let Some(ret) = call.receiver.as_ref()
        .and_then(|recv| infer_expr_type(recv, ctx))
        .and_then(|recv_ty| ctx.types.method_returns.get(&(recv_ty, method_key.to_string())))
    {
        return Some(RustType::parse(ret));
    }
    None
}

// ─── lower_call ──────────────────────────────────────────────────────────────

/// Lower `Expr::Call` to structured `RustExpr`.
///
/// Strategy: handle the common patterns structurally, fall through to
/// `RustExpr::Statement` wrapping `translate_call` for complex sub-paths that
/// are not worth migrating now.
///
/// Migrated paths:
/// - Bus routing (envelope/message JSON calls via routing traits)
/// - Cross-boundary envelope routing
///
/// The key win: async/fallible suffixes become structural composition
/// (`RustExpr::Await`, `RustExpr::Try`) instead of string appending.
/// Lower non-routing port/trait calls to structural MethodCall nodes.
///
/// Handles calls of the form `PortName.method(args)` where `PortName` is a trait
/// target that is NOT a routing trait (routing is handled by lower_call_bus_routing).
/// These produce: `deps.<field>.method(args).await?` or `.await` depending on
/// the method's return type.
fn lower_call_trait_dep(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustExpr> {
    use super::super::calls::param_types_for;

    // Only handle trait-shaped targets that are NOT routing traits
    if !ctx.is_trait_target(&call.target) {
        return None;
    }
    if ctx.routing.routing_traits.contains(&call.target) {
        return None;
    }
    // Sugar calls go through bus routing
    if call.sugar.is_some() {
        return None;
    }

    let dep_name = ctx.deps_field_for(&call.target);
    let method = if call.method.is_empty() { "call" } else { &call.method };
    let method_key = method.trim_end_matches(['!', '?']);

    // Determine args (using the same logic as translate_call)
    let param_tys = param_types_for(Some(call.target.as_str()), method_key, ctx);
    let args_str = call.args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let expected = param_tys.get(i).map(|s| s.as_str());
            let s = super::super::calls::arg_to_rust(a, expected, ctx);
            match a {
                Expr::Ident(name) if ctx.local_type(name) == Some("serde_json::Value") => {
                    format!("{}.clone()", name)
                }
                Expr::Ident(name)
                    if ctx.local_type(name)
                        .map(|t| t.starts_with("Option<"))
                        .unwrap_or(false) =>
                {
                    let expects_opt = expected
                        .map(|t| t.starts_with("Option<") || t.starts_with("Opt<"))
                        .unwrap_or(false);
                    if expects_opt {
                        format!("{}.clone()", name)
                    } else {
                        format!("{}.clone().ok_or({})?", name, ctx.error_model.not_found_path())
                    }
                }
                _ => s,
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    // Determine fallibility
    let has_bang = method.ends_with('!');
    let ret_type = ctx.return_type_of(&call.target, method)
        .or_else(|| {
            ctx.dep_fields.iter()
                .find(|(_, v)| *v == &call.target)
                .and_then(|(trait_name, _)| ctx.return_type_of(trait_name, method))
        });
    let is_fallible = if has_bang {
        true
    } else {
        match ret_type {
            Some("bool") | Some("Bool") | Some("i64") | Some("f64")
            | Some("String") | Some("()") | Some("") => false,
            Some(t) if t.starts_with("Option<") || t.starts_with("Opt<") => false,
            _ => true,
        }
    };

    // Build receiver: deps.<field> or self.<field>
    let prefix = if ctx.in_method && ctx.self_fields.contains(&dep_name) {
        format!("self.{}", dep_name)
    } else {
        format!("deps.{}", dep_name)
    };

    let args_ir = if args_str.is_empty() {
        vec![]
    } else {
        vec![RustExpr::Statement { text: args_str, ty: None }]
    };

    let ty = infer_call_type(call, ctx);

    Some(RustExpr::MethodCall {
        receiver: Box::new(RustExpr::Ident { name: prefix, ty: None }),
        method: to_snake(method_key),
        args: args_ir,
        ty,
        is_async: true,
        is_fallible,
    })
}

pub(super) fn lower_call(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> RustExpr {
    // Try structured bus routing first
    if let Some(expr) = lower_call_bus_routing(call, ctx) {
        return expr;
    }

    // Try builder chain lowering (chained receiver method calls with async/fallible terminal)
    if let Some(expr) = lower_call_builder_chain(call, ctx) {
        return expr;
    }

    // Try structured port/trait calls (non-routing, non-sugar)
    if let Some(expr) = lower_call_trait_dep(call, ctx) {
        return expr;
    }

    // Fall through to Raw wrapping translate_call for everything else
    let text = super::super::calls::translate_call(call, ctx);
    let ty = infer_call_type(call, ctx);
    RustExpr::Statement { text, ty }
}

/// Infer the RustType for a call expression from context.
fn infer_call_type(call: &veil_ir::ast::CallExpr, ctx: &GenCtx) -> Option<RustType> {
    // Try the method_returns map first (most precise)
    let method_key = call.method.trim_end_matches(['!', '?']);
    if !call.target.is_empty()
        && let Some(ret) = ctx.types.method_returns.get(&(call.target.clone(), method_key.to_string())) {
            return Some(RustType::parse(ret));
        }
    // Check receiver type for chained calls
    if let Some(recv) = &call.receiver
        && let Some(recv_ty) = infer_expr_type(recv, ctx)
            && let Some(ret) = ctx.types.method_returns.get(&(recv_ty.clone(), method_key.to_string())) {
                return Some(RustType::parse(ret));
            }
    // Struct constructors return the struct type
    if (call.method.is_empty() || method_key == "new" || method_key == "default")
        && !call.target.is_empty() && call.target.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            return Some(RustType::Named(call.target.clone()));
        }
    // Fall back to general inference
    infer_expr_type(&Expr::Call(call.clone()), ctx).map(|s| RustType::parse(&s))
}

/// Lower `Expr::Closure` to a structured `RustExpr`, applying try-suppression
/// on body expressions so that `?` and `.map_err(...)` become `.unwrap()`.
fn lower_closure(params: &[String], body: &[Expr], ctx: &GenCtx) -> RustExpr {
    // Create a closure-scoped context with params as locals
    let mut closure_ctx = ctx.clone_for_inference();
    for param in params {
        closure_ctx.locals.insert(param.clone());
    }

    // Lower and suppress-try each body expression
    let body_exprs: Vec<RustExpr> = body
        .iter()
        .map(|e| {
            let lowered = lower_to_rust(e, &closure_ctx);
            suppress_try_in_closure(lowered)
        })
        .collect();

    let p = params.join(", ");
    if body_exprs.len() == 1 {
        let body_str = emit(&body_exprs[0]);
        RustExpr::Statement {
            text: format!("|{}| {}", p, body_str),
            ty: None,
        }
    } else {
        let stmts = body_exprs
            .iter()
            .map(|e| format!("    {};", emit(e)))
            .collect::<Vec<_>>()
            .join("\n");
        RustExpr::Statement {
            text: format!("|{}| {{\n{}\n}}", p, stmts),
            ty: None,
        }
    }
}

