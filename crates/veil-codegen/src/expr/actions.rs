use veil_ir::ast::*;
use veil_ir::layer::StmtShape;
use crate::rust::to_snake;
use super::*;
use super::rust_ir::{
    field, ident, lower_to_rust, lower_value, map_err_ignore, method, not_found_err,
    ret_err, strip_try_ir, validation_err, RustExpr,
};

/// Negate a guard condition logically to avoid clippy::nonminimal_bool
/// and clippy::comparison_to_empty.
fn negate_guard_condition(cond: &Expr, ctx: &GenCtx) -> RustExpr {
    if let Expr::BinaryOp(bin) = cond {
        let rhs_is_empty_str = matches!(bin.right.as_ref(), Expr::StringLit(s) if s.is_empty());
        if rhs_is_empty_str {
            let left = lower_to_rust(&bin.left, ctx);
            return match bin.op {
                BinOp::NotEq => method(left, "is_empty", vec![]),
                BinOp::Eq => RustExpr::UnaryOp {
                    op: "!".to_string(),
                    expr: Box::new(method(left, "is_empty", vec![])),
                    ty: Some(super::rust_ir::RustType::Named("bool".to_string())),
                },
                _ => RustExpr::UnaryOp {
                    op: "!".to_string(),
                    expr: Box::new(lower_to_rust(cond, ctx)),
                    ty: Some(super::rust_ir::RustType::Named("bool".to_string())),
                },
            };
        }
        let negated_op = match bin.op {
            BinOp::Eq => Some("!="),
            BinOp::NotEq => Some("=="),
            BinOp::Gt => Some("<="),
            BinOp::GtEq => Some("<"),
            BinOp::Lt => Some(">="),
            BinOp::LtEq => Some(">"),
            _ => None,
        };
        if let Some(op) = negated_op {
            return RustExpr::BinOp {
                left: Box::new(lower_to_rust(&bin.left, ctx)),
                op: op.to_string(),
                right: Box::new(lower_to_rust(&bin.right, ctx)),
                ty: Some(super::rust_ir::RustType::Named("bool".to_string())),
            };
        }
    }
    RustExpr::UnaryOp {
        op: "!".to_string(),
        expr: Box::new(lower_to_rust(cond, ctx)),
        ty: Some(super::rust_ir::RustType::Named("bool".to_string())),
    }
}

/// Classify a `guard` failure message → DomainError variant.
/// Real input validation stays Validation (400).
pub fn guard_error_variant(msg: &str) -> &'static str {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("not found")
        || lower.contains("access denied")
        || lower.contains("forbidden")
        || lower.contains("unauthorized")
        || (lower.contains("denied") && !lower.contains("validation"))
    {
        "NotFound"
    } else {
        "Validation"
    }
}

/// Interpolate a statement `lowers_to` template for the given target.
///
/// Variables: `{args}`, `{argN}`, `{dep}`, `{self}`, `{named.key}`, `{body}`.
pub fn interpolate_action_template(
    template: &str,
    a: &ActionExpr,
    ctx: &GenCtx,
    translate_expr: &dyn Fn(&Expr, &GenCtx) -> String,
) -> String {
    let mut result = template.to_string();

    let args_str = if !a.named_args.is_empty() {
        // Prefer a single struct-like arg when named fields were used as payload.
        let fields = a
            .named_args
            .iter()
            .map(|(k, v)| format!("{}: {}", k, translate_expr(v, ctx)))
            .collect::<Vec<_>>()
            .join(", ");
        if a.target.is_empty() {
            format!("{{ {} }}", fields)
        } else {
            format!("{} {{ {} }}", a.target, fields)
        }
    } else if !a.args.is_empty() {
        a.args
            .iter()
            .map(|e| translate_expr(e, ctx))
            .collect::<Vec<_>>()
            .join(", ")
    } else if !a.target.is_empty() {
        a.target.clone()
    } else {
        String::new()
    };
    result = result.replace("{args}", &args_str);

    for (i, arg) in a.args.iter().enumerate() {
        let rendered = translate_expr(arg, ctx);
        result = result.replace(&format!("{{arg{i}}}"), &rendered);
    }
    // Also expose named-args as arg indices after positionals.
    for (i, (_k, v)) in a.named_args.iter().enumerate() {
        let idx = a.args.len() + i;
        let rendered = translate_expr(v, ctx);
        result = result.replace(&format!("{{arg{idx}}}"), &rendered);
    }

    if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
        if let Some(dep_type) = &spec.requires_dep {
            let dep_field = ctx.deps_field_for(dep_type);
            result = result.replace("{dep}", &dep_field);
        } else if let Some(port) = &spec.port_target {
            let dep_field = ctx.deps_field_for(port);
            result = result.replace("{dep}", &dep_field);
        }
    }
    // Bare `{dep}` left unresolved → snake of keyword (last resort).
    if result.contains("{dep}") {
        result = result.replace("{dep}", &to_snake(&a.keyword));
    }

    result = result.replace("{self}", "self");

    for (key, val) in &a.named_args {
        let rendered = translate_expr(val, ctx);
        result = result.replace(&format!("{{named.{key}}}"), &rendered);
    }

    if result.contains("{body}") {
        let body_str = a
            .body
            .iter()
            .map(|e| translate_expr(e, ctx))
            .collect::<Vec<_>>()
            .join("; ");
        result = result.replace("{body}", &body_str);
    }

    // Condition/message helpers for If-shaped statements with templates.
    if let Some(cond) = a.condition.as_deref() {
        result = result.replace("{condition}", &translate_expr(cond, ctx));
    }
    if let Some(msg) = &a.message {
        let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
        result = result.replace("{message}", &format!("\"{escaped}\""));
    }

    if let Some(binding) = &a.result_binding {
        format!("let {binding} = {result}")
    } else {
        result
    }
}

fn interpolate_action_ir(template: &str, a: &ActionExpr, ctx: &GenCtx) -> RustExpr {
    let mut bindings: Vec<(String, RustExpr)> = Vec::new();
    let args_node = if !a.named_args.is_empty() {
        RustExpr::StructLit {
            name: a.target.clone(),
            fields: a
                .named_args
                .iter()
                .map(|(k, v)| (k.clone(), lower_value(v, ctx)))
                .collect(),
            rest: None,
            ty: None,
        }
    } else if !a.args.is_empty() {
        RustExpr::Join {
            items: a.args.iter().map(|e| lower_value(e, ctx)).collect(),
            sep: ", ".to_string(),
        }
    } else if !a.target.is_empty() {
        ident(a.target.clone())
    } else {
        ident("")
    };
    bindings.push(("args".to_string(), args_node));
    for (i, arg) in a.args.iter().enumerate() {
        bindings.push((format!("arg{i}"), lower_value(arg, ctx)));
    }
    for (i, (_k, v)) in a.named_args.iter().enumerate() {
        let idx = a.args.len() + i;
        bindings.push((format!("arg{idx}"), lower_value(v, ctx)));
    }
    if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
        if let Some(dep_type) = &spec.requires_dep {
            bindings.push(("dep".to_string(), ident(ctx.deps_field_for(dep_type))));
        } else if let Some(port) = &spec.port_target {
            bindings.push(("dep".to_string(), ident(ctx.deps_field_for(port))));
        }
    }
    if !bindings.iter().any(|(k, _)| k == "dep") {
        bindings.push(("dep".to_string(), ident(to_snake(&a.keyword))));
    }
    // {target} — the action's target name (e.g., "CustomerCreated" for dispatch CustomerCreated{...})
    if !a.target.is_empty() {
        bindings.push(("target".to_string(), RustExpr::StringLit(a.target.clone())));
    }
    bindings.push(("self".to_string(), ident("self")));
    for (key, val) in &a.named_args {
        bindings.push((format!("named.{key}"), lower_value(val, ctx)));
    }
    if template.contains("{body}") {
        bindings.push((
            "body".to_string(),
            RustExpr::Join {
                items: a.body.iter().map(|e| lower_value(e, ctx)).collect(),
                sep: "; ".to_string(),
            },
        ));
    }
    if let Some(cond) = a.condition.as_deref() {
        bindings.push(("condition".to_string(), lower_value(cond, ctx)));
    }
    if let Some(msg) = &a.message {
        bindings.push(("message".to_string(), RustExpr::StringLit(msg.clone())));
    }
    let templ = RustExpr::LayerTemplate {
        template: template.to_string(),
        bindings,
    };
    if let Some(binding) = &a.result_binding {
        RustExpr::Let {
            name: binding.clone(),
            mutable: false,
            ty: None,
            value: Box::new(templ),
        }
    } else {
        templ
    }
}

/// Translate a layer-defined Action that was NOT desugared (e.g. emit, guard).
pub fn translate_action(a: &ActionExpr, ctx: &GenCtx) -> RustExpr {
    if let Some(spec) = ctx.statement_specs.get(&a.keyword) {
        if let Some(template) = spec.lowers_to.get("rust") {
            return interpolate_action_ir(template, a, ctx);
        }
        if let (Some(port), Some(meth)) = (&spec.port_target, &spec.port_method) {
            let dep = ctx.deps_field_for(port);
            let rref = if ctx.in_method {
                field(ident("self"), dep)
            } else {
                field(ident("deps"), dep)
            };
            let args = if !a.named_args.is_empty() {
                vec![RustExpr::StructLit {
                    name: a.target.clone(),
                    fields: a
                        .named_args
                        .iter()
                        .map(|(k, v)| (k.clone(), lower_value(v, ctx)))
                        .collect(),
                    rest: None,
                    ty: None,
                }]
            } else if !a.args.is_empty() {
                a.args.iter().map(|e| lower_value(e, ctx)).collect()
            } else if !a.target.is_empty() {
                vec![ident(a.target.clone())]
            } else {
                vec![]
            };
            let call = super::rust_ir::apply_finish(
                method(rref, to_snake(meth), args),
                super::rust_ir::CallFinish::AwaitTry,
                &ctx.error_model,
            );
            return if let Some(binding) = &a.result_binding {
                RustExpr::Let {
                    name: binding.clone(),
                    mutable: false,
                    ty: None,
                    value: Box::new(call),
                }
            } else {
                call
            };
        }
    }

    match a.shape {
        StmtShape::If => {
            let msg = a.message.as_deref().unwrap_or("precondition failed");
            let err_var = guard_error_variant(msg);
            let err_node = if err_var == "NotFound" {
                not_found_err(ctx)
            } else {
                validation_err(msg, ctx)
            };
            match a.condition.as_deref() {
                Some(cond @ Expr::Call(c))
                    if !c.method.is_empty()
                        && (ctx.name_to_shape.contains_key(&c.target)
                            || ctx.is_stub_method_fallible_global(&c.method)
                            || c.method == "validate") =>
                {
                    map_err_ignore(strip_try_ir(lower_to_rust(cond, ctx)), err_node)
                }
                Some(cond @ Expr::Await(_)) => {
                    map_err_ignore(strip_try_ir(lower_to_rust(cond, ctx)), err_node)
                }
                Some(cond) => {
                    if let Expr::Call(c) = cond
                        && c.method == "is_some"
                        && ctx.locals.contains(&c.target)
                    {
                        let var_type = ctx.types.local_types.get(&c.target);
                        let is_option = var_type
                            .map(|t| t.starts_with("Option<") || t == "Option")
                            .unwrap_or(true);
                        if !is_option {
                            return RustExpr::Comment(format!(
                                "guard {:?} — local is not Option (already forced present)",
                                msg
                            ));
                        }
                        return RustExpr::If {
                            condition: Box::new(method(ident(c.target.clone()), "is_none", vec![])),
                            then_body: vec![ret_err(err_node)],
                            else_body: None,
                        };
                    }
                    RustExpr::If {
                        condition: Box::new(negate_guard_condition(cond, ctx)),
                        then_body: vec![ret_err(err_node)],
                        else_body: None,
                    }
                }
                None => RustExpr::Comment(format!("guard: {msg} (no condition)")),
            }
        }
        StmtShape::Call | StmtShape::Assign | StmtShape::Infix | StmtShape::Block => {
            let args_node = if !a.named_args.is_empty() {
                RustExpr::StructLit {
                    name: a.target.clone(),
                    fields: a
                        .named_args
                        .iter()
                        .map(|(k, v)| (k.clone(), lower_value(v, ctx)))
                        .collect(),
                    rest: None,
                    ty: None,
                }
            } else if !a.args.is_empty() {
                RustExpr::Join {
                    items: a.args.iter().map(|e| lower_value(e, ctx)).collect(),
                    sep: ", ".to_string(),
                }
            } else {
                ident(a.target.clone())
            };
            let core = RustExpr::LayerTemplate {
                template: format!("/* {} {{args}} */", a.keyword),
                bindings: vec![("args".to_string(), args_node)],
            };
            if let Some(binding) = &a.result_binding {
                RustExpr::Let {
                    name: binding.clone(),
                    mutable: false,
                    ty: None,
                    value: Box::new(core),
                }
            } else {
                core
            }
        }
    }
}

/// Translate a full statement (expression at statement position) with semicolons.
pub fn stmt_to_rust(expr: &Expr, ctx: &mut GenCtx) -> String {
    match expr {
        Expr::Assign(name, rhs, ty_ann) | Expr::MutAssign(name, rhs, ty_ann) => {
            let s = expr_to_rust(expr, ctx);
            // Field assigns (`wt.name = x`) are not new locals.
            if !name.contains('.') {
                ctx.locals.insert(name.clone());
                // Prefer explicit type annotation (`mut x: Json = …`) so field
                // access on serde_json::Value lowers to indexing.
                if let Some(ty) = ty_ann {
                    ctx.types.local_types
                        .insert(name.clone(), crate::rust::type_to_rust(ty));
                } else if let Some(t) = infer_expr_type(rhs, ctx) {
                    ctx.types.local_types.insert(name.clone(), t);
                }
            }
            format!("    {};", s)
        }
        Expr::Action(a) if a.result_binding.is_some() => {
            let name = a.result_binding.as_ref().unwrap().clone();
            ctx.locals.insert(name.clone());
            if let Some(t) = infer_expr_type(expr, ctx) {
                ctx.types.local_types.insert(name, t);
            }
            format!("    {};", expr_to_rust(expr, ctx))
        }
        _ => {
            let rust = expr_to_rust(expr, ctx);
            if is_rust_block_stmt(expr) {
                indent_lines(&rust, "    ")
            } else {
                format!("    {};", rust.trim_end_matches(';'))
            }
        }
    }
}
